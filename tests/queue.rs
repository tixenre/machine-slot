//! The real binary, several at once, in a private state and record.

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_machine-slot");

struct World {
    dir: PathBuf,
}

impl World {
    fn new(name: &str) -> World {
        let dir =
            std::env::temp_dir().join(format!("machine-slot-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        World { dir }
    }
    fn cmd(&self) -> Command {
        let mut c = Command::new(BIN);
        c.env("MACHINE_SLOT_STATE", self.dir.join("state"))
            .env("MACHINE_SLOT_DIR", self.dir.join("rec"))
            .env("MACHINE_SLOT_SETTLE", "0")
            .env_remove("MACHINE_SLOT");
        c
    }
    /// A job that notes when it starts and ends into events.
    fn job(&self, mode: &str, label: &str, secs: f64) -> Child {
        let ev = self.dir.join("events");
        let script = format!(
            "echo \"$(date +%s.%N 2>/dev/null || date +%s) {label}+\" >> '{e}'; sleep {secs}; echo \"x {label}-\" >> '{e}'",
            e = ev.display()
        );
        self.cmd()
            .args([mode, label, "--", "sh", "-c", &script])
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap()
    }
    fn events(&self) -> Vec<String> {
        std::fs::read_to_string(self.dir.join("events"))
            .unwrap_or_default()
            .lines()
            .map(|l| l.split_whitespace().last().unwrap().to_string())
            .collect()
    }
    fn seed(&self, mode: &str, label: &str, secs: u64) {
        let rec = self.dir.join("rec");
        std::fs::create_dir_all(&rec).unwrap();
        let line = format!("1\t{mode}\t{label}\t0\t{secs}\t0\t{label}\t\t\t\t\t\t0\t1\n");
        let path = rec.join("runs.tsv");
        let mut text = std::fs::read_to_string(&path).unwrap_or_default();
        text.push_str(&line);
        text.push_str(&line);
        std::fs::write(path, text).unwrap();
    }
    fn status(&self) -> String {
        String::from_utf8(self.cmd().arg("status").output().unwrap().stdout).unwrap()
    }
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Waits (up to 20 s) until `status` shows `what`: no guessing how long a
/// loaded machine takes to start a process.
fn until(w: &World, what: &str) {
    let t = Instant::now();
    while !w.status().contains(what) {
        assert!(
            t.elapsed() < Duration::from_secs(20),
            "never saw «{what}» in:\n{}",
            w.status()
        );
        pause(50);
    }
}

fn pause(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

fn before(ev: &[String], a: &str, b: &str) -> bool {
    let i = ev
        .iter()
        .position(|e| e == a)
        .unwrap_or_else(|| panic!("no {a} in {ev:?}"));
    let j = ev
        .iter()
        .position(|e| e == b)
        .unwrap_or_else(|| panic!("no {b} in {ev:?}"));
    i < j
}

#[test]
fn a_solo_waits_for_shared_work_and_runs_alone_in_order() {
    let w = World::new("order");
    let mut a = w.job("shared", "A", 2.0);
    let mut b = w.job("shared", "B", 2.0);
    until(&w, "shared: A");
    until(&w, "shared: B");
    let mut s1 = w.job("solo", "S1", 1.0);
    until(&w, "next solo: S1");
    let mut c = w.job("shared", "C", 0.5);
    until(&w, "waiting shared: C");
    let mut s2 = w.job("solo", "S2", 1.0);
    until(&w, "queued solo: S2");
    for j in [&mut a, &mut b, &mut s1, &mut c, &mut s2] {
        assert!(j.wait().unwrap().success());
    }
    let ev = w.events();
    // A and B together; S1 after both; S2 after S1; C (no history) after the solos.
    assert!(before(&ev, "B+", "A-"), "{ev:?}");
    assert!(
        before(&ev, "A-", "S1+") && before(&ev, "B-", "S1+"),
        "{ev:?}"
    );
    assert!(before(&ev, "S1-", "S2+"), "{ev:?}");
    assert!(before(&ev, "S2-", "C+"), "{ev:?}");
}

#[test]
fn short_work_with_history_fits_in_the_drain_window() {
    let w = World::new("window");
    w.seed("shared", "long", 4);
    w.seed("shared", "short", 1);
    w.seed("shared", "huge", 30);
    let mut long = w.job("shared", "long", 4.0);
    until(&w, "shared: long");
    let mut s = w.job("solo", "S", 0.5);
    until(&w, "next solo: S");
    let mut short = w.job("shared", "short", 1.0);
    let mut huge = w.job("shared", "huge", 0.3);
    for j in [&mut long, &mut s, &mut short, &mut huge] {
        assert!(j.wait().unwrap().success());
    }
    let ev = w.events();
    assert!(
        before(&ev, "short-", "long-"),
        "short fit inside long's time: {ev:?}"
    );
    assert!(
        before(&ev, "S-", "huge+"),
        "huge didn't fit, waited for the solo: {ev:?}"
    );
}

#[test]
fn a_killed_job_frees_its_place() {
    let w = World::new("killed");
    let mut holder = w.job("solo", "holder", 30.0);
    until(&w, "solo: holder");
    assert!(w.status().contains("solo: holder"), "{}", w.status());
    // SIGKILL machine-slot itself: no cleanup runs, only the lock goes.
    Command::new("kill")
        .args(["-9", &holder.id().to_string()])
        .status()
        .unwrap();
    holder.wait().unwrap();
    pause(200);
    let t = Instant::now();
    let mut next = w.job("shared", "next", 0.1);
    assert!(next.wait().unwrap().success());
    assert!(t.elapsed() < Duration::from_secs(5), "waited on a dead job");
    // The orphaned sleep is its own business; clean it up.
    let _ = Command::new("pkill").args(["-f", "sleep 30"]).status();
}

#[test]
fn terminate_stops_the_whole_command_and_is_recorded() {
    let w = World::new("term");
    let mut j = w
        .cmd()
        .args([
            "shared",
            "term",
            "--",
            "sh",
            "-c",
            "sleep 41 & sleep 41; wait",
        ])
        .spawn()
        .unwrap();
    until(&w, "shared: term");
    pause(300); // the command itself is up
    Command::new("kill")
        .args(["-TERM", &j.id().to_string()])
        .status()
        .unwrap();
    let st = j.wait().unwrap();
    assert!(!st.success());
    pause(200);
    let left = Command::new("pgrep")
        .args(["-f", "sleep 41"])
        .output()
        .unwrap();
    assert!(left.stdout.is_empty(), "children left running");
    let rec = std::fs::read_to_string(w.dir.join("rec/runs.tsv")).unwrap();
    assert!(rec.lines().nth(1).unwrap().contains("\tterm\t"), "{rec}");
}

#[test]
fn records_exit_cpu_and_memory() {
    let w = World::new("record");
    let st = w
        .cmd()
        .args([
            "shared",
            "py mem 5",
            "--",
            "python3",
            "-c",
            "x = bytearray(150 * 1024 * 1024); s = sum(range(2 * 10**6)); raise SystemExit(3)",
        ])
        .status()
        .unwrap();
    assert_eq!(st.code(), Some(3));
    let rec = std::fs::read_to_string(w.dir.join("rec/runs.tsv")).unwrap();
    let row: Vec<&str> = rec.lines().nth(1).unwrap().split('\t').collect();
    assert_eq!(row[1], "shared");
    assert_eq!(row[2], "py mem", "numbers leave the key");
    assert_eq!(row[5], "3");
    let rss: u64 = row[9].parse().unwrap();
    assert!((140..1000).contains(&rss), "max rss {rss} MB");
    assert!(row[7].parse::<f64>().unwrap() > 0.0);
    assert!(Path::new(&w.dir.join("rec/times.txt")).exists());
}

#[test]
fn nesting_reuses_the_turn() {
    let w = World::new("nest");
    let inner = format!("{BIN} shared inner -- echo inner-ran");
    let out = w
        .cmd()
        .args(["solo", "outer", "--", "sh", "-c", &inner])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("inner-ran"));
    let bad = format!("{BIN} solo inner -- true");
    let st = w
        .cmd()
        .args(["shared", "outer", "--", "sh", "-c", &bad])
        .status()
        .unwrap();
    assert_eq!(st.code(), Some(2));
}

#[test]
fn mcp_answers_tools() {
    use std::io::Write;
    let w = World::new("mcp");
    w.seed("shared", "engine: cargo test", 12);
    let mut p = w
        .cmd()
        .arg("mcp")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = p.stdin.take().unwrap();
    for msg in [
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}"#,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"estimate","arguments":{"label":"engine: cargo test 42"}}}"#,
        r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"status","arguments":{}}}"#,
    ] {
        writeln!(stdin, "{msg}").unwrap();
    }
    drop(stdin);
    let out = String::from_utf8(p.wait_with_output().unwrap().stdout).unwrap();
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 4, "{out}");
    assert!(lines[0].contains("\"serverInfo\"") && lines[0].contains("2025-06-18"));
    assert!(lines[1].contains("\"estimate\"") && lines[1].contains("\"history\""));
    assert!(lines[2].contains("usual: ~12s"), "{}", lines[2]);
    assert!(lines[3].contains("free"));
}

