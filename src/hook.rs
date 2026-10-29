//! `machine-slot hook`: the Claude Code PreToolUse(Bash) check. A bare
//! build, test or bench is refused with the line to use instead, and
//! measurements must be `solo`. Text inside quotes (commit messages, echo)
//! is ignored.
//!
//! The lists grow from the config, each a comma-separated list of commands:
//! `hook_heavy` (more heavy commands), `hook_timed` (more commands that
//! measure) and `hook_allow` (tools that take their own turns).

use crate::json::Json;
use crate::paths;

const HEAVY: &[&str] = &[
    "cargo test",
    "cargo bench",
    "cargo build",
    "cargo run",
    "cargo check",
    "cargo clippy",
    "cargo nextest",
    "cargo criterion",
    "xcodebuild",
    "swift build",
    "swift test",
    "hyperfine",
    "make",
    "gmake",
    "ninja",
    "cmake --build",
    "meson compile",
    "meson test",
    "go build",
    "go test",
    "zig build",
    "bazel build",
    "bazel test",
    "bazelisk build",
    "bazelisk test",
    "gradle",
    "gradlew",
    "./gradlew",
    "mvn",
    "./mvnw",
    "dotnet build",
    "dotnet test",
    "npm test",
    "npm run build",
    "npm run test",
    "pnpm test",
    "pnpm build",
    "yarn test",
    "yarn build",
    "pytest",
    "python -m pytest",
    "python3 -m pytest",
    "tox",
    "nox",
];

const TIMED: &[&str] = &[
    "cargo bench",
    "cargo criterion",
    "hyperfine",
    "pytest --benchmark-only",
];

/// The command split into simple commands, each a list of words, with
/// quoted text dropped and `cargo +toolchain` read as `cargo`.
/// The command without the bodies of its here-documents: what `cat <<EOF`
/// feeds a file is text, not commands.
fn strip_heredocs(cmd: &str) -> String {
    let mut out = String::new();
    let mut pending: Vec<String> = Vec::new();
    for line in cmd.lines() {
        if let Some(end) = pending.first() {
            if line.trim() == end {
                pending.remove(0);
            }
            continue;
        }
        out.push_str(line);
        out.push('\n');
        let mut rest = line;
        while let Some(i) = rest.find("<<") {
            let after = &rest[i + 2..];
            if let Some(here_string) = after.strip_prefix('<') {
                rest = here_string;
                continue;
            }
            let after = after.strip_prefix('-').unwrap_or(after).trim_start();
            let word: String = after
                .trim_start_matches(['\'', '"', '\\'])
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if !word.is_empty() {
                pending.push(word);
            }
            rest = after;
        }
    }
    out
}

fn simple_commands(cmd: &str) -> Vec<Vec<String>> {
    let cmd = &strip_heredocs(cmd);
    let mut out = vec![Vec::new()];
    let mut word = String::new();
    let mut quote: Option<char> = None;
    let flush = |word: &mut String, out: &mut Vec<Vec<String>>| {
        if !word.is_empty() {
            out.last_mut().unwrap().push(std::mem::take(word));
        }
    };
    for c in cmd.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None => match c {
                '"' | '\'' => quote = Some(c),
                ';' | '&' | '|' | '(' | ')' | '`' | '\n' | '{' | '}' => {
                    flush(&mut word, &mut out);
                    out.push(Vec::new());
                }
                c if c.is_whitespace() => flush(&mut word, &mut out),
                c => word.push(c),
            },
        }
    }
    flush(&mut word, &mut out);
    for words in &mut out {
        let mut i = 0;
        while i + 1 < words.len() {
            if words[i] == "cargo" && words[i + 1].starts_with('+') {
                words.remove(i + 1);
            }
            i += 1;
        }
    }
    out.retain(|w| !w.is_empty());
    out
}

/// Whether any simple command has the entry's words in a row.
fn has(cmds: &[Vec<String>], entry: &str) -> bool {
    let e: Vec<&str> = entry.split_whitespace().collect();
    !e.is_empty()
        && cmds.iter().any(|w| {
            w.windows(e.len())
                .any(|win| win.iter().zip(&e).all(|(a, b)| a == b))
        })
}

fn any(cmds: &[Vec<String>], entries: &[String]) -> bool {
    entries.iter().any(|e| has(cmds, e))
}

