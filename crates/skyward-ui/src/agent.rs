//! OpenAI-compatible chat in recovery: the user names an API, the model
//! reads [`PROMPT`], and it may list the phone, look at folders, or run a
//! shell command.

use serde_json::{json, Value};
use std::fs;
use std::io::Read;
use std::net::Ipv4Addr;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use std::thread;

/// What the model is told about this recovery. Bundled with the binary.
pub const PROMPT: &str = include_str!("../agent.md");

const MAX_ROUNDS: usize = 8;
const MAX_OUT: usize = 32 * 1024;
const RUN_SECS: u64 = 60;

/// One line on the chat page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Line {
    User(String),
    Assistant(String),
    Tool { name: String, detail: String, result: String },
}

/// How a turn stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Phase {
    Working,
    Failed(String),
    Done,
}

/// A turn talking to the API, on its own thread.
pub struct Job {
    phase: Mutex<Phase>,
    extra: Mutex<Vec<Line>>,
    seq: AtomicU64,
}

impl Job {
    pub fn start(url: String, model: String, key: String, history: Vec<Line>) -> Arc<Self> {
        let job = Arc::new(Job {
            phase: Mutex::new(Phase::Working),
            extra: Mutex::new(Vec::new()),
            seq: AtomicU64::new(0),
        });
        let running = job.clone();
        thread::Builder::new()
            .name("agent".into())
            .spawn(move || {
                match turn(&url, &model, &key, &history, &running) {
                    Ok(()) => running.set(Phase::Done),
                    Err(why) => running.set(Phase::Failed(why)),
                }
            })
            .expect("agent thread");
        job
    }

    pub fn phase(&self) -> Phase {
        self.phase.lock().expect("agent phase").clone()
    }

    pub fn generation(&self) -> u64 {
        self.seq.load(Ordering::Relaxed)
    }

    /// Lines this turn added after the user's message.
    pub fn extra(&self) -> Vec<Line> {
        self.extra.lock().expect("agent extra").clone()
    }

    fn set(&self, phase: Phase) {
        *self.phase.lock().expect("agent phase") = phase;
        self.seq.fetch_add(1, Ordering::Relaxed);
    }

    fn push(&self, line: Line) {
        self.extra.lock().expect("agent extra").push(line);
        self.seq.fetch_add(1, Ordering::Relaxed);
    }
}

/// `base` as `.../v1/chat/completions`.
pub fn completions_url(base: &str) -> Option<String> {
    let base = base.trim().trim_end_matches('/');
    if base.is_empty() {
        return None;
    }
    Some(if base.ends_with("/chat/completions") {
        base.to_string()
    } else if base.ends_with("/v1") {
        format!("{base}/chat/completions")
    } else {
        format!("{base}/v1/chat/completions")
    })
}

fn turn(url: &str, model: &str, key: &str, history: &[Line], job: &Job) -> Result<(), String> {
    let endpoint = completions_url(url).ok_or_else(|| "fill in the API address".to_string())?;
    if model.trim().is_empty() {
        return Err("fill in the model name".into());
    }
    let agent = skyward_net::http::agent(std::sync::Arc::new(Mutex::new(vec![
        Ipv4Addr::new(8, 8, 8, 8),
        Ipv4Addr::new(1, 1, 1, 1),
    ])));
    let mut messages = api_messages(history);
    for _ in 0..MAX_ROUNDS {
        let body = json!({
            "model": model.trim(),
            "messages": messages,
            "tools": tools(),
            "tool_choice": "auto",
        });
        let reply = post(&agent, &endpoint, key, &body)?;
        let (assistant, calls) = parse_reply(&reply)?;
        if calls.is_empty() {
            if !assistant.is_empty() {
                job.push(Line::Assistant(assistant));
            }
            return Ok(());
        }
        messages.push(json!({
            "role": "assistant",
            "content": if assistant.is_empty() { Value::Null } else { Value::String(assistant.clone()) },
            "tool_calls": calls.iter().map(|c| json!({
                "id": c.id,
                "type": "function",
                "function": { "name": c.name, "arguments": c.arguments },
            })).collect::<Vec<_>>(),
        }));
        for call in &calls {
            let result = run_tool(&call.name, &call.arguments);
            job.push(Line::Tool {
                name: call.name.clone(),
                detail: call.arguments.clone(),
                result: result.clone(),
            });
            messages.push(json!({
                "role": "tool",
                "tool_call_id": call.id,
                "content": result,
            }));
        }
    }
    Err("the model kept calling tools".into())
}

