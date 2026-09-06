//! Export a canonical Control schema without starting a service or accessing native state.
use std::io::Write;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let native_digest = std::env::args()
        .nth(1)
        .map(communication_protocol::SchemaDigest::try_from)
        .transpose()?;
    let schema = communication_protocol::ControlSchema::generate(native_digest)?;
    std::io::stdout().lock().write_all(schema.bytes())?;
    eprintln!("{}", String::from(schema.digest().clone()));
    Ok(())
}