#[test]
fn a_command_can_report_its_progress() {
    let w = World::new("progress");
    let script = "echo 30/120 > \"$MACHINE_SLOT_PROGRESS\"; sleep 3";
    let mut j = w
        .cmd()
        .args(["shared", "reporting", "--", "sh", "-c", script])
        .spawn()
        .unwrap();
    until(&w, "shared: reporting");
    until(&w, "25 %");
    let state = String::from_utf8(w.cmd().arg("state").output().unwrap().stdout).unwrap();
    assert!(state.contains("\t0.250\tmeasured"), "{state}");
    assert!(j.wait().unwrap().success());
    // The progress file goes with the job.
    assert!(
        std::fs::read_dir(w.dir.join("state/jobs"))
            .unwrap()
            .next()
            .is_none()
    );
}

#[test]
fn estimated_progress_comes_from_the_usual_time() {
    let w = World::new("estimated");
    w.seed("shared", "steady", 100);
    let mut j = w.job("shared", "steady", 2.0);
    until(&w, "~");
    let state = String::from_utf8(w.cmd().arg("state").output().unwrap().stdout).unwrap();
    assert!(state.contains("\testimated"), "{state}");
    assert!(j.wait().unwrap().success());
}

#[test]
fn a_running_job_shows_what_it_uses() {
    let w = World::new("live");
    // One process that writes 40 MB to disk, then keeps a core busy: it's
    // alive at the samples, so both show.
    // On a real disk: the temp folder may be in memory (tmpfs on Linux).
    let script = format!(
        "import os, tempfile, time\n\
         fd, p = tempfile.mkstemp(dir={:?})\n\
         os.write(fd, b'x' * (40 << 20)); os.fsync(fd); os.close(fd); os.remove(p)\n\
         t = time.time() + 4\n\
         while time.time() < t: pass\n",
        env!("CARGO_TARGET_TMPDIR")
    );
    let mut j = w
        .cmd()
        .args(["shared", "busy", "--", "python3", "-c", &script])
        .spawn()
        .unwrap();
    until(&w, "cpu ");
    let state = String::from_utf8(w.cmd().arg("state").output().unwrap().stdout).unwrap();
    let f: Vec<&str> = state.lines().next().unwrap().split('\t').collect();
    assert_eq!(f.len(), 14, "{state}");
    let cpu: f64 = f[7].parse().unwrap();
    let mem: u64 = f[8].parse().unwrap();
    assert!(cpu > 20.0, "a busy loop shows as busy: {state}");
    assert!(mem > 0, "{state}");
    assert!(j.wait().unwrap().success());
    let rec = std::fs::read_to_string(w.dir.join("rec/runs.tsv")).unwrap();
    let head: Vec<&str> = rec.lines().next().unwrap().split('\t').collect();
    let row: Vec<&str> = rec.lines().nth(1).unwrap().split('\t').collect();
    let at = |name: &str| row[head.iter().position(|h| *h == name).unwrap()];
    assert!(at("peak_cpu_pct").parse::<u64>().unwrap() > 20, "{rec}");
    assert!(
        at("disk_mb").parse::<u64>().unwrap() >= 30,
        "dd wrote 40 MB: {rec}"
    );
}

