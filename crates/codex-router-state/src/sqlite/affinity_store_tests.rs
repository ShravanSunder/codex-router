use super::*;

#[cfg(test)]
#[test]
fn corrupt_session_account_affinity_uses_pin_specific_error() {
    let error = parse_session_account_affinity_row(
        "unknown-provider".to_owned(),
        "session-corrupt-provider".to_owned(),
        None,
        1_000,
        0,
    )
    .expect_err("unknown stored providers must fail closed");

    assert!(matches!(
        error,
        StateStoreError::CorruptSessionAccountAffinity {
            session_id,
            field: "provider"
        } if session_id == "session-corrupt-provider"
    ));
}
