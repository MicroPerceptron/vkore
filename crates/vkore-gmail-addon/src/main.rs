use std::io::{Read, Write};
use vkore_addon_sdk::InvocationApproval;

fn main() {
    let mut arguments = std::env::args().skip(1);
    if arguments.next().as_deref() == Some("--manifest") {
        let Some(hash) = arguments.next() else {
            eprintln!("gmail add-on: --manifest requires a BLAKE3 package hash");
            std::process::exit(2);
        };
        match serde_json::to_writer_pretty(std::io::stdout(), &vkore_gmail_addon::manifest(hash)) {
            Ok(()) => return,
            Err(error) => {
                eprintln!("gmail add-on: {error}");
                std::process::exit(2);
            }
        }
    }
    if let Err(error) = run() {
        eprintln!("gmail add-on: {error}");
        std::process::exit(2);
    }
}

fn run() -> Result<(), String> {
    let mut input = Vec::new();
    std::io::stdin()
        .take(65_537)
        .read_to_end(&mut input)
        .map_err(|error| error.to_string())?;
    if input.len() > 65_536 {
        return Err("input too large".into());
    }
    let approval: InvocationApproval =
        serde_json::from_slice(&input).map_err(|error| error.to_string())?;
    let request = vkore_gmail_addon::request(&approval)?;
    let output = serde_json::to_vec(&request).map_err(|error| error.to_string())?;
    std::io::stdout()
        .write_all(&output)
        .map_err(|error| error.to_string())
}
