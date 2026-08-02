use super::*;

#[test]
fn inventory_validation_orders_complete_inventory_and_selects_earliest_usable() {
    let inventory = validate_credit_inventory(
        vec![
            available_credit("never", None),
            available_credit("later", Some(300)),
            available_credit("expired", Some(50)),
            available_credit("earliest", Some(200)),
            redeemed_credit("redeemed", 100),
        ],
        100,
    )
    .expect("valid inventory");
    assert_eq!(inventory.len(), 5);
    assert_eq!(
        inventory
            .display_projection()
            .into_iter()
            .map(|credit| credit.id_hint)
            .collect::<Vec<_>>(),
        ["…ired", "…emed", "…iest", "…ater", "…ever"]
    );
    assert_eq!(inventory.earliest_usable_credit_id(), Some("earliest"));
}

#[test]
fn display_projection_never_contains_complete_credit_identity() {
    let canary = "credit-full-secret-canary-8f42";
    let inventory = validate_credit_inventory(vec![available_credit(canary, Some(200))], 100)
        .expect("valid inventory");
    let projection = inventory.display_projection();
    let rendered = format!("{projection:?}");
    assert!(!rendered.contains(canary));
    assert!(rendered.contains("…8f42"));
    assert!(
        projection
            .first()
            .expect("projected credit")
            .earliest_usable
    );
}

#[test]
fn inventory_validation_fails_closed_for_any_malformed_or_unknown_credit() {
    for credit in [
        LiveResetCredit {
            id: String::new(),
            status: "available".to_owned(),
            expires_unix_seconds: None,
            expires_at: None,
            title: None,
        },
        LiveResetCredit {
            id: "unknown".to_owned(),
            status: "future".to_owned(),
            expires_unix_seconds: None,
            expires_at: None,
            title: None,
        },
        LiveResetCredit {
            id: "bad-title".to_owned(),
            status: "available".to_owned(),
            expires_unix_seconds: None,
            expires_at: None,
            title: Some("unsafe\ntext".to_owned()),
        },
        LiveResetCredit {
            id: "bad-expiry".to_owned(),
            status: "available".to_owned(),
            expires_unix_seconds: None,
            expires_at: Some("malformed".to_owned()),
            title: None,
        },
    ] {
        assert!(validate_credit_inventory(vec![credit], 100).is_err());
    }
}

#[test]
fn reset_eligibility_uses_weekly_and_credit_expiry_boundaries() {
    let expiring_at_twelve_hours = validate_credit_inventory(
        vec![available_credit("expires-at-boundary", Some(43_300))],
        100,
    )
    .expect("valid inventory");
    let expiring_after_twelve_hours = validate_credit_inventory(
        vec![available_credit("expires-after-boundary", Some(43_301))],
        100,
    )
    .expect("valid inventory");
    let non_expiring = validate_credit_inventory(vec![available_credit("never", None)], 100)
        .expect("valid inventory");
    let empty = validate_credit_inventory(Vec::new(), 100).expect("valid empty inventory");

    assert!(!reset_credit_is_eligible(LiveWeeklyUsage::new(0), &empty));
    assert!(reset_credit_is_eligible(
        LiveWeeklyUsage::new(9),
        &non_expiring
    ));
    assert!(!reset_credit_is_eligible(
        LiveWeeklyUsage::new(10),
        &non_expiring
    ));
    assert!(reset_credit_is_eligible(
        LiveWeeklyUsage::new(10),
        &expiring_at_twelve_hours
    ));
    assert!(!reset_credit_is_eligible(
        LiveWeeklyUsage::new(10),
        &expiring_after_twelve_hours
    ));
}

#[test]
fn redeem_request_identity_is_bounded_and_control_character_free() {
    assert!(RedeemRequestId::new("redeem-1".to_owned()).is_ok());
    assert!(RedeemRequestId::new(String::new()).is_err());
    assert!(RedeemRequestId::new("bad\nvalue".to_owned()).is_err());
    assert!(RedeemRequestId::new("x".repeat(257)).is_err());
}

fn available_credit(id: &str, expires_unix_seconds: Option<i64>) -> LiveResetCredit {
    LiveResetCredit {
        id: id.to_owned(),
        status: "available".to_owned(),
        expires_unix_seconds,
        expires_at: expires_unix_seconds.map(|value| format!("unix-{value}")),
        title: None,
    }
}

fn redeemed_credit(id: &str, expires: i64) -> LiveResetCredit {
    LiveResetCredit {
        id: id.to_owned(),
        status: "redeemed".to_owned(),
        expires_unix_seconds: Some(expires),
        expires_at: Some(format!("unix-{expires}")),
        title: None,
    }
}
