//! Server-reported model speed, separate from queueing and guest execution.
use serde_json::Value;

pub struct CallTiming {
    pub role: String,
    pub input: String,
    pub output: String,
    pub cached: String,
}

fn phase(timing: &Value, count: &str, duration: &str, rate: &str) -> String {
    let tokens = timing[count].as_u64();
    let ms = timing[duration]
        .as_f64()
        .filter(|n| n.is_finite() && *n > 0.0);
    let speed = timing[rate]
        .as_f64()
        .filter(|n| n.is_finite() && *n > 0.0)
        .or_else(|| Some(tokens? as f64 * 1000.0 / ms?));
    match (tokens, ms, speed) {
        (Some(tokens), Some(ms), Some(speed)) if tokens > 0 => {
            format!("{speed:.1} tok/s · {tokens} tokens in {:.2} s", ms / 1000.0)
        }
        (Some(0), _, _) => "0 tokens · rate unavailable".into(),
        _ => "Unavailable".into(),
    }
}

pub fn calls(events: &[&Value]) -> Vec<CallTiming> {
    let mut measured: Vec<(String, String, Value, bool)> = Vec::new();
    for event in events {
        let payload = &event["payload"];
        let compact = payload["kind"] == "ModelTiming";
        let timing = if compact {
            payload["payload"]
                .as_str()
                .and_then(|s| serde_json::from_str::<Value>(s).ok())
        } else if payload["kind"] == "ReturnedModelFields" {
            // Older saved chats also carry final timings in the raw SSE fields.
            payload["payload"].as_str().and_then(|s| {
                s.lines()
                    .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                    .filter(|v| v["stop"] == true)
                    .filter_map(|v| v.get("timings").filter(|t| t.is_object()).cloned())
                    .last()
            })
        } else {
            None
        };
        let Some(timing) = timing.filter(|v| v.is_object()) else {
            continue;
        };
        let role = match payload["role"].as_str() {
            Some("Broker") => "Broker",
            Some("Agent") => "Worker",
            _ => continue,
        };
        let key = serde_json::json!([
            event["session_id"],
            event["prompt_id"],
            payload["role"],
            payload["route"],
            payload["generation"],
            payload["request"],
            payload["turn"]
        ])
        .to_string();
        if let Some(prior) = measured.iter_mut().find(|row| row.0 == key) {
            if compact || !prior.3 {
                prior.2 = timing;
                prior.3 = compact;
            }
        } else {
            measured.push((key, role.into(), timing, compact));
        }
    }
    let mut broker = 0;
    let mut worker = 0;
    measured
        .into_iter()
        .map(|(_, role, timing, _)| {
            let index = if role == "Broker" {
                broker += 1;
                broker
            } else {
                worker += 1;
                worker
            };
            CallTiming {
                role: format!("{role} · call {index}"),
                input: phase(&timing, "prompt_n", "prompt_ms", "prompt_per_second"),
                output: phase(
                    &timing,
                    "predicted_n",
                    "predicted_ms",
                    "predicted_per_second",
                ),
                cached: timing["cache_n"]
                    .as_u64()
                    .map(|n| format!("Reused input: {n} tokens"))
                    .unwrap_or_else(|| "Reused input: unavailable".into()),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn event(role: &str, turn: u64, kind: &str, payload: String) -> Value {
        json!({"session_id":"s","prompt_id":"p","payload":{
            "role":role,"turn":turn,"kind":kind,"payload":payload
        }})
    }

    #[test]
    fn final_rates_are_separate_deduplicated_and_keep_multiple_worker_calls() {
        let timing = json!({"prompt_n":1999,"prompt_ms":20833.997,"prompt_per_second":95.94894,
            "predicted_n":27,"predicted_ms":3156.977,"predicted_per_second":8.55248,"cache_n":0});
        let raw = event(
            "Broker",
            1,
            "ReturnedModelFields",
            format!(
                "{}\n{}\n",
                json!({"stop":false,"timings":{"prompt_n":1}}),
                json!({"stop":true,"timings":timing})
            ),
        );
        let compact = event("Broker", 1, "ModelTiming", timing.to_string());
        let first = event("Agent", 1, "ModelTiming", timing.to_string());
        let second = event("Agent", 2, "ModelTiming", timing.to_string());
        let result = calls(&[&raw, &compact, &first, &second, &raw]);
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].input, "95.9 tok/s · 1999 tokens in 20.83 s");
        assert_eq!(result[0].output, "8.6 tok/s · 27 tokens in 3.16 s");
        assert_eq!(result[0].cached, "Reused input: 0 tokens");
        assert_eq!(result[2].role, "Worker · call 2");
    }

    #[test]
    fn missing_partial_zero_and_invalid_measurements_do_not_invent_rates() {
        let partial = event(
            "Broker",
            1,
            "ReturnedModelFields",
            "{\"stop\":false,\"timings\":{\"predicted_per_second\":999}}\ntruncated".into(),
        );
        assert!(calls(&[&partial]).is_empty());
        assert_eq!(
            phase(&json!({"n":2,"ms":0}), "n", "ms", "rate"),
            "Unavailable"
        );
        assert_eq!(
            phase(&json!({"n":0,"ms":0}), "n", "ms", "rate"),
            "0 tokens · rate unavailable"
        );
        assert_eq!(
            phase(&json!({"n":10,"ms":2000,"rate":-1}), "n", "ms", "rate"),
            "5.0 tok/s · 10 tokens in 2.00 s"
        );
        let compact = event("Agent", 1, "ModelTiming", "{}".into());
        assert_eq!(calls(&[&compact])[0].input, "Unavailable");
    }
}
