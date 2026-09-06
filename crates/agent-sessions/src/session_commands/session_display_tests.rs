use super::*;

#[test]
fn explicit_session_name_precedes_derived_title_and_is_searchable() {
    let mut record = search_consistency_record(None, None);
    record.name = Some("Website stuff".to_owned());
    record.title = Some("Derived first-message title".to_owned());
    record.display_title = super::display_title_from_session_fields(
        record.name.as_deref(),
        record.title.as_deref(),
        record.preview.as_deref(),
        record.first_user_message.as_deref(),
    );

    let picker_record = SessionPickerRecord::from_record(&record);

    assert_eq!(
        picker_record.title,
        "Website stuff | Derived first-message title"
    );
    for query in ["website stuff", "derived first-message"] {
        let expression = SessionSearchExpression::parse(query);
        assert!(record.matches_search(&expression), "loader search: {query}");
        assert!(
            picker_record.matches_search(&expression),
            "picker search: {query}"
        );
    }
}

#[test]
fn missing_explicit_session_name_keeps_the_previous_display_fallback() {
    let mut record = search_consistency_record(None, Some("fallback first message"));
    record.title = Some("previous title".to_owned());
    record.display_title = super::display_title_from_session_fields(
        record.name.as_deref(),
        record.title.as_deref(),
        record.preview.as_deref(),
        record.first_user_message.as_deref(),
    );
    let picker_record = SessionPickerRecord::from_record(&record);

    assert_eq!(picker_record.title, "previous title");
}

#[cfg(unix)]
#[test]
fn picker_record_normalizes_existing_cwd_before_interactive_matching() {
    let fixture_root = std::env::temp_dir().join(format!(
        "codex-router-session-picker-path-normalization-{}",
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
    let actual_cwd = picker_record.normalized_cwd;
    let expected_cwd = fs::canonicalize(&canonical_checkout)
        .expect("canonicalize checkout")
        .display()
        .to_string();
    fs::remove_file(&checkout_alias).expect("remove checkout symlink");
    fs::remove_dir(&canonical_checkout).expect("remove canonical checkout");
    fs::remove_dir(&fixture_root).expect("remove fixture root");

    assert_eq!(actual_cwd.as_deref(), Some(expected_cwd.as_str()));
}

#[test]
fn duration_format_uses_now_without_suffix_for_subminute_values() {
    assert_eq!(format_duration_ms(0), "now");
    assert_eq!(format_duration_ms(59_000), "now");
    assert_eq!(format_duration_ms(60_000), "1m");
}
