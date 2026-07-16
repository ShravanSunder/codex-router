use super::*;

#[test]
fn interactive_quota_requires_effective_table_format_and_both_terminals() {
    for format in [
        QuotaStatusFormat::Table,
        QuotaStatusFormat::Plain,
        QuotaStatusFormat::Json,
    ] {
        for stdin_is_terminal in [false, true] {
            for stdout_is_terminal in [false, true] {
                let expected =
                    format == QuotaStatusFormat::Table && stdin_is_terminal && stdout_is_terminal;
                assert_eq!(
                    should_run_interactive_quota(format, stdin_is_terminal, stdout_is_terminal,),
                    expected,
                    "format={format:?}, stdin={stdin_is_terminal}, stdout={stdout_is_terminal}"
                );
            }
        }
    }
}

#[test]
fn interactive_quota_component_uses_the_process_async_runtime() {
    let component_source = include_str!("../../presentation/quota/component.rs");

    for forbidden_runtime_owner in ["tokio::runtime::Builder", ".block_on(", "spawn_blocking"] {
        assert!(
            !component_source.contains(forbidden_runtime_owner),
            "interactive quota component must not own {forbidden_runtime_owner}"
        );
    }
}
