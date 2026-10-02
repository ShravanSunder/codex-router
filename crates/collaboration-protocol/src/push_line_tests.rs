use crate::{
    EndpointId, EndpointRef, SessionDisplayName, SessionId, SessionRef, UuidIdentity,
    push_line::{
        MAX_PUSH_LINE_BYTES, MachineId, MachineLabel, ParsedPushLineHeader, PushHeaderFacts,
        PushId, PushKind, PushLineInput, PushOrigin, RouterLink, parse_push_line_header,
        render_push_line,
    },
};
use agent_automation::{RunId, ScheduleId};

const MACHINE_ID: &str = "018f47d2-24d5-7a68-b9ec-6f759c39458f";
const PUSH_ID: &str = "018f47d2-24d5-7a68-b9ec-6f759c39458f";

fn machine_id() -> MachineId {
    MachineId::try_from(MACHINE_ID.to_owned()).expect("valid machine UUID")
}

fn push_id() -> PushId {
    PushId::try_from(PUSH_ID.to_owned()).expect("valid UUIDv7 push id")
}

fn router_link() -> RouterLink {
    RouterLink::new(machine_id(), push_id())
}

fn session(endpoint_id: &str, session_id: &str) -> SessionRef {
    SessionRef {
        endpoint: EndpointRef {
            service_id: UuidIdentity::try_from(MACHINE_ID.to_owned()).expect("valid service id"),
            endpoint_id: EndpointId::try_from(endpoint_id.to_owned()).expect("valid endpoint id"),
        },
        session_id: SessionId::try_from(session_id.to_owned()).expect("valid session id"),
    }
}

fn display_name(value: &str) -> Option<SessionDisplayName> {
    Some(SessionDisplayName::try_from(value.to_owned()).expect("valid display name"))
}

fn line_input(
    origin: PushOrigin,
    header_facts: PushHeaderFacts,
    body: Option<String>,
    machine_label: &str,
) -> PushLineInput {
    PushLineInput {
        link: router_link(),
        machine_label: MachineLabel::try_from(machine_label.to_owned())
            .expect("valid machine label"),
        origin,
        header_facts,
        body,
    }
}

fn agent_dm_input(body: String, machine_label: &str) -> PushLineInput {
    line_input(
        PushOrigin::Session(session("claude-local", "12345678-session")),
        PushHeaderFacts::DirectMessage {
            sender_display_name: display_name("✳️ Main"),
        },
        Some(body),
        machine_label,
    )
}

fn arbitrary_display_name(source_scalars: &[char]) -> SessionDisplayName {
    let mut value = String::from("Name");
    for scalar in source_scalars
        .iter()
        .copied()
        .filter(|scalar| !scalar.is_control())
    {
        let scalar = match scalar {
            '→' => '›',
            '←' => '‹',
            other => other,
        };
        if value.chars().count() == 120 {
            break;
        }
        value.push(scalar);
    }
    SessionDisplayName::try_from(value).expect("bounded display name")
}

fn arbitrary_uuid_v7(seed: u64) -> String {
    format!(
        "018f47d2-24d5-7a68-b9ec-{:012x}",
        seed & 0x0000_ffff_ffff_ffff
    )
}

fn bounded_fixture_text(prefix: &str, source_scalars: &[char], max_scalars: usize) -> String {
    let mut value = prefix.to_owned();
    let remaining_scalars = max_scalars.saturating_sub(value.chars().count());
    value.extend(source_scalars.iter().copied().take(remaining_scalars));
    value
}

fn assert_push_line_invariants(
    request: &PushLineInput,
) -> Result<(), proptest::test_runner::TestCaseError> {
    let rendered = render_push_line(request).expect("valid push line input renders");
    let has_raw_line_break = rendered
        .chars()
        .any(|scalar| matches!(scalar, '\r' | '\n' | '\u{2028}' | '\u{2029}'));
    proptest::prop_assert!(rendered.len() <= MAX_PUSH_LINE_BYTES);
    proptest::prop_assert_eq!(rendered.lines().count(), 1);
    proptest::prop_assert!(!has_raw_line_break);
    proptest::prop_assert!(rendered.ends_with(&request.link.to_string()));
    Ok(())
}

#[test]
fn agent_direct_message_uses_the_header_grammar_and_complete_link() {
    let request = agent_dm_input("S1 is green".to_owned(), "Sunbook-Pro-M4");

    let rendered = render_push_line(&request).expect("push line");

    assert_eq!(
        rendered,
        format!(
            "✉️ ✳️ Main (claude-local/12345678) @Sunbook-Pro-M4 → you · \"S1 is green\" · {}",
            request.link
        )
    );
}

