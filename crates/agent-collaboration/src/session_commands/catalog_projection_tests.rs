use super::*;

#[test]
fn picker_search_uses_the_same_complete_persisted_fields_as_loader_search() {
    let record = search_consistency_record(None, Some("deploy\nrollback plan"));
    let picker_record = SessionPickerRecord::from_record(&record);
    let expression = SessionSearchExpression::parse("\"deploy rollback\"");

    assert_eq!(
        picker_record.matches_search(&expression),
        record.matches_search(&expression)
    );
    assert!(!picker_record.matches_search(&expression));
}

#[cfg(unix)]
#[test]
fn picker_search_preserves_raw_persisted_cwd_for_loader_parity() {
    let fixture_root = std::env::temp_dir().join(format!(
        "codex-router-session-picker-search-cwd-{}",
        std::process::id()
    ));
    let canonical_checkout = fixture_root.join("canonical-checkout");
    let checkout_alias = fixture_root.join("checkout-alias");
    fs::create_dir_all(&canonical_checkout).expect("create canonical checkout");
    std::os::unix::fs::symlink(&canonical_checkout, &checkout_alias)
        .expect("create checkout symlink");
    let mut record = search_consistency_record(None, None);
    record.cwd = Some(checkout_alias.display().to_string());
    let picker_record = SessionPickerRecord::from_record(&record);
    let alias_expression = SessionSearchExpression::parse("checkout-alias");
    let canonical_expression = SessionSearchExpression::parse("canonical-checkout");
    let actual = [
        picker_record.matches_search(&alias_expression),
        picker_record.matches_search(&canonical_expression),
    ];
    let expected = [
        record.matches_search(&alias_expression),
        record.matches_search(&canonical_expression),
    ];
    fs::remove_file(&checkout_alias).expect("remove checkout symlink");
    fs::remove_dir(&canonical_checkout).expect("remove canonical checkout");
    fs::remove_dir(&fixture_root).expect("remove fixture root");

    assert_eq!(actual, expected);
}

#[test]
fn picker_search_does_not_match_missing_branch_display_placeholder() {
    let mut record = search_consistency_record(None, None);
    record.git_branch = None;
    let picker_record = SessionPickerRecord::from_record(&record);
    let expression = SessionSearchExpression::parse("b:-");

    assert_eq!(
        picker_record.matches_search(&expression),
        record.matches_search(&expression)
    );
    assert!(!picker_record.matches_search(&expression));
}

#[test]
fn picker_repository_matching_normalizes_persisted_origin_exactly_once() {
    let raw_origin = "http://gitlab.internal:8443/team/app.git";
    let identity = RepositoryIdentity {
        normalized_origin: collaboration_client::board::normalize_git_origin_url(raw_origin),
        live_roots: vec![PathBuf::from("/dev/app")],
        repository_basename: "app".to_owned(),
        fallback_cwd: None,
    };
    let record = search_consistency_record(Some(raw_origin), None);
    let picker_record = SessionPickerRecord::from_record(&record);

    assert!(session_belongs_to_repository(
        &identity,
        picker_record.git_origin_url.as_deref(),
        std::path::Path::new(picker_record.cwd.as_deref().expect("record cwd")),
    ));
}

#[test]
fn picker_record_carries_the_stored_model_and_reasoning_effort() {
    // Arrange: the picker resumes from these values, so it must project both.
    let mut record = search_consistency_record(None, None);
    record.model = Some("gpt-5.6-sol".to_owned());
    record.reasoning_effort = Some("low".to_owned());

    // Act
    let picker_record = SessionPickerRecord::from_record(&record);

    // Assert
    assert_eq!(picker_record.model.as_deref(), Some("gpt-5.6-sol"));
    assert_eq!(picker_record.reasoning_effort.as_deref(), Some("low"));

    // Arrange / Act / Assert: an unknown effort is never invented.
    record.reasoning_effort = None;
    assert_eq!(
        SessionPickerRecord::from_record(&record).reasoning_effort,
        None
    );
}
