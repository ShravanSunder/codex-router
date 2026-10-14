//! Provenance for the original quota account-options storyboard.
use super::*;

pub(super) fn write_receipt(
    output: &Path,
    binary: &Path,
    tools: &CaptureTools,
    columns: u16,
    rows: u16,
    expected_timeout: bool,
    request_count: usize,
) -> TestResult<()> {
    let binary_digest = digest_file(binary)?;
    let vhs_digest = digest_file(&tools.vhs)?;
    let ttyd_digest = digest_file(&tools.ttyd)?;
    let image_verifier_digest = digest_file(&tools.image_verifier)?;
    let driver_source_digest = format!("{:x}", Sha256::digest(include_bytes!("mod.rs")));
    let image_verifier_source_digest = format!(
        "{:x}",
        Sha256::digest(include_bytes!("verify_screen_image.swift"))
    );
    let font_digest = digest_file(Path::new("/System/Library/Fonts/Menlo.ttc"))?;
    let source_digest = digest_file(Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/main.rs"
    )))?;
    let image_records = if expected_timeout {
        String::new()
    } else {
        CHECKPOINTS
            .into_iter()
            .map(|name| {
                let digest = digest_file(&output.join(format!("{name}.png")))?;
                Ok(format!(
                    "{{\"checkpoint\":\"{name}\",\"png_sha256\":\"{digest}\"}}"
                ))
            })
            .collect::<TestResult<Vec<_>>>()?
            .join(",")
    };
    fs::write(
        output.join("receipt.json"),
        format!(
            "{{\"binary_sha256\":\"{binary_digest}\",\"entrypoint_source_sha256\":\"{source_digest}\",\"driver_source_sha256\":\"{driver_source_digest}\",\"image_verifier_source_sha256\":\"{image_verifier_source_digest}\",\"image_verifier_binary_sha256\":\"{image_verifier_digest}\",\"vhs_sha256\":\"{vhs_digest}\",\"ttyd_sha256\":\"{ttyd_digest}\",\"columns\":{columns},\"rows\":{rows},\"configured_font_family\":\"{FONT_FAMILY}\",\"candidate_font_file_sha256\":\"{font_digest}\",\"fixture\":\"sealed synthetic loopback data; actual quota TUI\",\"expected_timeout\":{expected_timeout},\"get_requests\":{request_count},\"post_requests\":0,\"saved_policies\":\"Disallow\",\"secret_manifest_unchanged\":true,\"owned_children_stopped\":true,\"images\":[{image_records}],\"pixel_semantics\":\"Vision checks independent scene text in every positive PNG and rejects blank/wrong images; no full styled-cell, native Ghostty or synchronized paint claim\"}}\n"
        ),
    )?;
    Ok(())
}
