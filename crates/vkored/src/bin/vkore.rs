use serde_json::{Value, json};
use std::io::{BufRead, Read, Write};
use std::net::TcpStream;

fn main() {
    if let Err(error) = run() {
        eprintln!("vkore: {error}");
        std::process::exit(2);
    }
}

fn run() -> Result<(), String> {
    let mut server = "127.0.0.1:8787".to_owned();
    let mut project = "default".to_owned();
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() != Some("chat") {
        return Err("usage: vkore chat [--server HOST:PORT] [--project ID]".into());
    }
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--server" => server = args.next().ok_or("missing --server")?,
            "--project" => project = args.next().ok_or("missing --project")?,
            _ => return Err(format!("unknown argument {arg}")),
        }
    }
    let session = format!("cli-{}", std::process::id());
    request(
        &server,
        "POST",
        "/api/v1/sessions",
        &json!({
            "api_version":1,"project_id":project,"session_id":session
        })
        .to_string(),
    )?;
    eprintln!("Connected. Enter a prompt; Ctrl-Z/Ctrl-D exits.");
    for (index, line) in std::io::stdin().lock().lines().enumerate() {
        let text = line.map_err(|error| error.to_string())?;
        if text.trim().is_empty() {
            continue;
        }
        let prompt = format!("prompt-{}", index + 1);
        request(&server, "POST", &format!("/api/v1/sessions/{session}/prompts"), &json!({
            "api_version":1,"prompt_id":prompt,"idempotency_key":format!("cli-{}",index + 1),"text":text
        }).to_string())?;
        let mut cursor = 0u64;
        loop {
            std::thread::sleep(std::time::Duration::from_millis(25));
            let body = request(
                &server,
                "GET",
                &format!("/api/v1/sessions/{session}/events?after={cursor}"),
                "",
            )?;
            let mut terminal = false;
            for line in body.lines().filter_map(|line| line.strip_prefix("data: ")) {
                let event: Value = serde_json::from_str(line).map_err(|error| error.to_string())?;
                cursor = cursor.max(event["sequence"].as_u64().unwrap_or(0));
                if event["kind"] == "assistant" {
                    println!("{}", event["payload"]["text"].as_str().unwrap_or(""));
                }
                if event["kind"] == "prompt_terminal" && event["prompt_id"] == prompt {
                    terminal = true;
                }
            }
            if terminal {
                break;
            }
        }
    }
    let _ = request(
        &server,
        "DELETE",
        &format!("/api/v1/sessions/{session}"),
        "",
    );
    Ok(())
}

fn request(server: &str, method: &str, path: &str, body: &str) -> Result<String, String> {
    let mut stream = TcpStream::connect(server).map_err(|error| error.to_string())?;
    write!(stream, "{method} {path} HTTP/1.1\r\nHost: {server}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).map_err(|error| error.to_string())?;
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .map_err(|error| error.to_string())?;
    let (headers, body) = response
        .split_once("\r\n\r\n")
        .ok_or("malformed HTTP response")?;
    if !headers.starts_with("HTTP/1.1 200") {
        return Err(body.into());
    }
    Ok(body.into())
}
