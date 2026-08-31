use std::io::{Read, Write};
use vkore_addon_sdk::InvocationApproval;

fn main() {
    if let Err(error) = run() {
        eprintln!("reference add-on: {error}");
        std::process::exit(2);
    }
}

fn run() -> Result<(), String> {
    let mut input = Vec::new();
    std::io::stdin()
        .take(32_769)
        .read_to_end(&mut input)
        .map_err(|error| error.to_string())?;
    if input.len() > 32_768 {
        return Err("input too large".into());
    }
    let approval: InvocationApproval =
        serde_json::from_slice(&input).map_err(|error| error.to_string())?;
    let result = vkore_reference_addon::handle(&approval)?;
    let output = serde_json::to_vec(&result).map_err(|error| error.to_string())?;
    std::io::stdout()
        .write_all(&output)
        .map_err(|error| error.to_string())
}
