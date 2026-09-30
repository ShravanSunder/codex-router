pub(crate) fn render_failure_line(explanation: &str, next_step: &str) -> String {
    format!("error: {explanation} — {next_step}")
}

pub(crate) fn render_held_line(push_link: &str, target: &str) -> String {
    format!("held: {push_link} — delivered when {target} is next running")
}

#[cfg(test)]
mod tests {
    use super::{render_failure_line, render_held_line};

    #[test]
    fn failure_line_includes_the_explanation_and_next_step() {
        assert_eq!(
            render_failure_line("target is ambiguous", "close one terminal"),
            "error: target is ambiguous — close one terminal"
        );
    }

    #[test]
    fn held_line_includes_the_link_and_delivery_condition() {
        assert_eq!(
            render_held_line("router://host/push/abc", "target-session"),
            "held: router://host/push/abc — delivered when target-session is next running"
        );
    }
}
