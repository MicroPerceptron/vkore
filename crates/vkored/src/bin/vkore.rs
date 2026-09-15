use serde_json::{Value, json};
use std::fs::OpenOptions;
use std::io::{BufRead, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

fn main() {
    if let Err(error) = run() {
        eprintln!("[{}] vkore: {error}", unix_ms());
        std::process::exit(2);
    }
}

fn run() -> Result<(), String> {
    let mut server = "127.0.0.1:8787".to_owned();
    let mut project = "default".to_owned();
    let mut trace = false;
    let mut trace_file = None;
    let mut timeout = Duration::from_secs(900);
    let mut prompt_delay = Duration::ZERO;
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() != Some("chat") {
        return Err(usage());
    }
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--server" => server = args.next().ok_or("missing --server")?,
            "--project" => project = args.next().ok_or("missing --project")?,
            "--trace" => trace = true,
            "--trace-file" => {
                trace_file = Some(PathBuf::from(args.next().ok_or("missing --trace-file")?));
            }
            "--timeout-seconds" => {
                timeout = Duration::from_secs(
                    args.next()
                        .ok_or("missing --timeout-seconds")?
                        .parse()
                        .map_err(|_| "invalid --timeout-seconds")?,
                );
            }
            "--delay-between-prompts-seconds" => {
                prompt_delay = Duration::from_secs(
                    args.next()
                        .ok_or("missing --delay-between-prompts-seconds")?
                        .parse()
                        .map_err(|_| "invalid --delay-between-prompts-seconds")?,
                );
            }
            _ => return Err(format!("unknown argument {arg}")),
        }
    }
    let mut trace_output = trace_file
        .map(|path| {
            OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .open(path)
                .map_err(|error| error.to_string())
        })
        .transpose()?;
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
    eprintln!(
        "[{}] Connected session={session} project={project}; enter a prompt; Ctrl-Z/Ctrl-D exits.",
        unix_ms()
    );
    let mut completed_prompts = 0usize;
    let mut cursor = 0u64;
    let mut had_prompt_failure = false;
    for (index, line) in std::io::stdin().lock().lines().enumerate() {
        let text = line.map_err(|error| error.to_string())?;
        if text.trim().is_empty() {
            continue;
        }
        if let Err(error) = validate_prompt_text(&text) {
            eprintln!("[{}] Prompt rejected: {error}. The session is still open.", unix_ms());
            had_prompt_failure = true;
            continue;
        }
        if completed_prompts != 0 {
            std::thread::sleep(prompt_delay);
        }
        let prompt = format!("prompt-{}", index + 1);
        let started = Instant::now();
        if let Err(error) = request(&server, "POST", &format!("/api/v1/sessions/{session}/prompts"), &json!({
            "api_version":1,"prompt_id":prompt,"idempotency_key":format!("cli-{}",index + 1),"text":text
        }).to_string()) {
            eprintln!("[{}] Prompt {prompt} was not admitted: {error}", unix_ms());
            had_prompt_failure = true;
            continue;
        }
        loop {
            if started.elapsed() >= timeout {
                let _ = request(&server, "DELETE", &format!("/api/v1/sessions/{session}"), "");
                return Err(format!(
                    "prompt {prompt} timed out after {} ms",
                    started.elapsed().as_millis()
                ));
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
            let body = request(
                &server,
                "GET",
                &format!("/api/v1/sessions/{session}/events?after={cursor}"),
                "",
            )?;
            let mut terminal = None;
            for line in body.lines().filter_map(|line| line.strip_prefix("data: ")) {
                let mut event: Value = serde_json::from_str(line).map_err(|error| error.to_string())?;
                event["client_elapsed_ms"] = json!(started.elapsed().as_secs_f64() * 1000.0);
                event["client_received_unix_ms"] = json!(unix_ms());
                if let Some(output) = trace_output.as_mut() {
                    writeln!(output, "{event}").map_err(|error| error.to_string())?;
                    output.flush().map_err(|error| error.to_string())?;
                }
                cursor = cursor.max(event["sequence"].as_u64().unwrap_or(0));
                if trace {
                    let payload = event["payload"].to_string();
                    eprintln!(
                        "[event_unix_ms={} elapsed_ms={} sequence={} kind={}] {}",
                        event["timestamp_unix_ms"].as_u64().unwrap_or(0),
                        started.elapsed().as_millis(),
                        event["sequence"].as_u64().unwrap_or(0),
                        event["kind"].as_str().unwrap_or("unknown"),
                        preview(&payload, 512)
                    );
                }
                if event["kind"] == "assistant" {
                    println!(
                        "[{}] {}",
                        event["timestamp_unix_ms"].as_u64().unwrap_or_else(unix_ms),
                        event["payload"]["text"].as_str().unwrap_or("")
                    );
                }
                if event["kind"] == "prompt_terminal" && event["prompt_id"] == prompt {
                    terminal = Some(terminal_result(&event["payload"]));
                }
            }
            if let Some(result) = terminal {
                if let Err(error) = result {
                    eprintln!("[{}] Prompt {prompt}: {error}. The session is still open.", unix_ms());
                    had_prompt_failure = true;
                }
                completed_prompts += 1;
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
    if had_prompt_failure { Err("one or more prompts failed or were rejected; see the transcript".into()) } else { Ok(()) }
}

fn validate_prompt_text(text: &str) -> Result<(), String> {
    if text.len() > kore_hostlink::TASK_INTENT_MAX {
        return Err(format!("Kore accepts at most {} UTF-8 bytes per prompt (received {})", kore_hostlink::TASK_INTENT_MAX, text.len()));
    }
    if text.as_bytes().contains(&0) { return Err("prompts cannot contain NUL bytes".into()); }
    Ok(())
}

fn usage() -> String {
    "usage: vkore chat [--server HOST:PORT] [--project ID] [--trace] [--trace-file PATH] [--timeout-seconds N] [--delay-between-prompts-seconds N]".into()
}

fn terminal_result(payload: &Value) -> Result<(), String> {
    match payload["status"].as_str() {
        Some("completed") => Ok(()),
        Some(status) => Err(format!("terminal status {status}")),
        None => Err("terminal event has no status".into()),
    }
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn preview(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_owned();
    }
    let boundary = value
        .char_indices()
        .map(|(index, _)| index)
        .take_while(|index| *index <= max_bytes)
        .last()
        .unwrap_or(0);
    format!(
        "{}… [full payload: {} bytes]",
        &value[..boundary],
        value.len()
    )
}

fn request(server: &str, method: &str, path: &str, body: &str) -> Result<String, String> {
    let mut stream = TcpStream::connect(server).map_err(|error| error.to_string())?;
    stream.set_read_timeout(Some(Duration::from_secs(10))).map_err(|error| error.to_string())?;
    stream.set_write_timeout(Some(Duration::from_secs(10))).map_err(|error| error.to_string())?;
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
    decode_http_body(headers, body)
}

fn decode_http_body(headers: &str, body: &str) -> Result<String, String> {
    if !headers.lines().any(|line| {
        line.split_once(':').is_some_and(|(name, value)| {
            name.eq_ignore_ascii_case("transfer-encoding")
                && value
                    .split(',')
                    .any(|encoding| encoding.trim().eq_ignore_ascii_case("chunked"))
        })
    }) {
        return Ok(body.to_owned());
    }
    let bytes = body.as_bytes();
    let mut decoded = Vec::new();
    let mut cursor = 0usize;
    loop {
        let line_end = bytes[cursor..]
            .windows(2)
            .position(|pair| pair == b"\r\n")
            .map(|offset| cursor + offset)
            .ok_or("malformed chunked HTTP response")?;
        let size_text = std::str::from_utf8(&bytes[cursor..line_end])
            .map_err(|_| "non-UTF-8 HTTP chunk size")?
            .split(';')
            .next()
            .unwrap_or("");
        let size =
            usize::from_str_radix(size_text.trim(), 16).map_err(|_| "invalid HTTP chunk size")?;
        cursor = line_end + 2;
        if size == 0 {
            break;
        }
        let end = cursor
            .checked_add(size)
            .filter(|end| end.saturating_add(2) <= bytes.len())
            .ok_or("truncated chunked HTTP response")?;
        decoded.extend_from_slice(&bytes[cursor..end]);
        if &bytes[end..end + 2] != b"\r\n" {
            return Err("malformed HTTP chunk terminator".into());
        }
        cursor = end + 2;
    }
    String::from_utf8(decoded).map_err(|_| "non-UTF-8 HTTP response body".into())
}

#[cfg(test)]
mod tests {
    use super::{decode_http_body, preview, terminal_result};

    #[test]
    fn prompt_bound_counts_utf8_bytes_without_truncation() {
        assert!(super::validate_prompt_text(&"a".repeat(128)).is_ok());
        assert!(super::validate_prompt_text(&"a".repeat(129)).unwrap_err().contains("129"));
        assert!(super::validate_prompt_text(&"💡".repeat(33)).unwrap_err().contains("132"));
        assert!(super::validate_prompt_text("a\0b").is_err());
    }

    #[test]
    fn only_successful_terminal_events_pass_diagnostics() {
        use serde_json::json;

        assert!(terminal_result(&json!({"status": "completed"})).is_ok());
        for status in ["failed", "cancelled", "running", "unknown"] {
            assert!(terminal_result(&json!({"status": status})).is_err());
        }
        assert!(terminal_result(&json!({})).is_err());
    }

    #[test]
    fn decodes_chunked_sse_records_larger_than_one_transport_chunk() {
        let headers = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked";
        let body = "5\r\ndata:\r\n8\r\n large\n\n\r\n0\r\n\r\n";
        assert_eq!(decode_http_body(headers, body).unwrap(), "data: large\n\n");
    }

    #[test]
    fn terminal_preview_is_bounded_without_splitting_utf8() {
        assert_eq!(preview("hello", 5), "hello");
        assert_eq!(preview("abc💡def", 5), "abc… [full payload: 10 bytes]");
    }
}
