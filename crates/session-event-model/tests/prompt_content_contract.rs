use session_event_model::{PromptContent, PromptEmbeddedSource};

#[test]
fn validated_blocks_preserve_agent_order_and_exact_fields() {
    let blocks = [
        PromptContent::text("read this".into()).expect("text"),
        PromptContent::resource_link(
            "https://example.test/spec".into(),
            "Specification".into(),
            Some("text/markdown".into()),
        )
        .expect("resource link"),
        PromptContent::image(
            "image/png".into(),
            "aGVsbG8=".into(),
            Some("file:///image.png".into()),
        )
        .expect("image"),
        PromptContent::audio("audio/wav".into(), "aGVsbG8=".into()).expect("audio"),
        PromptContent::embedded_blob("resource://fixture".into(), None, "aGVsbG8=".into())
            .expect("embedded blob"),
    ];
    assert!(matches!(&blocks[0], PromptContent::Text { text } if text.as_str() == "read this"));
    assert!(
        matches!(&blocks[1], PromptContent::ResourceLink { uri, name, mime_type }
        if uri.as_str() == "https://example.test/spec"
            && name.as_str() == "Specification"
            && mime_type.as_ref().map(|value| value.as_str()) == Some("text/markdown"))
    );
    assert!(
        matches!(&blocks[2], PromptContent::Image { mime_type, data, uri }
        if mime_type.as_str() == "image/png"
            && data.as_str() == "aGVsbG8="
            && uri.as_ref().map(|value| value.as_str()) == Some("file:///image.png"))
    );
    assert!(
        matches!(&blocks[4], PromptContent::EmbeddedResource { source: PromptEmbeddedSource::Blob(data), .. }
        if data.as_str() == "aGVsbG8=")
    );
}

#[test]
fn constructors_reject_empty_required_values_and_uri_only_images() {
    assert!(PromptContent::text(String::new()).is_err());
    assert!(
        PromptContent::resource_link("https://example.test".into(), String::new(), None).is_err()
    );
    assert!(
        PromptContent::image(
            "image/png".into(),
            String::new(),
            Some("file:///image.png".into())
        )
        .is_err()
    );
    assert!(PromptContent::audio("audio/wav".into(), String::new()).is_err());
    assert!(
        PromptContent::embedded_text("resource://fixture".into(), None, String::new()).is_err()
    );
    assert!(
        PromptContent::embedded_blob("resource://fixture".into(), None, "bytes\0unsafe".into())
            .is_err()
    );
}