fn post(agent: &ureq::Agent, url: &str, key: &str, body: &Value) -> Result<String, String> {
    let mut req = agent
        .post(url)
        .header("Content-Type", "application/json")
        .header("User-Agent", "AuroraLynx");
    if !key.trim().is_empty() {
        req = req.header("Authorization", format!("Bearer {}", key.trim()));
    }
    let mut response = req.send(body.to_string()).map_err(|e| e.to_string())?;
    let status = response.status();
    let text = response.body_mut().read_to_string().map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("{status}: {}", clip(&text, 512)));
    }
    Ok(text)
}

struct Call {
    id: String,
    name: String,
    arguments: String,
}

fn parse_reply(body: &str) -> Result<(String, Vec<Call>), String> {
    let v: Value = serde_json::from_str(body).map_err(|e| format!("the API answer is not JSON: {e}"))?;
    if let Some(err) = v.pointer("/error/message").and_then(Value::as_str) {
        return Err(err.to_string());
    }
    let msg = v
        .pointer("/choices/0/message")
        .ok_or_else(|| "the API named no message".to_string())?;
    let content = msg.get("content").and_then(Value::as_str).unwrap_or("").trim().to_string();
    let mut calls = Vec::new();
    if let Some(list) = msg.get("tool_calls").and_then(Value::as_array) {
        for (i, item) in list.iter().enumerate() {
            let id = item
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| format!("call{i}"));
            let name = item
                .pointer("/function/name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let arguments = item
                .pointer("/function/arguments")
                .and_then(Value::as_str)
                .unwrap_or("{}")
                .to_string();
            if !name.is_empty() {
                calls.push(Call { id, name, arguments });
            }
        }
    }
    Ok((content, calls))
}

fn api_messages(history: &[Line]) -> Vec<Value> {
    let mut out = vec![json!({"role": "system", "content": PROMPT})];
    for line in history {
        match line {
            Line::User(text) => out.push(json!({"role": "user", "content": text})),
            Line::Assistant(text) => out.push(json!({"role": "assistant", "content": text})),
            Line::Tool { name, detail, result } => {
                out.push(json!({"role": "user", "content": format!("[{name}] {detail}\n{result}")}));
            }
        }
    }
    out
}

