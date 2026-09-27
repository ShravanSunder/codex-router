use super::*;
use serde_json::json;

#[test]
fn acp_prompt_keeps_text_and_resource_link_as_distinct_blocks() {
    let parsed = parse_prompt_content(&json!({"prompt":[
        {"type":"text","text":"inspect this"},
        {"type":"resource_link","uri":"file:///tmp/report.txt","name":"report.txt","mimeType":"text/plain"}
    ]}));
    let expected = vec![
        CommandContent::text("inspect this".into()).expect("text"),
        CommandContent::resource_link(
            "file:///tmp/report.txt".into(),
            "report.txt".into(),
            Some("text/plain".into()),
        )
        .expect("link"),
    ];
    assert_eq!(parsed, Ok(expected));
}

#[test]
fn acp_prompt_validates_media_and_embedded_resource_shapes() {
    let parsed = parse_prompt_content(&json!({"prompt":[
        {"type":"image","data":"AQID","mimeType":"image/png"},
        {"type":"audio","data":"AQID","mimeType":"audio/wav"},
        {"type":"resource","resource":{"uri":"file:///tmp/data","mimeType":"text/plain","text":"inside"}}
    ]}));
    assert!(matches!(parsed, Ok(ref blocks) if blocks.len() == 3));
    assert!(
        parse_prompt_content(
            &json!({"prompt":[{"type":"image","data":"bad!","mimeType":"image/png"}]})
        )
        .is_err()
    );
    assert!(
        parse_prompt_content(
            &json!({"prompt":[{"type":"resource_link","uri":"file:///tmp/report","name":""}]})
        )
        .is_err()
    );
}
