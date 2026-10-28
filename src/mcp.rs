//! `machine-slot mcp`: a read-only MCP server on stdio, so any session can
//! ask what holds the machine and how long a task usually takes. It never
//! runs commands: that stays with each session's own permissions.

use crate::json::Json;
use crate::{record, sched};
use std::io::{BufRead, Write};

fn tools() -> Json {
    let schema = |props: Json, required: &[&str]| {
        Json::obj([
            ("type", Json::str("object")),
            ("properties", props),
            (
                "required",
                Json::Arr(required.iter().map(|s| Json::str(*s)).collect()),
            ),
        ])
    };
    let prop =
        |t: &str, d: &str| Json::obj([("type", Json::str(t)), ("description", Json::str(d))]);
    let tool = |name: &str, description: &str, input: Json| {
        Json::obj([
            ("name", Json::str(name)),
            ("description", Json::str(description)),
            ("inputSchema", input),
        ])
    };
    Json::Arr(vec![
        tool(
            "status",
            "What holds this Mac's machine-slot now: running jobs (solo = alone, shared = together) with elapsed and usual time, and the queue.",
            schema(Json::obj([]), &[]),
        ),
        tool(
            "times",
            "How long each kind of task usually takes on this Mac (clean = ran alone on a quiet machine), with CPU and peak memory.",
            schema(Json::obj([]), &[]),
        ),
        tool(
            "history",
            "The last runs from the record (runs.tsv), newest last, optionally only those whose label contains `filter`.",
            schema(
                Json::obj([
                    ("limit", prop("number", "How many runs (default 20).")),
                    (
                        "filter",
                        prop("string", "Only labels containing this text."),
                    ),
                ]),
                &[],
            ),
        ),
        tool(
            "estimate",
            "How long a task would take and how long it would wait now, from its label (same label = same kind of task; commit ids and numbers are ignored).",
            schema(
                Json::obj([
                    (
                        "label",
                        prop("string", "The label you'd pass to machine-slot."),
                    ),
                    ("mode", prop("string", "solo or shared (default shared).")),
                ]),
                &["label"],
            ),
        ),
    ])
}

fn call(name: &str, args: &Json) -> Result<String, String> {
    let dir = crate::state_dir();
    match name {
        "status" => Ok(crate::status_text(&sched::peek(&dir))),
        "times" => Ok(record::times_table(&record::read(&crate::record_path()))),
        "history" => {
            let limit = args
                .get("limit")
                .and_then(Json::as_f64)
                .unwrap_or(20.0)
                .max(1.0) as usize;
            let filter = args.get("filter").and_then(Json::as_str).unwrap_or("");
            let runs = record::read(&crate::record_path());
            let picked: Vec<_> = runs.iter().filter(|r| r.label.contains(filter)).collect();
            let start = picked.len().saturating_sub(limit);
            let mut out = String::from(record::HEADER);
            for r in &picked[start..] {
                out.push('\n');
                out.push_str(&r.to_line());
            }
            Ok(out)
        }
        "estimate" => {
            let label = args
                .get("label")
                .and_then(Json::as_str)
                .ok_or("label is required")?;
            let mode = args.get("mode").and_then(Json::as_str).unwrap_or("shared");
            if mode != "solo" && mode != "shared" {
                return Err("mode is solo or shared".into());
            }
            Ok(crate::estimate_text(label, mode))
        }
        _ => Err(format!("unknown tool {name}")),
    }
}

pub fn serve() {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let Some(msg) = Json::parse(&line) else {
            let err = Json::obj([
                ("code", Json::Num(-32700.0)),
                ("message", Json::str("parse error")),
            ]);
            let _ = writeln!(
                out,
                "{}",
                Json::obj([
                    ("jsonrpc", Json::str("2.0")),
                    ("id", Json::Null),
                    ("error", err)
                ])
                .dump()
            );
            continue;
        };
        let Some(id) = msg.get("id").cloned() else {
            continue;
        }; // a notification
        let method = msg.get("method").and_then(Json::as_str).unwrap_or("");
        let params = msg.get("params").cloned().unwrap_or(Json::Null);
        let result: Result<Json, (f64, String)> = match method {
            "initialize" => Ok(Json::obj([
                (
                    "protocolVersion",
                    params
                        .get("protocolVersion")
                        .cloned()
                        .unwrap_or(Json::str("2025-06-18")),
                ),
                ("capabilities", Json::obj([("tools", Json::obj([]))])),
                (
                    "serverInfo",
                    Json::obj([
                        ("name", Json::str("machine-slot")),
                        ("version", Json::str(env!("CARGO_PKG_VERSION"))),
                    ]),
                ),
                (
                    "instructions",
                    Json::str(
                        "Read-only view of this machine's machine-slot queue. Run heavy commands through `machine-slot shared LABEL -- CMD` (builds, tests) or `machine-slot solo [--needs cpu,disk,ram,gpu,screen] LABEL -- CMD` (anything timed) in a shell; `machine-slot --help` has the rest.",
                    ),
                ),
            ])),
            "ping" => Ok(Json::obj([])),
            "tools/list" => Ok(Json::obj([("tools", tools())])),
            "tools/call" => {
                let name = params.get("name").and_then(Json::as_str).unwrap_or("");
                let args = params.get("arguments").cloned().unwrap_or(Json::obj([]));
                let (text, is_error) = match call(name, &args) {
                    Ok(t) => (t, false),
                    Err(e) => (e, true),
                };
                Ok(Json::obj([
                    (
                        "content",
                        Json::Arr(vec![Json::obj([
                            ("type", Json::str("text")),
                            ("text", Json::str(text)),
                        ])]),
                    ),
                    ("isError", Json::Bool(is_error)),
                ]))
            }
            _ => Err((-32601.0, format!("method not found: {method}"))),
        };
        let reply = match result {
            Ok(r) => Json::obj([("jsonrpc", Json::str("2.0")), ("id", id), ("result", r)]),
            Err((code, m)) => Json::obj([
                ("jsonrpc", Json::str("2.0")),
                ("id", id),
                (
                    "error",
                    Json::obj([("code", Json::Num(code)), ("message", Json::str(m))]),
                ),
            ]),
        };
        let _ = writeln!(out, "{}", reply.dump());
        let _ = out.flush();
    }
}
