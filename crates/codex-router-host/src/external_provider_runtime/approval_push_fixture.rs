//! Private store-backed approval composition and native notice capture.

use automation_storage::AutomationStore;
use collaboration_protocol::{
    MachineId, PushKind, PushLineInput, PushOrigin, PushRecord, RouterLink, SessionRef,
    UuidIdentity, render_push_line,
};
use collaboration_service::{MachineIdentity, ServiceIdentity, ServiceInteractionBroker};
use std::{path::Path, sync::Arc};
use tokio::sync::Mutex;

type TestResult<TValue> = Result<TValue, Box<dyn std::error::Error + Send + Sync>>;

pub(super) struct ApprovalPushFixture {
    store: Arc<Mutex<AutomationStore>>,
    machine_identity: MachineIdentity,
}

impl ApprovalPushFixture {
    pub(super) async fn compose(
        directory: &Path,
        service_id: &UuidIdentity,
        broker: &Arc<ServiceInteractionBroker>,
    ) -> TestResult<Self> {
        let store = Arc::new(Mutex::new(
            AutomationStore::open(&directory.join("approval-pushes.sqlite")).await?,
        ));
        let machine_identity = MachineIdentity::new(service_id.clone(), Some("approval-fixture"))?;
        let service_id_text = String::from(service_id.clone());
        let identity = ServiceIdentity::new(
            &service_id_text,
            &service_id_text,
            "sha256:0000000000000000000000000000000000000000000000000000000000000000",
        )?
        .with_machine_identity(machine_identity.clone())?
        .with_automation_store(Arc::clone(&store))
        .with_approval_broker(Arc::clone(broker));
        drop(identity);
        Ok(Self {
            store,
            machine_identity,
        })
    }

    pub(super) async fn captured_approval(
        &self,
        delivered_line: &str,
        approver: &SessionRef,
    ) -> TestResult<PushRecord> {
        let (_, link_text) = delivered_line
            .rsplit_once(" · ")
            .ok_or("approval push omitted its Router link")?;
        let link = RouterLink::parse(link_text)?;
        if link.machine_id() != &MachineId::from(self.machine_identity.service_id().clone()) {
            return Err("approval push link belongs to another service".into());
        }
        let record = self
            .store
            .lock()
            .await
            .get_push_record(link.push_id())
            .await?
            .ok_or("delivered approval push was not stored")?;
        if record.kind != PushKind::Approval
            || record.origin != PushOrigin::Router(PushKind::Approval)
            || &record.target != approver
        {
            return Err("stored approval push has the wrong kind, origin or target".into());
        }
        let expected_line = render_push_line(&PushLineInput {
            link,
            machine_label: self.machine_identity.machine_label().clone(),
            origin: record.origin.clone(),
            header_facts: record.header_facts.clone(),
            body: record.body.clone(),
        })?;
        if delivered_line != expected_line || delivered_line.lines().count() != 1 {
            return Err("native approver did not receive the stored neutral push line".into());
        }
        Ok(record)
    }
}
