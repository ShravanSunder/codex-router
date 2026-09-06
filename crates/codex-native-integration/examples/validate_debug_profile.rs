//! Validate existing debug configuration without starting Codex or printing configuration values.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let home = std::env::args_os()
        .nth(1)
        .ok_or("expected normal Codex home path")?;
    let port: u16 = std::env::args()
        .nth(2)
        .ok_or("expected debug provider port")?
        .parse()?;
    let _profile =
        codex_native_integration::DebugCodexProfile::read(std::path::Path::new(&home), port)?;
    println!("existing debug profile accepted for isolated backend projection; no runtime started");
    Ok(())
}
