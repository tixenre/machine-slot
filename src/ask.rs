//! `solo --ask`: when the machine isn't quiet, ask the user whether the
//! busy apps may be quit for the run, and reopen them afterwards. Apps quit
//! the normal way (they save, and ask about unsaved work themselves);
//! nothing is ever force-quit. The list of what was closed is kept in the
//! state folder until it's reopened, so `machine-slot reopen` can finish the
//! job if machine-slot itself was killed.

use crate::sys;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq)]
pub struct App {
    pub name: String,
    pub bundle_id: String,
    pub pid: i32,
}

/// Regular (Dock) apps, from `lsappinfo list`.
fn parse_lsappinfo(text: &str) -> Vec<App> {
    let mut apps = Vec::new();
    let (mut name, mut bundle, mut pid, mut fg) = (String::new(), String::new(), 0, false);
    let mut flush = |name: &str, bundle: &str, pid: i32, fg: bool| {
        if fg && pid > 0 && !bundle.is_empty() {
            apps.push(App {
                name: name.to_string(),
                bundle_id: bundle.to_string(),
                pid,
            });
        }
    };
    for line in text.lines() {
        let t = line.trim_start();
        if let Some(rest) = t
            .split_once(") \"")
            .filter(|(n, _)| n.chars().all(|c| c.is_ascii_digit()))
            .map(|(_, r)| r)
        {
            flush(&name, &bundle, pid, fg);
            name = rest.split('"').next().unwrap_or("").to_string();
            (bundle, pid, fg) = (String::new(), 0, false);
        } else if let Some(b) = t.strip_prefix("bundleID=\"") {
            bundle = b.split('"').next().unwrap_or("").to_string();
        } else if let Some(p) = t.strip_prefix("pid = ") {
            pid = p
                .split_whitespace()
                .next()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            fg = t.contains("type=\"Foreground\"");
        }
    }
    flush(&name, &bundle, pid, fg);
    apps
}

pub fn running_apps() -> Vec<App> {
    Command::new("lsappinfo")
        .arg("list")
        .output()
        .map(|o| parse_lsappinfo(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or_default()
}

/// Never offered: the shell / agent this runs under, and the system's own.
fn protected(app: &App, ancestors: &[i32], keep: &[String]) -> bool {
    const ALWAYS: &[&str] = &[
        "com.apple.finder",
        "com.apple.dock",
        "com.apple.systemuiserver",
        "com.apple.loginwindow",
    ];
    ancestors.contains(&app.pid)
        || ALWAYS.contains(&app.bundle_id.as_str())
        || keep.iter().any(|k| k == &app.bundle_id || k == &app.name)
}

/// This process's parents up to launchd: the terminal or agent app among
/// them must stay open.
fn ancestors() -> Vec<i32> {
    let out = Command::new("ps")
        .args(["-Ao", "pid=,ppid="])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();
    let parent: std::collections::HashMap<i32, i32> = out
        .lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            Some((it.next()?.parse().ok()?, it.next()?.parse().ok()?))
        })
        .collect();
    let mut chain = Vec::new();
    let mut p = sys::me();
    while let Some(&pp) = parent.get(&p) {
        if pp <= 1 || chain.contains(&pp) {
            break;
        }
        chain.push(pp);
        p = pp;
    }
    chain
}

/// CPU % per process, from ps.
fn cpu_by_pid() -> std::collections::HashMap<i32, f64> {
    let out = Command::new("ps")
        .args(["-Ao", "pid=,pcpu="])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();
    out.lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            Some((
                it.next()?.parse().ok()?,
                it.next()?.replace(',', ".").parse().ok()?,
            ))
        })
        .collect()
}

/// Apps (with their helpers) using more than 3 % of one core, busiest first.
pub fn busy_apps(keep: &[String]) -> Vec<(App, f64)> {
    let cpu = cpu_by_pid();
    let up = ancestors();
    let mut busy: Vec<(App, f64)> = running_apps()
        .into_iter()
        .filter(|a| !protected(a, &up, keep))
        .map(|a| {
            let total = sys::tree(a.pid)
                .iter()
                .filter_map(|p| cpu.get(p))
                .sum::<f64>();
            (a, total)
        })
        .filter(|(_, c)| *c > 3.0)
        .collect();
    busy.sort_by(|a, b| b.1.total_cmp(&a.1));
    busy
}

pub enum Answer {
    Close,
    RunAnyway,
    Wait,
}