#[test]
fn owner_unverified_direct_message_uses_the_compact_line() {
    let request = line_input(
        PushOrigin::OwnerUnverified,
        PushHeaderFacts::DirectMessage {
            sender_display_name: None,
        },
        Some("owner text".to_owned()),
        "Sunbook-Pro-M4",
    );

    let rendered = render_push_line(&request).expect("push line");

    assert!(rendered.starts_with("🧑 Owner (unverified) @Sunbook-Pro-M4 → you · "));
    assert!(rendered.contains("\"owner text\""));
    assert!(!rendered.contains("Self-declared sender:"));
    assert!(!rendered.contains("Intended recipient:"));
    assert!(rendered.ends_with(&request.link.to_string()));
}

#[test]
fn preview_counts_source_scalars_before_escaping_and_reports_the_exact_remainder() {
    let body = "🪿".repeat(101);
    let request = agent_dm_input(body, "machine");

    let rendered = render_push_line(&request).expect("push line");

    assert!(rendered.contains(&format!("\"{}\" (+1)", "🪿".repeat(100))));
}

#[test]
fn preview_escapes_quotes_slashes_line_breaks_unicode_separators_and_controls() {
    let body = "quote \" slash \\ CR\rLF\nLS\u{2028}PS\u{2029}control\u{0001}".to_owned();
    let request = agent_dm_input(body, "machine");

    let rendered = render_push_line(&request).expect("push line");

    assert!(rendered.contains(r#"quote \" slash \\ CR⏎LF⏎LS⏎PS⏎control\u{1}"#));
}

#[test]
fn machine_label_separators_are_isolated_from_structural_separators() {
    let request = agent_dm_input("hello".to_owned(), "Node · forged");

    let rendered = render_push_line(&request).expect("push line");

    assert!(rendered.contains(r#"@Node \u{b7} forged → you"#));
    assert_eq!(rendered.matches(" · ").count(), 2);
}

#[test]
fn machine_label_escapes_quotes_backslashes_controls_and_unicode_line_breaks() {
    let request = agent_dm_input(
        "hello".to_owned(),
        "Host\" \\ node\r\n\u{2028}\u{2029}\u{0001} · branch",
    );

    let rendered = render_push_line(&request).expect("push line");

    assert!(rendered.contains(r#"@Host\" \\ node⏎⏎⏎⏎\u{1} \u{b7} branch → you"#));
    assert_eq!(rendered.lines().count(), 1);
}

#[test]
fn every_push_kind_renders_as_one_bounded_line_with_a_complete_link() {
    let requester = session("codex-local", "fedcba98-requester");
    let cases = [
        line_input(
            PushOrigin::Session(requester.clone()),
            PushHeaderFacts::DirectMessage {
                sender_display_name: display_name("🤖 Codex Main"),
            },
            Some("dm".to_owned()),
            "machine",
        ),
        line_input(
            PushOrigin::OwnerUnverified,
            PushHeaderFacts::DirectMessage {
                sender_display_name: None,
            },
            Some("owner".to_owned()),
            "machine",
        ),
        line_input(
            PushOrigin::Router(PushKind::Wake),
            PushHeaderFacts::Wake,
            Some("wake instruction".to_owned()),
            "machine",
        ),
        line_input(
            PushOrigin::Router(PushKind::ScheduleRun),
            PushHeaderFacts::ScheduleRun {
                schedule_id: ScheduleId::generate(),
                run_id: RunId::generate(),
            },
            Some("schedule instruction".to_owned()),
            "machine",
        ),
        line_input(
            PushOrigin::Router(PushKind::Approval),
            PushHeaderFacts::Approval {
                requester: requester.clone(),
                requester_display_name: display_name("🤖 Codex Main"),
            },
            Some("approval request".to_owned()),
            "machine",
        ),
        line_input(
            PushOrigin::Router(PushKind::Question),
            PushHeaderFacts::Question {
                requester,
                requester_display_name: display_name("🤖 Codex Main"),
            },
            Some("question request".to_owned()),
            "machine",
        ),
        line_input(
            PushOrigin::Router(PushKind::SubscriptionActivity),
            PushHeaderFacts::SubscriptionActivity {
                root_count: 2,
                message_count: 5,
                held_since: Some("2026-09-30T12:00:00Z".to_owned()),
                thread_resolved: true,
            },
            None,
            "machine",
        ),
        line_input(
            PushOrigin::Router(PushKind::SubscriptionExpiry),
            PushHeaderFacts::SubscriptionExpiry {
                scope: "thread".to_owned(),
            },
            None,
            "machine",
        ),
    ];

    for request in cases {
        let rendered = render_push_line(&request).expect("push line");

        assert!(rendered.len() <= 1024, "{rendered:?}");
        assert_eq!(rendered.lines().count(), 1);
        assert!(rendered.ends_with(&request.link.to_string()));
    }
}

#[test]
fn schedule_header_uses_short_typed_ids_without_instruction_text() {
    let schedule_id = ScheduleId::generate();
    let run_id = RunId::generate();
    let request = line_input(
        PushOrigin::Router(PushKind::ScheduleRun),
        PushHeaderFacts::ScheduleRun {
            schedule_id: schedule_id.clone(),
            run_id: run_id.clone(),
        },
        Some("Inspect the confidential quarterly plan".to_owned()),
        "machine",
    );

    let rendered = render_push_line(&request).expect("scheduled push line");
    let header = parse_push_line_header(&rendered).expect("scheduled push title");

    assert_eq!(
        rendered,
        format!(
            "🗓 Router schedule {} @machine · run {} · \"Inspect the confidential quarterly plan\" · {}",
            schedule_id.as_str().chars().take(8).collect::<String>(),
            run_id.as_str().chars().take(8).collect::<String>(),
            request.link,
        )
    );
    assert_eq!(header.kind, PushKind::ScheduleRun);
    assert_eq!(
        header.title,
        format!(
            "🗓 Router schedule {} @machine · run {}",
            schedule_id.as_str().chars().take(8).collect::<String>(),
            run_id.as_str().chars().take(8).collect::<String>(),
        )
    );
    assert!(!header.title.contains("confidential"));
}

#[test]
fn subscription_notices_are_neutral_and_have_no_preview() {
    for (origin, facts) in [
        (
            PushOrigin::Router(PushKind::SubscriptionActivity),
            PushHeaderFacts::SubscriptionActivity {
                root_count: 2,
                message_count: 5,
                held_since: Some("2026-09-30T12:00:00Z".to_owned()),
                thread_resolved: false,
            },
        ),
        (
            PushOrigin::Router(PushKind::SubscriptionExpiry),
            PushHeaderFacts::SubscriptionExpiry {
                scope: "thread".to_owned(),
            },
        ),
    ] {
        let request = line_input(origin, facts, None, "machine");
        let rendered = render_push_line(&request).expect("neutral subscription notice");

        assert!(!rendered.contains('"'));
        assert!(!rendered.contains("peer authored"));
        assert!(rendered.ends_with(&request.link.to_string()));
    }
}

#[test]
fn line_budget_trims_display_fields_without_truncating_emoji_or_link() {
    let machine_label = "🪿".repeat(120);
    let mut request = agent_dm_input("🪿".repeat(200), &machine_label);
    request.header_facts = PushHeaderFacts::DirectMessage {
        sender_display_name: display_name(&format!("✳️ {}", "界".repeat(117))),
    };

    let rendered = render_push_line(&request).expect("bounded push line");

    assert!(rendered.len() <= 1024);
    assert!(rendered.starts_with("✉️ "));
    assert!(rendered.contains('…'));
    assert!(rendered.ends_with(&request.link.to_string()));
}

#[test]
fn header_parser_returns_display_facts_without_the_preview() {
    let request = agent_dm_input("private preview text".to_owned(), "machine");
    let rendered = render_push_line(&request).expect("push line");

    let ParsedPushLineHeader { kind, title } =
        parse_push_line_header(&rendered).expect("display header");

    assert_eq!(kind, PushKind::DirectMessage);
    assert!(title.contains("Main"));
    assert!(!title.contains("private preview text"));
}

#[test]
fn router_link_parses_only_local_canonical_uuidv7_push_links() {
    let link = router_link();

    assert_eq!(
        RouterLink::parse(&link.to_string()).expect("valid link"),
        link
    );
    assert!(
        RouterLink::parse("https://machine/push/018f47d2-24d5-7a68-b9ec-6f759c39458f").is_err()
    );
    assert!(RouterLink::parse("router://018f47d2-24d5-7a68-b9ec-6f759c39458f/push/018f47d2-24d5-4a68-b9ec-6f759c39458f").is_err());
    assert!(RouterLink::parse("router://018f47d2-24d5-7a68-b9ec-6f759c39458f/push/018f47d2-24d5-7a68-b9ec-6f759c39458f/extra").is_err());
}

#[test]
fn origin_and_header_kind_must_agree() {
    let request = line_input(
        PushOrigin::Router(PushKind::Wake),
        PushHeaderFacts::DirectMessage {
            sender_display_name: None,
        },
        Some("body".to_owned()),
        "machine",
    );

    assert!(render_push_line(&request).is_err());
}

proptest::proptest! {
    #![proptest_config(proptest::test_runner::Config {
        cases: 64,
        failure_persistence: None,
        ..proptest::test_runner::Config::default()
    })]
    #[test]
    fn arbitrary_display_fields_for_every_push_kind_fit_and_keep_the_link_suffix(
        machine_scalars in proptest::collection::vec(proptest::char::any(), 0..300),
        name_scalars in proptest::collection::vec(proptest::char::any(), 0..160),
        body_scalars in proptest::collection::vec(proptest::char::any(), 0..500),
        held_since_scalars in proptest::collection::vec(proptest::char::any(), 0..100),
        scope_scalars in proptest::collection::vec(proptest::char::any(), 0..100),
        schedule_seed in proptest::prelude::any::<u64>(),
        run_seed in proptest::prelude::any::<u64>(),
        root_count in 1_u8..=20,
        message_count in proptest::prelude::any::<u64>(),
        held_since_present in proptest::prelude::any::<bool>(),
        thread_resolved in proptest::prelude::any::<bool>()
    ) {
        let machine_text: String = machine_scalars.into_iter().collect();
        let machine_label = format!("Machine {machine_text}");
        let name = arbitrary_display_name(&name_scalars);
        let body: String = body_scalars.into_iter().collect();
        let requester = session("claude-local", "12345678-session");
        let schedule_id = ScheduleId::try_from(arbitrary_uuid_v7(schedule_seed))
            .expect("generated schedule UUIDv7");
        let run_id = RunId::try_from(arbitrary_uuid_v7(run_seed)).expect("generated run UUIDv7");
        let held_since = held_since_present.then(|| {
            bounded_fixture_text("since ", &held_since_scalars, 64)
        });
        let scope = bounded_fixture_text("scope", &scope_scalars, 64);

        // ScheduleRun currently stores IDs only; its absent display-name field is an explicit follow-up.
        for kind in [
            PushKind::DirectMessage,
            PushKind::Wake,
            PushKind::ScheduleRun,
            PushKind::Approval,
            PushKind::Question,
            PushKind::SubscriptionActivity,
            PushKind::SubscriptionExpiry,
        ] {
            let request = match kind {
                PushKind::DirectMessage => line_input(
                    PushOrigin::Session(requester.clone()),
                    PushHeaderFacts::DirectMessage {
                        sender_display_name: Some(name.clone()),
                    },
                    Some(body.clone()),
                    &machine_label,
                ),
                PushKind::Wake => line_input(
                    PushOrigin::Router(PushKind::Wake),
                    PushHeaderFacts::Wake,
                    Some(body.clone()),
                    &machine_label,
                ),
                PushKind::ScheduleRun => line_input(
                    PushOrigin::Router(PushKind::ScheduleRun),
                    PushHeaderFacts::ScheduleRun {
                        schedule_id: schedule_id.clone(),
                        run_id: run_id.clone(),
                    },
                    Some(body.clone()),
                    &machine_label,
                ),
                PushKind::Approval => line_input(
                    PushOrigin::Router(PushKind::Approval),
                    PushHeaderFacts::Approval {
                        requester: requester.clone(),
                        requester_display_name: Some(name.clone()),
                    },
                    Some(body.clone()),
                    &machine_label,
                ),
                PushKind::Question => line_input(
                    PushOrigin::Router(PushKind::Question),
                    PushHeaderFacts::Question {
                        requester: requester.clone(),
                        requester_display_name: Some(name.clone()),
                    },
                    Some(body.clone()),
                    &machine_label,
                ),
                PushKind::SubscriptionActivity => line_input(
                    PushOrigin::Router(PushKind::SubscriptionActivity),
                    PushHeaderFacts::SubscriptionActivity {
                        root_count,
                        message_count,
                        held_since: held_since.clone(),
                        thread_resolved,
                    },
                    None,
                    &machine_label,
                ),
                PushKind::SubscriptionExpiry => line_input(
                    PushOrigin::Router(PushKind::SubscriptionExpiry),
                    PushHeaderFacts::SubscriptionExpiry {
                        scope: scope.clone(),
                    },
                    None,
                    &machine_label,
                ),
            };
            assert_push_line_invariants(&request)?;

            if kind == PushKind::DirectMessage {
                let owner_request = line_input(
                    PushOrigin::OwnerUnverified,
                    PushHeaderFacts::DirectMessage {
                        sender_display_name: None,
                    },
                    Some(body.clone()),
                    &machine_label,
                );
                assert_push_line_invariants(&owner_request)?;
            }
        }
    }
}
