use codex_router_keeper_protocol::{DefaultEndpointPath, GenerationAliasPath, GenerationId};
use std::path::PathBuf;
type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;
#[test]
fn effective_debug_basename_and_literal_generation_sibling_are_preserved_without_io() -> TestResult
{
    let endpoint =
        DefaultEndpointPath::try_from(PathBuf::from("/absent-test-home/private/app-server.sock"))?;
    let generation: GenerationId =
        serde_json::from_str(r#"{"epoch":"11111111-2222-4333-8444-555555555555","number":7}"#)?;
    let alias = GenerationAliasPath::for_generation(&endpoint, &generation)?;
    if alias.as_path() != std::path::Path::new("/absent-test-home/private/gen-11111111-7.sock") {
        return Err("alias differs from independent literal".into());
    }
    alias.validate_for(&endpoint, &generation)?;
    if serde_json::to_string(&alias)? != r#""/absent-test-home/private/gen-11111111-7.sock""# {
        return Err("alias wire changed".into());
    }
    Ok(())
}
#[test]
fn malformed_alias_and_nonfile_paths_are_rejected() {
    for path in [
        "",
        "relative.sock",
        "/",
        "/private/../endpoint.sock",
        "/private/endpoint.sock/",
        "/private/endpoint.sock/.",
        "/private/endpoint\0.sock",
    ] {
        assert!(DefaultEndpointPath::try_from(PathBuf::from(path)).is_err());
    }
    for path in [
        "/private/gen-11111111-0.sock",
        "/private/gen-11111111-01.sock",
        "/private/gen-11111111-18446744073709551616.sock",
        "/private/gen-1111111-1.sock",
        "/private/gen-G1111111-1.sock",
        "/private/not-generation.sock",
        "gen-11111111-1.sock",
    ] {
        assert!(GenerationAliasPath::try_from(PathBuf::from(path)).is_err());
    }
}
#[test]
fn epoch_number_and_parent_relationships_are_checked_with_the_actual_generation() -> TestResult {
    let endpoint =
        DefaultEndpointPath::try_from(PathBuf::from("/private/app-server-control.sock"))?;
    let generation: GenerationId =
        serde_json::from_str(r#"{"epoch":"11111111-2222-4333-8444-555555555555","number":7}"#)?;
    for path in [
        "/other/gen-11111111-7.sock",
        "/private/gen-22222222-7.sock",
        "/private/gen-11111111-8.sock",
    ] {
        if GenerationAliasPath::try_from(PathBuf::from(path))?
            .validate_for(&endpoint, &generation)
            .is_ok()
        {
            return Err("mismatched alias relationship accepted".into());
        }
    }
    Ok(())
}

#[test]
fn a_generation_alias_cannot_be_the_effective_endpoint_itself() -> TestResult {
    let endpoint = DefaultEndpointPath::try_from(PathBuf::from("/private/gen-11111111-7.sock"))?;
    let generation: GenerationId =
        serde_json::from_str(r#"{"epoch":"11111111-2222-4333-8444-555555555555","number":7}"#)?;
    if GenerationAliasPath::for_generation(&endpoint, &generation).is_ok() {
        return Err("self-referential alias construction accepted".into());
    }
    Ok(())
}