#[test]
fn short_lived_children_count_toward_live_cpu() {
    let w = World::new("short");
    // A build-like tree: a shell running many busy children that each live
    // a fraction of a second, between the one-second samples.
    let script = "for i in $(seq 24); do python3 -c 'import time\nt=time.time()+0.25\nwhile time.time()<t: pass'; done";
    let ok = w
        .cmd()
        .args(["shared", "short", "--", "sh", "-c", script])
        .status()
        .unwrap();
    assert!(ok.success());
    let rec = std::fs::read_to_string(w.dir.join("rec/runs.tsv")).unwrap();
    let head: Vec<&str> = rec.lines().next().unwrap().split('\t').collect();
    let row: Vec<&str> = rec.lines().nth(1).unwrap().split('\t').collect();
    let at = |name: &str| row[head.iter().position(|h| *h == name).unwrap()];
    let peak: u64 = at("peak_cpu_pct").parse().unwrap();
    assert!(
        peak > 70,
        "one core busy the whole time, seen as {peak} %: {rec}"
    );
}

#[test]
fn cancel_stops_a_job_by_label_and_wait_follows_it() {
    let w = World::new("cancel");
    let mut j = w.job("shared", "long sleep 1a2b3c4d", 30.0);
    until(&w, "shared: long sleep");
    let waiter = w
        .cmd()
        .args(["wait", "long sleep 1a2b3c4d"])
        .spawn()
        .unwrap();
    // By task key: the commit id doesn't matter.
    let out = w
        .cmd()
        .args(["cancel", "long sleep 9f8e7d6c"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let st = j.wait().unwrap();
    assert!(!st.success());
    let waited = waiter.wait_with_output().unwrap();
    assert_eq!(waited.status.code(), st.code(), "wait exits as the job did");
    assert!(w.status().contains("free"), "{}", w.status());
    let rec = std::fs::read_to_string(w.dir.join("rec/runs.tsv")).unwrap();
    assert!(
        rec.contains("long sleep 1a2b3c4d"),
        "the stopped run is recorded"
    );
    let none = w.cmd().args(["cancel", "nothing"]).output().unwrap();
    assert_eq!(none.status.code(), Some(1));
}

#[test]
fn a_waiting_solo_says_when_it_starts() {
    let w = World::new("eta");
    w.seed("shared", "steady", 20);
    w.seed("solo", "measure", 30);
    let mut a = w.job("shared", "steady", 3.0);
    until(&w, "shared: steady");
    let mut b = w.job("solo", "measure", 0.2);
    until(&w, "next solo: measure");
    let mut c = w.job("solo", "second", 0.2);
    until(&w, "queued solo: second");
    let st = w.status();
    // steady has ~20 s left, then measure's 30 s, each solo with 5 s of settling.
    assert!(
        st.contains("measure (waiting for running work to end, starts in ~2"),
        "{st}"
    );
    assert!(st.contains("second (#1, starts in ~"), "{st}");
    for j in [&mut a, &mut b, &mut c] {
        assert!(j.wait().unwrap().success());
    }
}

#[test]
fn a_job_past_its_time_limit_is_stopped_and_recorded() {
    let w = World::new("timeout");
    let t = Instant::now();
    let st = w
        .cmd()
        .args(["shared", "hang", "--timeout", "2s", "--", "sleep", "60"])
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
    assert_eq!(st.code(), Some(124));
    assert!(t.elapsed() < Duration::from_secs(15), "{:?}", t.elapsed());
    let rec = std::fs::read_to_string(w.dir.join("rec/runs.tsv")).unwrap();
    let head: Vec<&str> = rec.lines().next().unwrap().split('\t').collect();
    let row: Vec<&str> = rec.lines().nth(1).unwrap().split('\t').collect();
    assert_eq!(
        row[head.iter().position(|h| *h == "timed_out").unwrap()],
        "1"
    );
    assert!(w.status().contains("free"));
}

#[test]
fn a_quiet_job_says_for_how_long() {
    let w = World::new("quiet");
    let mut j = w.job("shared", "idle", 5.0);
    until(&w, "shared: idle");
    pause(3500);
    let state = String::from_utf8(w.cmd().arg("state").output().unwrap().stdout).unwrap();
    let f: Vec<&str> = state.lines().next().unwrap().split('\t').collect();
    let quiet: u64 = f[11].parse().unwrap();
    assert!(quiet >= 2, "a sleeping job does nothing: {state}");
    assert!(j.wait().unwrap().success());
}

#[test]
fn waiting_shared_work_says_when_it_starts() {
    let w = World::new("eta-shared");
    w.seed("solo", "measure", 30);
    w.seed("shared", "build", 10);
    let mut a = w.job("solo", "measure", 3.0);
    until(&w, "solo: measure");
    let mut b = w.job("shared", "build", 0.2);
    until(&w, "waiting shared: build (starts in ~");
    let est = String::from_utf8(
        w.cmd()
            .args(["estimate", "build", "shared"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert!(est.contains("wait now: ~"), "{est}");
    for j in [&mut a, &mut b] {
        assert!(j.wait().unwrap().success());
    }
}