fn list(text: Option<String>) -> Vec<String> {
    text.unwrap_or_default()
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// The turn the command needs and doesn't take, if any.
#[derive(Debug, PartialEq)]
pub enum Verdict {
    Allow,
    NeedsShared,
    NeedsSolo,
}

pub fn judge(cmd: &str, config: &dyn Fn(&str) -> Option<String>) -> Verdict {
    let cmds = simple_commands(cmd);
    if any(&cmds, &list(config("hook_allow"))) {
        return Verdict::Allow;
    }
    let more_timed = list(config("hook_timed"));
    let mut heavy: Vec<String> = HEAVY.iter().map(|s| s.to_string()).collect();
    heavy.extend(list(config("hook_heavy")));
    heavy.extend(more_timed.iter().cloned());
    if !any(&cmds, &heavy) {
        return Verdict::Allow;
    }
    if has(&cmds, "machine-slot solo") {
        return Verdict::Allow;
    }
    let mut timed: Vec<String> = TIMED.iter().map(|s| s.to_string()).collect();
    timed.extend(more_timed);
    let bench_flag = cmds.iter().flatten().any(|w| {
        w == "--bench" || w.starts_with("--bench=") || w == "-bench" || w.starts_with("-bench=")
    });
    if any(&cmds, &timed) || bench_flag {
        return Verdict::NeedsSolo;
    }
    if has(&cmds, "machine-slot shared") {
        return Verdict::Allow;
    }
    Verdict::NeedsShared
}

const SOLO: &str = "Measurements run alone on this machine: `machine-slot solo [--needs cpu,disk,ram,gpu,screen] \"<project: what you measure>\" -- <command>` (default cpu,disk,ram; add gpu for GPU work, screen for UI tests). It waits for its turn and a quiet machine, so run it in the background. Use the same label for the same task. A solo stops at 3 × its usual time (10 min at least, 30 min on a first run): for anything longer, or a bench that changed, say how long it takes with `--expect 25m`. See `machine-slot --help`.";
const SHARED: &str = "Heavy work goes through the machine queue: `machine-slot shared \"<project: task>\" -- <command>` (runs alongside other work, never during a measurement). To measure time or resources use `machine-slot solo [--needs cpu,disk,ram,gpu,screen] ...`. Same label for the same task; run it in the background if it may wait. `machine-slot status` / `machine-slot estimate LABEL` show the queue.";

/// Reads the hook's JSON on stdin; prints a deny decision when needed.
pub fn main() -> i32 {
    let mut input = String::new();
    let _ = std::io::Read::read_to_string(&mut std::io::stdin(), &mut input);
    let cmd = Json::parse(&input)
        .and_then(|j| {
            j.get("tool_input")?
                .get("command")?
                .as_str()
                .map(String::from)
        })
        .unwrap_or_default();
    let reason = match judge(&cmd, &paths::config) {
        Verdict::Allow => return 0,
        Verdict::NeedsSolo => SOLO,
        Verdict::NeedsShared => SHARED,
    };
    let out = Json::obj([(
        "hookSpecificOutput",
        Json::obj([
            ("hookEventName", Json::str("PreToolUse")),
            ("permissionDecision", Json::str("deny")),
            ("permissionDecisionReason", Json::str(reason)),
        ]),
    )]);
    println!("{}", out.dump());
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn splits_like_a_shell_and_drops_quoted_text() {
        let c = simple_commands(
            "cd x && cargo +nightly test -p a; echo \"cargo test\" | git commit -m 'make it'",
        );
        assert_eq!(
            c,
            vec![
                vec!["cd", "x"],
                vec!["cargo", "test", "-p", "a"],
                vec!["echo"],
                vec!["git", "commit", "-m"],
            ]
        );
    }

    #[test]
    fn heredoc_bodies_are_text() {
        let cmd = "cat > notes.md <<'EOF'\nrun cargo test and make\nEOF\ncargo fmt";
        assert_eq!(judge(cmd, &none), Verdict::Allow);
        let cmd = "python3 - <<EOF\nmake\nEOF\ncargo build";
        assert_eq!(judge(cmd, &none), Verdict::NeedsShared);
        assert_eq!(judge("cat <<< 'x' && make", &none), Verdict::NeedsShared);
    }

    #[test]
    fn verdicts() {
        use Verdict::*;
        assert_eq!(judge("cargo test", &none), NeedsShared);
        assert_eq!(judge("time cargo build --release", &none), NeedsShared);
        assert_eq!(judge("cargo bench", &none), NeedsSolo);
        assert_eq!(judge("ls cargo-tests", &none), Allow);
        assert_eq!(judge("machine-slot shared x -- cargo test", &none), Allow);
        assert_eq!(
            judge("machine-slot shared x -- cargo bench", &none),
            NeedsSolo
        );
        assert_eq!(judge("machine-slot solo x -- cargo bench", &none), Allow);
    }
}
