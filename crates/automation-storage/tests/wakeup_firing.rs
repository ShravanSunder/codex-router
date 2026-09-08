use agent_automation::{DurableMessage, ExpiryRule, OperationId, TimingRule};
use automation_storage::{AutomationStore, WakeCreate, WakeEvaluation};
use serde::{Deserialize, Serialize};
use sqlx::{Connection, SqliteConnection};
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
struct Message {
    target: String,
    text: String,
}
impl DurableMessage for Message {
    type Target = String;
    type Content = String;
    type Generation = String;
    fn target(&self) -> &String {
        &self.target
    }
    fn content(&self) -> &String {
        &self.text
    }
    fn generation_guard(&self) -> Option<&String> {
        None
    }
    fn delivery_mode(&self) -> &'static str {
        "auto"
    }
}
#[tokio::test]
async fn firing_persists_one_obligation_and_coalesces_until_delivery_resolves()
-> Result<(), Box<dyn std::error::Error>> {
    // Arrange: a repeating wake-up with no native receiver attached.
    let path = std::env::temp_dir().join(format!(
        "wake-firing-{}.sqlite",
        OperationId::generate().as_str()
    ));
    let mut store = AutomationStore::open(&path).await?;
    let wake = store
        .create_wakeup(&WakeCreate {
            operation_id: OperationId::generate(),
            message: Message {
                target: "B".into(),
                text: "Check job".into(),
            },
            timing: TimingRule::Interval { seconds: 60 },
            expiry: ExpiryRule::None,
            now_ms: 0,
        })
        .await?;
    // Act: overdue firing persists an obligation; later tick coalesces while unsent.
    let fired = store
        .evaluate_wakeup::<Message>(&wake.definition.wakeup_id, 180000)
        .await?;
    let (first, delivery) = match fired {
        WakeEvaluation::Fired {
            fire, delivery_id, ..
        } => (fire, delivery_id),
        _ => return Err("wake did not fire".into()),
    };
    let coalesced = store
        .evaluate_wakeup::<Message>(&wake.definition.wakeup_id, 240000)
        .await?;
    if !matches!(coalesced,WakeEvaluation::Coalesced{delivery_id,..} if delivery_id==delivery) {
        return Err("unresolved wake did not coalesce".into());
    }
    store.close().await?;
    // Assert: reopening and removing old events cannot erase first firing or pending content.
    let mut check =
        SqliteConnection::connect_with(&sqlx::sqlite::SqliteConnectOptions::new().filename(&path))
            .await?;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM mailbox_deliveries")
        .fetch_one(&mut check)
        .await?;
    if count != 1 {
        return Err("duplicate reminder obligations".into());
    }
    let receipt: Option<String> =
        sqlx::query_scalar("SELECT accepted_receipt_json FROM mailbox_deliveries")
            .fetch_one(&mut check)
            .await?;
    if receipt.is_some() {
        return Err("firing invented native acceptance".into());
    }
    sqlx::query("DELETE FROM automation_events")
        .execute(&mut check)
        .await?;
    check.close().await?;
    let mut store = AutomationStore::open(&path).await?;
    let current = store
        .read_wakeup::<Message>(&wake.definition.wakeup_id)
        .await?;
    if current.first_fire != Some(first) || current.pending_delivery_id != Some(delivery) {
        return Err("event cleanup erased current wake state".into());
    }
    store.close().await?;
    std::fs::remove_file(path)?;
    Ok(())
}