fn applescript_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// A dialog in front of the user; no answer in 2 minutes means run anyway
/// without closing anything.
pub fn ask(label: &str, busy: &[(App, f64)], also: &[String]) -> Answer {
    let apps = busy
        .iter()
        .map(|(a, c)| format!("{} ({c:.0} %)", a.name))
        .collect::<Vec<_>>()
        .join(", ");
    let mut text = format!("«{label}» needs a quiet Mac to measure.\n\nBusy apps: {apps}.");
    if !also.is_empty() {
        text.push_str(&format!("\nAlso busy: {}.", also.join(", ")));
    }
    text.push_str("\n\nQuit them while it runs and reopen them afterwards? They quit normally and ask about anything unsaved.");
    let script = format!(
        "display dialog {} with title \"machine-slot\" buttons {{\"Wait\", \"Run anyway\", \"Quit and reopen after\"}} default button 3 giving up after 120",
        applescript_str(&text)
    );
    let out = Command::new("osascript").args(["-e", &script]).output();
    let reply = out
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();
    if reply.contains("Quit and reopen after") && !reply.contains("gave up:true") {
        Answer::Close
    } else if reply.contains("button returned:Wait") {
        Answer::Wait
    } else {
        Answer::RunAnyway
    }
}

/// Quits each app normally; returns the ones that did quit (an app with an
/// unsaved document may stay open — the user decides there).
pub fn quit(apps: &[App], state: &Path) -> Vec<App> {
    let mut closed = Vec::new();
    for a in apps {
        let script = format!(
            "tell application id {} to quit",
            applescript_str(&a.bundle_id)
        );
        let _ = Command::new("osascript").args(["-e", &script]).output();
        let t = Instant::now();
        while sys::footprint(a.pid).is_some() && t.elapsed() < Duration::from_secs(20) {
            std::thread::sleep(Duration::from_millis(250));
        }
        if sys::footprint(a.pid).is_none() {
            closed.push(a.clone());
            remember(state, &closed);
        } else {
            eprintln!(
                "machine-slot: {} didn't quit (unsaved work?); left open",
                a.name
            );
        }
    }
    closed
}

fn remember(state: &Path, apps: &[App]) {
    let text: String = apps
        .iter()
        .map(|a| format!("{}\t{}\n", a.bundle_id, a.name))
        .collect();
    let _ = std::fs::write(state.join("closed-apps"), text);
}

/// Reopens what a solo closed, in the background, and forgets the list.
pub fn reopen(state: &Path) -> Vec<String> {
    let path = state.join("closed-apps");
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let mut names = Vec::new();
    for line in text.lines() {
        let Some((id, name)) = line.split_once('\t') else {
            continue;
        };
        let _ = Command::new("open").args(["-g", "-b", id]).status();
        names.push(name.to_string());
    }
    let _ = std::fs::remove_file(path);
    names
}

pub fn pending(state: &Path) -> Vec<String> {
    std::fs::read_to_string(state.join("closed-apps"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.split_once('\t').map(|(_, n)| n.to_string()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_foreground_apps_from_lsappinfo() {
        let text = r#" 1) "loginwindow" ASN:0x0-0x2002:
    bundleID="com.apple.loginwindow"
    pid = 434 token=[sess=1] type="UIElement" flavor=3
 2) "Google Chrome" ASN:0x0-0x5005:
    bundleID="com.google.Chrome"
    bundle path="/Applications/Google Chrome.app"
    pid = 900 token=[sess=1 pid=900] type="Foreground" flavor=3
 3) "helperd" ASN:0x0-0x6006:
    bundleID=[ NULL ]
    pid = 901 type="BackgroundOnly"
10) "Photos Pro" ASN:0x0-0x7007:
    bundleID="com.example.photos"
    pid = 1200 type="Foreground"
"#;
        let apps = parse_lsappinfo(text);
        assert_eq!(apps.len(), 2, "{apps:?}");
        assert_eq!(
            apps[0],
            App {
                name: "Google Chrome".into(),
                bundle_id: "com.google.Chrome".into(),
                pid: 900
            }
        );
        assert_eq!(apps[1].pid, 1200);
    }

    #[test]
    fn the_agent_running_this_is_never_offered() {
        let a = App {
            name: "Terminal".into(),
            bundle_id: "com.apple.Terminal".into(),
            pid: 50,
        };
        assert!(protected(&a, &[60, 50], &[]));
        assert!(!protected(&a, &[60], &[]));
        assert!(protected(&a, &[], &["Terminal".into()]));
        let f = App {
            name: "Finder".into(),
            bundle_id: "com.apple.finder".into(),
            pid: 2,
        };
        assert!(protected(&f, &[], &[]));
    }

    #[test]
    fn live_apps_are_listed() {
        // On a desktop session there is always some Dock app.
        assert!(!running_apps().is_empty());
    }
}
