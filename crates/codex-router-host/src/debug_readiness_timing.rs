//! Opt-in debug proof timings containing fixed stage names and no configuration values.
pub(crate) fn record(stage: &'static str, started: std::time::Instant) {
    #[cfg(debug_assertions)]
    if std::env::var_os("CODEX_ROUTER_DEBUG_READINESS_TIMING").as_deref()
        == Some(std::ffi::OsStr::new("1"))
    {
        eprintln!(
            "{}",
            serde_json::json!({"kind":"debugReadinessTiming","stage":stage,"elapsedMs":started.elapsed().as_millis(),"at":chrono::Utc::now().to_rfc3339()})
        );
    }
    #[cfg(not(debug_assertions))]
    let _ = (stage, started);
}
