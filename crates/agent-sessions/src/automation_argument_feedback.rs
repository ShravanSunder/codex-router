//! Preserve explicit machine output when argument validation fails before any service call.
use clap::Parser;
use std::{ffi::OsString, io::Write};

pub(crate) fn parse_arguments<TArguments: Parser>(
    arguments: Vec<OsString>,
) -> Result<TArguments, i32> {
    let machine = arguments
        .iter()
        .take_while(|arg| *arg != "--")
        .any(|arg| arg == "--json");
    match TArguments::try_parse_from(arguments) {
        Ok(value) => Ok(value),
        Err(error) if error.use_stderr() && machine => {
            let _written = writeln!(
                std::io::stdout(),
                "{}",
                serde_json::json!({
                    "kind":"error",
                    "operationId":null,
                    "error":{
                        "kind":"invalidField", "stage":"validation",
                        "message":error.to_string(), "field":"arguments",
                        "constraint":"Use the command help to correct the arguments.",
                        "effects":{"kind":"local","mutation":"none"},
                        "nextAction":"correctRequest"
                    }
                })
            );
            Err(2)
        }
        Err(error) => {
            let code = if error.use_stderr() { 2 } else { 0 };
            let _printed = error.print();
            Err(code)
        }
    }
}
