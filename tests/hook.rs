//! The Claude Code hook, fed the JSON it gets from Claude Code.

use std::io::Write;
use std::process::{Command, Stdio};

const HOOK: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/integrations/claude-code/hook.sh"
);

/// "deny" or "allow", for a Bash command, with a private config.
fn decide(cmd: &str, config: &str) -> String {
    let dir = std::env::temp_dir().join(format!("machine-slot-hook-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("machine-slot")).unwrap();
    std::fs::write(dir.join("machine-slot/config"), config).unwrap();
    let input = format!("{{\"tool_input\":{{\"command\":{}}}}}", json_str(cmd));
    let mut p = Command::new("bash")
        .arg(HOOK)
        .env("XDG_CONFIG_HOME", &dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    p.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
    let out = String::from_utf8(p.wait_with_output().unwrap().stdout).unwrap();
    if out.contains("\"deny\"") {
        "deny".into()
    } else {
        "allow".into()
    }
}

fn json_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

#[test]
fn heavy_commands_need_a_turn() {
    if Command::new("jq").arg("--version").output().is_err() {
        eprintln!("no jq: skipped");
        return;
    }
    let cases = [
        ("cargo test -p store", "deny"),
        ("cd x && cargo build --release", "deny"),
        ("cargo +nightly test", "deny"),
        ("xcodebuild -scheme App test", "deny"),
        ("swift build", "deny"),
        (
            "machine-slot shared \"app: tests\" -- cargo test -p store",
            "allow",
        ),
        ("cargo bench -p engine", "deny"),
        ("machine-slot shared b -- cargo bench", "deny"),
        (
            "machine-slot solo \"bench\" -- cargo bench -p engine",
            "allow",
        ),
        (
            "machine-slot solo --needs cpu,gpu \"bench\" -- cargo bench",
            "allow",
        ),
        ("hyperfine 'sleep 1'", "deny"),
        ("cargo test --release -- --bench foo", "deny"),
        ("echo \"run cargo test later\"", "allow"),
        ("git commit -m 'fix: cargo test flake'", "allow"),
        ("cargo fmt", "allow"),
        ("ls cargo-tests", "allow"),
        ("mybuild --ui", "allow"),
    ];
    for (cmd, want) in cases {
        assert_eq!(decide(cmd, ""), want, "{cmd}");
    }
    // A tool listed in hook_allow takes its own turns.
    assert_eq!(
        decide("mybuild && cargo build", "hook_allow = mybuild\n"),
        "allow"
    );
    assert_eq!(decide("cargo build", "hook_allow = mybuild\n"), "deny");
}
