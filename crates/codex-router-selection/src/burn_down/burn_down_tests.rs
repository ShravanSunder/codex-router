//! Test coordinator for the burn-down policy.

use super::*;
use crate::run_rate::QuotaRunRateConfidence;
use codex_router_core::ids::AccountId;
use codex_router_core::provider::Provider;
use codex_router_core::route_profile::RESPONSES_HTTP;
use codex_router_core::routes::RouteBand;

#[path = "burn_down_tests/quota_account_fixtures.rs"]
mod quota_account_fixtures;
#[path = "burn_down_tests/scenario_selection_fixtures.rs"]
mod scenario_selection_fixtures;
use quota_account_fixtures::*;
use scenario_selection_fixtures::*;

#[path = "burn_down_tests/drain_selection_tests.rs"]
mod drain_selection_tests;
#[path = "burn_down_tests/floor_switch_tests.rs"]
mod floor_switch_tests;
#[path = "burn_down_tests/idle_admission_tests.rs"]
mod idle_admission_tests;
#[path = "burn_down_tests/minimum_runway_tests.rs"]
mod minimum_runway_tests;
#[path = "burn_down_tests/quota_evidence_tests.rs"]
mod quota_evidence_tests;
#[path = "burn_down_tests/runway_priority_tests.rs"]
mod runway_priority_tests;
#[path = "burn_down_tests/session_balance_tests.rs"]
mod session_balance_tests;
#[path = "burn_down_tests/weekly_floor_tests.rs"]
mod weekly_floor_tests;
#[path = "burn_down_tests/weekly_survival_tests.rs"]
mod weekly_survival_tests;
#[path = "burn_down_tests/window_guard_tests.rs"]
mod window_guard_tests;

#[test]
fn preferred_next_matches_first_strict_candidate_without_smooth_selector() {
    use std::path::{Path, PathBuf};

    fn collect_production_sources(directory: &Path, sources: &mut Vec<PathBuf>) {
        let entries = std::fs::read_dir(directory).unwrap_or_else(|error| {
            panic!("burn-down source directory should be readable: {error}")
        });
        for entry in entries {
            let entry = entry.unwrap_or_else(|error| {
                panic!("burn-down source entry should be readable: {error}")
            });
            let path = entry.path();
            if path.is_dir() {
                if path
                    .file_name()
                    .is_some_and(|name| name == "burn_down_tests")
                {
                    continue;
                }
                collect_production_sources(&path, sources);
            } else if path.extension().is_some_and(|extension| extension == "rs")
                && !path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().ends_with("_tests.rs"))
            {
                sources.push(path);
            }
        }
    }

    let manifest_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let source_root = manifest_root.join("src/burn_down.rs");
    let child_root = manifest_root.join("src/burn_down");
    let mut source_paths = vec![source_root.clone()];
    collect_production_sources(&child_root, &mut source_paths);
    source_paths.sort();
    assert!(
        !source_paths.is_empty(),
        "burn-down source collection must not be empty"
    );
    assert!(
        source_paths.iter().any(|path| path == &source_root),
        "burn-down source collection must include its root"
    );

    for source_path in source_paths {
        let source = std::fs::read_to_string(&source_path).unwrap_or_else(|error| {
            panic!(
                "burn-down source should be readable at {}: {error}",
                source_path.display()
            )
        });
        let production_source = source.split("#[cfg(test)]").next().unwrap_or(&source);
        assert!(
            !production_source.contains("WeightedDeficitSelector"),
            "burn-down preferred_next must be the first strict candidate, not a smooth weighted selector"
        );
    }
}
