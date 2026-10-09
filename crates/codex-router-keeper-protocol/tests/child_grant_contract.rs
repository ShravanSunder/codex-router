use codex_router_keeper_protocol::{
    ChildGrantFrame, ChildGrantFrameError, ChildLaunchContext, ComponentKind, ListenerKind,
};
use std::path::Path;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const CHILD_LAUNCH_LITERAL: &str = r#"{"role":"agentProxyServices","image":{"retainedPath":"/fixture/codex-router","fileSha256":[0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31],"device":1,"inode":2},"fingerprint":"0000000000000000000000000000000000000000000000000000000000000000"}"#;
const BOOTSTRAP_LITERAL: &str = r#"{"type":"bootstrap","launch":{"role":"agentProxyServices","image":{"retainedPath":"/fixture/codex-router","fileSha256":[0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31],"device":1,"inode":2},"fingerprint":"0000000000000000000000000000000000000000000000000000000000000000"},"listeners":[{"kind":"proxyHttp"}]}"#;
const LISTENER_GRANT_LITERAL: &str = r#"{"type":"listenerGrant","listeners":[{"kind":"proxyHttp"},{"kind":"routerSessionFace","endpoint":"relay-42"}]}"#;

#[test]
fn literal_bootstrap_and_listener_grant_use_declared_wire_fields() -> TestResult {
    let bootstrap = serde_json::from_str::<ChildGrantFrame>(BOOTSTRAP_LITERAL)?;
    if serde_json::to_string(&bootstrap)? != BOOTSTRAP_LITERAL {
        return Err("Bootstrap frame differs from its independent literal JSON".into());
    }
    match bootstrap {
        ChildGrantFrame::Bootstrap { launch, listeners } => {
            if launch.role != ComponentKind::AgentProxyServices
                || launch.image.retained_path() != Path::new("/fixture/codex-router")
                || launch.image.file_sha256()
                    != &[
                        0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20,
                        21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31,
                    ]
                || launch.image.device() != 1
                || launch.image.inode() != 2
                || launch.fingerprint.as_bytes() != &[0; 32]
                || listeners != [ListenerKind::ProxyHttp]
            {
                return Err("Bootstrap literal fields changed during typed decoding".into());
            }
        }
        ChildGrantFrame::ListenerGrant { .. } => {
            return Err("Bootstrap literal decoded as ListenerGrant".into());
        }
    }

    let listener_grant = serde_json::from_str::<ChildGrantFrame>(LISTENER_GRANT_LITERAL)?;
    if serde_json::to_string(&listener_grant)? != LISTENER_GRANT_LITERAL
        || listener_grant.listener_kinds().len() != 2
    {
        return Err("ListenerGrant frame differs from its independent literal JSON".into());
    }
    Ok(())
}

#[test]
fn child_grant_rejects_bad_nested_shapes_unknown_fields_and_unknown_tags() -> TestResult {
    let malformed = [
        BOOTSTRAP_LITERAL.replace(r#""type":"bootstrap""#, r#""type":"future""#),
        BOOTSTRAP_LITERAL.replace(
            r#""role":"agentProxyServices","#,
            r#""role":"agentProxyServices","unexpected":true,"#,
        ),
        BOOTSTRAP_LITERAL.replace(
            r#""retainedPath":"/fixture/codex-router""#,
            r#""retainedPath":"relative/codex-router""#,
        ),
        BOOTSTRAP_LITERAL.replace(
            r#""fingerprint":"0000000000000000000000000000000000000000000000000000000000000000""#,
            r#""fingerprint":"not-a-fingerprint""#,
        ),
        BOOTSTRAP_LITERAL.replace(
            r#""listeners":[{"kind":"proxyHttp"}]"#,
            r#""unexpected":true,"listeners":[{"kind":"proxyHttp"}]"#,
        ),
        LISTENER_GRANT_LITERAL.replace(r#""type":"listenerGrant""#, r#""type":"future""#),
        LISTENER_GRANT_LITERAL.replace(r#""endpoint":"relay-42""#, r#""endpoint":"Relay 42""#),
        LISTENER_GRANT_LITERAL.replace(
            r#""listeners":[{"kind":"proxyHttp"}"#,
            r#""unexpected":true,"listeners":[{"kind":"proxyHttp"}"#,
        ),
    ];
    for literal in malformed {
        if serde_json::from_str::<ChildGrantFrame>(&literal).is_ok() {
            return Err("malformed ChildGrantFrame literal was accepted".into());
        }
    }
    Ok(())
}

#[test]
fn frame_constructors_and_literal_wire_enforce_variant_rights_limits() -> TestResult {
    let launch = serde_json::from_str::<ChildLaunchContext>(CHILD_LAUNCH_LITERAL)?;
    let proxy_http = serde_json::from_str::<ListenerKind>(r#"{"kind":"proxyHttp"}"#)?;
    let repeated_listeners_json = |count: usize| {
        std::iter::repeat_n(r#"{"kind":"proxyHttp"}"#, count)
            .collect::<Vec<_>>()
            .join(",")
    };
    let bootstrap_literal = |count: usize| {
        format!(
            r#"{{"type":"bootstrap","launch":{CHILD_LAUNCH_LITERAL},"listeners":[{}]}}"#,
            repeated_listeners_json(count)
        )
    };
    let listener_grant_literal = |count: usize| {
        format!(
            r#"{{"type":"listenerGrant","listeners":[{}]}}"#,
            repeated_listeners_json(count)
        )
    };

    for (count, allowed) in [(62, true), (63, false)] {
        let listeners = vec![proxy_http.clone(); count];
        let constructed = ChildGrantFrame::bootstrap(launch.clone(), listeners);
        if constructed.is_ok() != allowed {
            return Err(format!("Bootstrap constructor count {count} had wrong result").into());
        }
        let decoded = serde_json::from_str::<ChildGrantFrame>(&bootstrap_literal(count));
        if decoded.is_ok() != allowed {
            return Err(format!("Bootstrap literal count {count} had wrong result").into());
        }
        if !allowed && !matches!(constructed, Err(ChildGrantFrameError::TooManyRights)) {
            return Err("Bootstrap over-limit constructor returned the wrong error".into());
        }
    }

    for (count, allowed) in [(64, true), (65, false)] {
        let constructed = ChildGrantFrame::listener_grant(vec![proxy_http.clone(); count]);
        if constructed.is_ok() != allowed {
            return Err(format!("ListenerGrant constructor count {count} had wrong result").into());
        }
        let literal = listener_grant_literal(count);
        let decoded = serde_json::from_str::<ChildGrantFrame>(&literal);
        if decoded.is_ok() != allowed {
            return Err(format!("ListenerGrant literal count {count} had wrong result").into());
        }
        if !allowed && !matches!(constructed, Err(ChildGrantFrameError::TooManyRights)) {
            return Err("ListenerGrant over-limit constructor returned the wrong error".into());
        }
    }

    let invalid_public_variant = ChildGrantFrame::ListenerGrant {
        listeners: vec![proxy_http; 65],
    };
    if serde_json::to_string(&invalid_public_variant).is_ok() {
        return Err("invalid directly constructed ListenerGrant was serialized".into());
    }
    Ok(())
}