fn tools() -> Value {
    json!([
        {
            "type": "function",
            "function": {
                "name": "device_status",
                "description": "Battery, slot, kernel, and mounts on this recovery.",
                "parameters": { "type": "object", "properties": {} }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "list_dir",
                "description": "List one directory: names, types, sizes.",
                "parameters": {
                    "type": "object",
                    "properties": { "path": { "type": "string" } },
                    "required": ["path"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "run",
                "description": "Run any shell command with sh -c.",
                "parameters": {
                    "type": "object",
                    "properties": { "command": { "type": "string" } },
                    "required": ["command"]
                }
            }
        }
    ])
}

fn run_tool(name: &str, arguments: &str) -> String {
    let args: Value = serde_json::from_str(arguments).unwrap_or(json!({}));
    match name {
        "device_status" => device_status(),
        "list_dir" => list_dir(args.get("path").and_then(Value::as_str).unwrap_or("/")),
        "run" => run(args.get("command").and_then(Value::as_str).unwrap_or("")),
        other => format!("unknown tool {other}"),
    }
}

fn device_status() -> String {
    let mut out = String::from("AuroraLynx recovery\n");
    if let Ok(rel) = fs::read_to_string("/proc/version") {
        out.push_str(rel.trim());
        out.push('\n');
    }
    if let Ok(Some(slot)) = skyward_vab::current_slot() {
        out.push_str(&format!("slot: {}\n", slot.letter()));
    }
    if let Ok(cmdline) = fs::read_to_string("/proc/cmdline") {
        out.push_str("cmdline: ");
        out.push_str(cmdline.trim());
        out.push('\n');
    }
    for name in ["battery", "Battery", "bms"] {
        let cap = Path::new("/sys/class/power_supply").join(name).join("capacity");
        if let Ok(level) = fs::read_to_string(cap) {
            out.push_str(&format!("battery: {}%\n", level.trim()));
            break;
        }
    }
    if let Ok(mounts) = fs::read_to_string("/proc/mounts") {
        out.push_str("mounts:\n");
        out.push_str(&clip(&mounts, 4096));
    }
    out
}

fn list_dir(path: &str) -> String {
    let path = if path.is_empty() { "/" } else { path };
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(e) => return format!("{path}: {e}"),
    };
    let mut rows = Vec::new();
    for entry in entries.flatten().take(200) {
        let name = entry.file_name().to_string_lossy().into_owned();
        let meta = entry.metadata();
        let (kind, size) = match &meta {
            Ok(m) if m.is_dir() => ("dir", 0),
            Ok(m) => ("file", m.len()),
            Err(_) => ("?", 0),
        };
        let mode = meta.ok().map(|m| m.mode()).unwrap_or(0);
        rows.push(format!("{kind}\t{size}\t{mode:o}\t{name}"));
    }
    rows.sort();
    if rows.is_empty() {
        format!("{path} is empty")
    } else {
        format!("{path}\n{}", rows.join("\n"))
    }
}

fn run(command: &str) -> String {
    let command = command.trim();
    if command.is_empty() {
        return "empty command".into();
    }
    let sh = if cfg!(target_os = "android") {
        "/system/bin/sh"
    } else {
        "sh"
    };
    let mut child = match Command::new(sh)
        .arg("-c")
        .arg(command)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(e) => return e.to_string(),
    };
    let mut stdout_pipe = child.stdout.take();
    let mut stderr_pipe = child.stderr.take();
    let start = Instant::now();
    let limit = Duration::from_secs(RUN_SECS);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut stdout = String::new();
                let mut stderr = String::new();
                if let Some(mut out) = stdout_pipe.take() {
                    let _ = out.read_to_string(&mut stdout);
                }
                if let Some(mut err) = stderr_pipe.take() {
                    let _ = err.read_to_string(&mut stderr);
                }
                let mut text = String::new();
                if !stdout.is_empty() {
                    text.push_str(&stdout);
                }
                if !stderr.is_empty() {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(&stderr);
                }
                if !status.success() {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(&status.to_string());
                }
                if text.is_empty() {
                    text = status.to_string();
                }
                return clip(&text, MAX_OUT);
            }
            Ok(None) if start.elapsed() < limit => thread::sleep(Duration::from_millis(50)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return format!("timed out after {RUN_SECS}s");
            }
            Err(e) => return e.to_string(),
        }
    }
}

fn clip(text: &str, max: usize) -> String {
    if text.len() <= max {
        text.to_string()
    } else {
        let end = text.floor_char_boundary(max);
        format!("{}\n… truncated", &text[..end])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completions_url_fills_v1() {
        assert_eq!(completions_url("  "), None);
        assert_eq!(
            completions_url("https://api.openai.com"),
            Some("https://api.openai.com/v1/chat/completions".into())
        );
        assert_eq!(
            completions_url("https://example.com/v1/"),
            Some("https://example.com/v1/chat/completions".into())
        );
        assert_eq!(
            completions_url("http://127.0.0.1:8080/v1/chat/completions"),
            Some("http://127.0.0.1:8080/v1/chat/completions".into())
        );
    }

    #[test]
    fn parse_text_and_tools() {
        let (text, calls) = parse_reply(
            r#"{"choices":[{"message":{"content":"hi","tool_calls":[{"id":"1","function":{"name":"run","arguments":"{\"command\":\"ls\"}"}}]}}]}"#,
        )
        .unwrap();
        assert_eq!(text, "hi");
        assert_eq!(calls[0].name, "run");
        assert!(calls[0].arguments.contains("ls"));
    }

    #[test]
    fn list_and_run_work_on_the_host() {
        let dir = std::env::temp_dir();
        let listed = list_dir(dir.to_str().unwrap());
        assert!(listed.starts_with(dir.to_str().unwrap()), "{listed}");
        assert_eq!(clip(&run("echo agent-ok"), 32).trim(), "agent-ok");
    }

    #[test]
    fn prompt_names_this_recovery() {
        assert!(PROMPT.contains("AuroraLynx Recovery"));
        assert!(PROMPT.contains("run"));
    }
}
