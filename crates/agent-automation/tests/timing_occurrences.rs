use agent_automation::TimingRule;
use chrono::{DateTime, Utc};

fn instant(value: &str) -> Result<DateTime<Utc>, chrono::ParseError> {
    Ok(DateTime::parse_from_rfc3339(value)?.with_timezone(&Utc))
}
#[test]
fn interval_resume_preserves_original_anchor_without_replaying_ticks()
-> Result<(), Box<dyn std::error::Error>> {
    // Arrange: every ten minutes, anchored at 12:03 rather than a cron boundary.
    let rule = TimingRule::Interval(600);
    let anchor = instant("2026-09-08T12:03:00Z")?;
    // Act: resume after multiple intentionally skipped ticks.
    let next = rule.next_due(anchor, Some(instant("2026-09-08T12:36:00Z")?))?;
    // Assert: 12:43 preserves the anchor; it is neither 12:40 nor 12:46.
    if next != Some(instant("2026-09-08T12:43:00Z")?) {
        return Err("interval anchor changed or old ticks replayed".into());
    }
    Ok(())
}
#[test]
fn one_shot_has_no_next_occurrence_after_its_due_time() -> Result<(), Box<dyn std::error::Error>> {
    // Arrange: a one-shot after ten minutes.
    let rule = TimingRule::After(600);
    let anchor = instant("2026-09-08T12:00:00Z")?;
    // Act / Assert: it first fires once, then is exhausted.
    if rule.next_due(anchor, None)? != Some(instant("2026-09-08T12:10:00Z")?) {
        return Err("first one-shot missing".into());
    }
    if rule
        .next_due(anchor, Some(instant("2026-09-08T12:20:00Z")?))?
        .is_some()
    {
        return Err("one-shot repeated".into());
    }
    Ok(())
}
#[test]
fn cron_uses_explicit_timezone_and_rejects_extra_fields() -> Result<(), Box<dyn std::error::Error>>
{
    // Arrange: calendar time in Toronto, not an elapsed interval.
    let anchor = instant("2026-09-08T12:00:00Z")?;
    let rule = TimingRule::Cron {
        expression: "30 9 * * *".into(),
        timezone: "America/Toronto".into(),
    };
    // Act / Assert: 09:30 local is 13:30 UTC on this date.
    if rule.next_due(anchor, None)? != Some(instant("2026-09-08T13:30:00Z")?) {
        return Err("cron timezone not applied".into());
    }
    let invalid = TimingRule::Cron {
        expression: "0 30 9 * * *".into(),
        timezone: "UTC".into(),
    };
    if invalid.next_due(anchor, None).is_ok() {
        return Err("six-field cron accepted".into());
    }
    Ok(())
}
