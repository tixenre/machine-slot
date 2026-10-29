//! The Claude Code hook (`machine-slot hook` and the script that finds it),
//! fed the JSON it gets from Claude Code.

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
    let mut p = Command::new(env!("CARGO_BIN_EXE_machine-slot"))
        .arg("hook")
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
        ("make -j8", "deny"),
        ("cd web && npm test", "deny"),
        ("go test ./...", "deny"),
        ("python3 -m pytest -q", "deny"),
        ("./gradlew assemble", "deny"),
        ("machine-slot shared \"web: tests\" -- npm test", "allow"),
        ("go test -bench . ./...", "deny"),
        ("machine-slot shared b -- go test -bench .", "deny"),
        ("machine-slot solo b -- go test -bench .", "allow"),
        ("ls make-targets", "allow"),
        ("cmake -S . -B out", "allow"),
        ("npm install", "allow"),
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
    // More heavy and timed commands from the config.
    let config = "hook_heavy = ./build.sh, just test\nhook_timed = ./bench.sh\n";
    assert_eq!(decide("./build.sh --release", config), "deny");
    assert_eq!(decide("just test", config), "deny");
    assert_eq!(decide("just fmt", config), "allow");
    assert_eq!(
        decide("machine-slot shared x -- ./bench.sh", config),
        "deny"
    );
    assert_eq!(decide("machine-slot solo x -- ./bench.sh", config), "allow");
    assert_eq!(decide("./build.sh", ""), "allow");
}

#[test]
fn the_installed_script_finds_the_binary() {
    let bin = std::path::Path::new(env!("CARGO_BIN_EXE_machine-slot"));
    let path = format!("{}:/usr/bin:/bin", bin.parent().unwrap().display());
    let run = |cmd: &str| {
        let mut p = Command::new("/bin/sh")
            .arg(HOOK)
            .env("PATH", &path)
            .env("HOME", std::env::temp_dir())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let input = format!("{{\"tool_input\":{{\"command\":{}}}}}", json_str(cmd));
        p.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
        String::from_utf8(p.wait_with_output().unwrap().stdout).unwrap()
    };
    assert!(run("cargo build").contains("\"deny\""));
    assert_eq!(run("ls"), "");
}

#[test]
fn the_scripts_are_executable() {
    // Claude Code runs the hook as a program: without the bit it fails
    // silently and nothing is refused.
    use std::os::unix::fs::PermissionsExt;
    for f in [
        "integrations/claude-code/hook.sh",
        "integrations/claude-code/install.sh",
        "macos/menubar/build.sh",
    ] {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(f);
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert!(mode & 0o111 != 0, "{f} isn't executable");
    }
}
