//! Exports the shared DTO catalog without starting a service or accessing session state.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let schemas = communication_protocol::protocol_type_schemas()?;
    serde_json::to_writer_pretty(std::io::stdout().lock(), &schemas)?;
    Ok(())
}
