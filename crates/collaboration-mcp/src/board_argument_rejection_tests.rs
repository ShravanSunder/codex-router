//! Board tools refuse arguments that do not decode with the board's typed rejection, which
//! names the field and its requirement and repeats nothing the caller supplied.
use crate::api_test_harness::{ServedApi, api_config, test_identity};
use collaboration_service::CollaborationApplication;
use serde_json::{Value, json};

const PRIVATE_VALUE: &str = "PRIVATE_ARGUMENT_VALUE_MUST_NOT_ECHO";

fn private_directory() -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt as _;
    let directory = tempfile::tempdir_in("/tmp").expect("private test directory");
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
        .expect("private directory permissions");
    directory
}

fn rejection(result: &rmcp::model::CallToolResult) -> Value {
    assert_eq!(result.is_error, Some(true), "{result:?}");
    result.structured_content.clone().expect("typed rejection")
}

#[tokio::test]
async fn malformed_board_arguments_are_typed_rejections_without_echo() {
    // Arrange: classification happens before the application, so no board store is needed.
    let directory = private_directory();
    let config = api_config(
        CollaborationApplication::new(test_identity()),
        directory.path(),
    );
    let api = ServedApi::tcp(&config).await;
    let cases = [
        // The wait tool decodes its own arguments; it classifies them like the others.
        (
            "board_thread_wait",
            json!({"filter":{"kind":"all"},"maxWaitSeconds":5}),
            "invalidIdentity",
            "actor",
            "4096",
        ),
        (
            "board_thread_wait",
            json!({"actor":{"kind":"futureSecret","private":PRIVATE_VALUE},"filter":{"kind":"all"},"maxWaitSeconds":5}),
            "invalidIdentity",
            "actor",
            "4096",
        ),
        (
            "board_project_show",
            json!({"projectId":"018f1f12-3456-7abc-8def-0123456789ab","privateField":PRIVATE_VALUE}),
            "invalidField",
            "request",
            "camelCase",
        ),
    ];

    for (tool, arguments, kind, field, requirement) in cases {
        // Act
        let result = api.call_tool(tool, arguments).await;

        // Assert
        let failure = rejection(&result);
        assert_eq!(failure["kind"], kind, "{tool}: {failure}");
        assert_eq!(failure["stage"], "validation", "{tool}: {failure}");
        assert_eq!(failure["details"]["field"], field, "{tool}: {failure}");
        assert!(
            failure["details"]["requirement"]
                .as_str()
                .is_some_and(|text| text.contains(requirement)),
            "{tool}: {failure}"
        );
        let encoded = serde_json::to_string(&result).expect("encoded result");
        for supplied in [PRIVATE_VALUE, "privateField", "futureSecret"] {
            assert!(
                !encoded.contains(supplied),
                "{tool} echoed {supplied}: {encoded}"
            );
        }
    }
    api.stop().await;
}
