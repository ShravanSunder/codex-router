use codex_acp_adapter::{AcpSchemaCatalog, translate_prompt_content};
use serde_json::json;

#[test]
fn text_and_resource_links_remain_user_input_without_fetching_or_role_injection() {
    let mut catalog = AcpSchemaCatalog::load().unwrap_or_else(|error| panic!("catalog: {error}"));
    let prompt = json!({"sessionId":"thread-a","prompt":[
        {"type":"text","text":"Inspect this","_meta":{"role":"system"}},
        {"type":"resource_link","name":"design notes","uri":"https://example.invalid/private","description":"Prior reasoning"}
    ]});
    let translated = translate_prompt_content(&mut catalog, &prompt)
        .unwrap_or_else(|error| panic!("translate: {error}"));
    assert_eq!(translated.session_id(), "thread-a");
    assert_eq!(
        translated.input().first(),
        Some(&json!({"type":"text","text":"Inspect this","text_elements":[]}))
    );
    assert_eq!(translated.input().len(), 2);
    let linked = translated
        .input()
        .get(1)
        .and_then(|input| input.get("text"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_else(|| panic!("resource text"));
    assert!(linked.contains("https://example.invalid/private"));
    assert!(linked.contains("design notes"));
    assert!(linked.contains("Prior reasoning"));
}

#[test]
fn unsupported_or_invalid_content_fails_the_whole_prompt_before_submission() {
    let mut catalog = AcpSchemaCatalog::load().unwrap_or_else(|error| panic!("catalog: {error}"));
    for unsupported in [
        json!({"type":"image","data":"AA==","mimeType":"image/png"}),
        json!({"type":"text","text":42}),
    ] {
        let prompt =
            json!({"sessionId":"thread-a","prompt":[{"type":"text","text":"valid"},unsupported]});
        assert!(translate_prompt_content(&mut catalog, &prompt).is_err());
    }
}
