//! machine-slot: one turn-taker for the heavy work of every session on this
//! Mac. See README.md.

mod ask;
mod calm;
mod hook;
mod json;
mod mcp;
mod paths;
mod record;
mod run;
mod sched;
mod sys;

use record::{Run, clock};
use sched::{Job, Kind, Slot};
use std::path::PathBuf;
use std::time::Duration;

const HELP: &str = "\
machine-slot solo [--needs LIST] [--ask] LABEL -- CMD…
                                   alone: waits for its turn, for running work to end,
                                   then for the Mac to be quiet in LIST (cpu,disk,ram,gpu,
                                   screen; default cpu,disk,ram); nothing new starts until
                                   it ends (benches, baselines, UI tests). One queue.
                                   --ask: if apps keep the Mac busy, ask the user whether
                                   to quit them for the run; they reopen afterwards.
                                   --timeout T: stop it after T (90s, 20m, 1h30m); a
                                   solo without one stops at 3 × its usual time
                                   (10 min at least); 0: no limit.
                                   --at HH:MM: join the queue only then (it holds
                                   nothing until then), e.g. a long solo for tonight.
machine-slot shared LABEL -- CMD…  together with other shared work, never during a solo.
                                   While a solo waits, it starts only if its usual time
                                   ends before the running work does.
machine-slot status                what runs, what waits, how long is left
machine-slot times                 how long each kind of task takes (clean / loaded)
machine-slot log [N]               the last N runs
machine-slot repeats               tests run more than once (same command and repo, any label)
machine-slot estimate LABEL [solo|shared]
machine-slot state [--json]        for the menu bar and scripts
machine-slot mcp                   read-only MCP server on stdio
machine-slot hook                  the Claude Code Bash check (JSON on stdin)
machine-slot cancel LABEL          stop a job (its whole command), record it, free its turn
machine-slot wait LABEL            wait for a job to end; exits as it did
machine-slot reopen                reopen apps a killed --ask run left closed
machine-slot paths                 where the queue, the record and the config live

Use the same LABEL for the same kind of task (commit ids and numbers are ignored).
Waiting can be long: run it in the background.";

pub fn state_dir() -> PathBuf {
    paths::state()
}

fn record_dir() -> PathBuf {
    paths::data()
}

pub fn record_path() -> PathBuf {
    record_dir().join("runs.tsv")
}

/// How far along a running job is, 0 to 1, and whether that's measured
/// (the command reports it) or estimated (elapsed against its usual time).
pub fn fraction(j: &Job, now: u64) -> Option<(f64, bool)> {
    if let Some(p) = j.progress {
        return Some((p, true));
    }
    let u = j.usual.filter(|&u| u > 0)?;
    Some((
        (now.saturating_sub(j.start) as f64 / u as f64).min(0.99),
        false,
    ))
}

fn left(j: &Job, now: u64) -> String {
    let el = now.saturating_sub(j.start);
    let pct = match fraction(j, now) {
        Some((p, true)) => format!("{:.0} %, ", p * 100.0),
        Some((p, false)) => format!("~{:.0} %, ", p * 100.0),
        None => String::new(),
    };
    // A command that reports its progress also says how long is left.
    if let Some(p) = j.progress.filter(|&p| p > 0.02 && el > 2) {
        let rest = (el as f64 / p - el as f64).max(0.0).round() as u64;
        return format!("{pct}{} so far, ~{} left", clock(el), clock(rest));
    }
    match j.usual {
        Some(u) if el <= u => format!(
            "{pct}{} of ~{}, {} left{}",
            clock(el),
            clock(u),
            clock(u - el),
            if j.loaded { " (from loaded runs)" } else { "" }
        ),
        Some(u) => format!("{pct}{}, longer than usual (~{})", clock(el), clock(u)),
        None => format!("{}, no history", clock(el)),
    }
}

/// A running tree this long without CPU or disk is shown as maybe stalled.
pub const STALLED_S: u64 = 300;

/// «cpu 340 % · 2.1 GB · disk 12 MB/s», while it runs.
pub fn live_text(j: &Job) -> String {
    match j.live {
        Some((cpu, mem, disk)) => {
            let mem = if mem >= 1024 {
                format!("{:.1} GB", mem as f64 / 1024.0)
            } else {
                format!("{mem} MB")
            };
            let disk = if disk >= 0.5 {
                format!(" · disk {disk:.0} MB/s")
            } else {
                String::new()
            };
            // UI tests drive an app outside the tree, so a quiet tree is a
            // question, not a verdict.
            let quiet = match j.quiet_s {
                Some(q) if q >= STALLED_S => format!(" · nothing for {} (stalled?)", clock(q)),
                _ => String::new(),
            };
            format!(" · cpu {cpu:.0} % · {mem}{disk}{quiet}")
        }
        None => String::new(),
    }
}

/// Seconds until a running job should end: from its own progress, else its
/// usual time (0 once it's past it). None without either.
fn remaining(j: &Job, now: u64) -> Option<u64> {
    let el = now.saturating_sub(j.start);
    if let Some(p) = j.progress.filter(|&p| p > 0.02 && el > 2) {
        return Some((el as f64 / p - el as f64).max(0.0).round() as u64);
    }
    j.usual.map(|u| u.saturating_sub(el))
}

/// The seconds a waiting job should wait for its turn. A solo waits for
/// the running work, then for every solo ahead of it, each with its few
/// seconds of settling. A waiting shared job goes after every solo.
pub fn starts_in(jobs: &[Job], j: &Job, now: u64) -> Option<u64> {
    const SETTLE_S: u64 = 5;
    let solo = match j.kind {
        Kind::Draining | Kind::Queued => true,
        Kind::Waiting => false,
        _ => return None,
    };
    let mut wait = 0;
    for r in jobs.iter().filter(|r| match r.kind {
        Kind::Solo => true,
        // A shared job waiting behind solos doesn't wait for running shared
        // work unless a solo has to drain it first.
        Kind::Shared => solo || jobs.iter().any(|a| a.kind == Kind::Draining),
        _ => false,
    }) {
        wait = wait.max(remaining(r, now)?);
    }
    let ahead = jobs.iter().filter(|a| {
        matches!(a.kind, Kind::Draining | Kind::Queued) && (!solo || a.ticket < j.ticket)
    });
    for a in ahead {
        wait += SETTLE_S + a.usual?;
    }
    Some(if solo { wait + SETTLE_S } else { wait })
}

pub fn status_text(jobs: &[Job]) -> String {
    let now = sched::now();
    let mut out = String::new();
    let mut place = 0;
    for j in jobs {
        let eta = starts_in(jobs, j, now)
            .map(|s| format!(", starts in ~{}", clock(s)))
            .unwrap_or_default();
        if j.kind == Kind::Queued {
            place += 1;
        }
        let line = match j.kind {
            Kind::Solo => match &j.settling {
                Some((since, what)) => format!(
                    "solo: {} (waiting {} for a quiet machine: {what})",
                    j.label,
                    clock(now.saturating_sub(*since))
                ),
                None => format!("solo: {} ({}{})", j.label, left(j, now), live_text(j)),
            },
            Kind::Shared => format!("shared: {} ({}{})", j.label, left(j, now), live_text(j)),
            Kind::Draining => format!(
                "next solo: {} (waiting for running work to end{eta})",
                j.label
            ),
            Kind::Queued => format!("queued solo: {} (#{place}{eta})", j.label),
            Kind::Waiting => format!(
                "waiting shared: {}{}",
                j.label,
                starts_in(jobs, j, now)
                    .map(|s| format!(" (starts in ~{})", clock(s)))
                    .unwrap_or_default()
            ),
        };
        out.push_str(&line);
        out.push('\n');
    }
    if out.is_empty() { "free\n".into() } else { out }
}

pub fn estimate_text(label: &str, mode: &str) -> String {
    let key = record::key_of(label);
    let runs = record::read(&record_path());
    let jobs = sched::peek(&state_dir());
    let now = sched::now();
    let usual = record::usual(&runs, mode, &key);
    let (load, cpus) = (sys::load1(), sys::cpus() as f64);
    // As if it joined the queue now: a solo behind every solo, a shared job
    // after them all, or right away when no solo is around.
    let solos = jobs
        .iter()
        .any(|j| matches!(j.kind, Kind::Solo | Kind::Queued | Kind::Draining));
    let me = Job {
        kind: if mode == "solo" {
            if solos { Kind::Queued } else { Kind::Draining }
        } else {
            Kind::Waiting
        },
        ticket: u64::MAX,
        ..Job::blank()
    };
    let wait = if mode != "solo" && !solos {
        Some(0)
    } else {
        starts_in(&jobs, &me, now)
    };
    format!(
        "task: {key} ({mode})\nusual: {}\nwait now: {}\n",
        usual
            .map(|u| {
                if u.loaded {
                    format!("~{} (only loaded runs: an upper bound)", clock(u.secs))
                } else if mode == "solo" || u.now(load, cpus) == u.secs {
                    format!("~{}", clock(u.secs))
                } else {
                    format!(
                        "~{} at today's load (~{} on a free machine)",
                        clock(u.now(load, cpus)),
                        clock(u.secs)
                    )
                }
            })
            .unwrap_or_else(|| "no history".into()),
        wait.map(|w| format!("~{}", clock(w)))
            .unwrap_or_else(|| "unknown (work ahead without history)".into()),
    )
}

fn write_times() {
    let runs = record::read(&record_path());
    let path = record_dir().join("times.txt");
    let tmp = path.with_extension("txt.tmp");
    if std::fs::write(&tmp, record::times_table(&runs)).is_ok() {
        let _ = std::fs::rename(tmp, path);
    }
}

struct Calm {
    settled: bool,
    waited_s: u64,
    /// What still wasn't quiet when it started (empty when calm).
    not_quiet: String,
}

/// Waits until everything the solo needs has been quiet for QUIET seconds
/// in a row: at most 60 s, or 10 min when it needs the screen (a UI test
/// can't run on a locked or busy screen anyway). Never closes anything:
/// when it doesn't calm down it says what keeps it busy and goes on.
/// `note` gets what isn't quiet yet, each second, for `status` to show.
fn settle(
    label: &str,
    needs: &[calm::Need],
    asking: bool,
    closed: &mut Vec<ask::App>,
    note: &std::path::Path,
) -> Calm {
    let quiet_needed: u32 = std::env::var("MACHINE_SLOT_SETTLE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(5);
    if quiet_needed == 0 || needs.is_empty() {
        return Calm {
            settled: true,
            waited_s: 0,
            not_quiet: String::new(),
        };
    }
    let mut max: u64 = if needs.contains(&calm::Need::Screen) {
        600
    } else {
        60
    };
    let t0 = sched::now();
    let mut probe = calm::Probe::new();
    let (mut quiet, mut last, mut said, mut asked, mut busy_for) =
        (0u32, Vec::new(), 0u64, !asking, 0u32);
    while quiet < quiet_needed && sched::now() - t0 < max && run::stop_requested().is_none() {
        last = probe.look(needs);
        let _ = std::fs::write(
            note,
            format!(
                "{}\t{}\n",
                t0,
                if last.is_empty() {
                    "quiet".to_string()
                } else {
                    last.join(", ")
                }
            ),
        );
        if last.is_empty() {
            quiet += 1;
            busy_for = 0;
        } else {
            quiet = 0;
            busy_for += 1;
            // Busy for a few seconds in a row: offer to quit the busy apps.
            if !asked && busy_for >= 3 {
                asked = true;
                let keep: Vec<String> = paths::config("ask_keep")
                    .unwrap_or_default()
                    .split(',')
                    .map(|k| k.trim().to_string())
                    .filter(|k| !k.is_empty())
                    .collect();
                let busy = ask::busy_apps(&keep);
                if !busy.is_empty() {
                    eprintln!("machine-slot: solo «{label}» asks the user about busy apps…");
                    match ask::ask(label, &busy, &last) {
                        ask::Answer::Close => {
                            let apps: Vec<ask::App> = busy.into_iter().map(|(a, _)| a).collect();
                            closed.extend(ask::quit(&apps, &state_dir()));
                            if !closed.is_empty() {
                                let names: Vec<&str> =
                                    closed.iter().map(|a| a.name.as_str()).collect();
                                eprintln!(
                                    "machine-slot: quit {} for the run; they reopen after it",
                                    names.join(", ")
                                );
                            }
                            max += 30; // what was closed takes a moment to settle
                        }
                        ask::Answer::RunAnyway => break,
                        ask::Answer::Wait => {}
                    }
                }
            }
            let waited = sched::now() - t0;
            if waited >= said {
                eprintln!(
                    "machine-slot: solo «{label}» waits for a quiet Mac: {}",
                    last.join(", ")
                );
                said = waited + 15;
            }
        }
    }
    let waited_s = sched::now() - t0;
    if quiet >= quiet_needed {
        if waited_s > quiet_needed as u64 + 1 {
            eprintln!(
                "machine-slot: solo «{label}»: the Mac is quiet after {}",
                clock(waited_s)
            );
        }
        return Calm {
            settled: true,
            waited_s,
            not_quiet: String::new(),
        };
    }
    let top = std::process::Command::new("ps")
        .args(["-Aceo", "pcpu=,comm=", "-r"])
        .output()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .take(5)
                .map(|l| l.trim().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    eprintln!(
        "machine-slot: solo «{label}» starts on a busy Mac ({}); marked loaded. Busiest: {top}",
        last.join(", ")
    );
    Calm {
        settled: false,
        waited_s,
        not_quiet: last.join(", "),
    }
}

struct Identity {
    cmd_id: String,
    repo: String,
    head: String,
    command: String,
}

/// Which test this is, from git (one call) and the command line.
fn identity(cmd: &[String]) -> Identity {
    let cwd = std::env::current_dir().unwrap_or_default();
    let out = std::process::Command::new("git")
        .args([
            "rev-parse",
            "--path-format=absolute",
            "--show-toplevel",
            "--git-common-dir",
            "--short",
            "HEAD",
        ])
        .current_dir(&cwd)
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();
    let mut l = out.lines();
    let (top, common, head) = (
        l.next().unwrap_or(""),
        l.next().unwrap_or(""),
        l.next().unwrap_or(""),
    );
    let cwd_s = cwd.to_string_lossy().to_string();
    // Outside a repo, the directory itself is the place.
    let (repo, rel) = if top.is_empty() {
        (cwd_s.clone(), String::new())
    } else {
        (
            common.trim_end_matches("/.git").to_string(),
            cwd_s
                .strip_prefix(top)
                .unwrap_or("")
                .trim_start_matches('/')
                .to_string(),
        )
    };
    let home = paths::home().to_string_lossy().to_string();
    let repo_id = if top.is_empty() {
        repo.replace(&home, "~")
    } else {
        common.to_string()
    };
    let (cmd_id, command) = record::fingerprint(&repo_id, top, &rel, &home, cmd);
    let shown = if rel.is_empty() {
        repo.replace(&home, "~")
    } else {
        format!("{}/{rel}", repo.replace(&home, "~"))
    };
    Identity {
        cmd_id,
        repo: shown,
        head: head.to_string(),
        command,
    }
}

/// Reopens the apps a solo quit, however the run ends.
struct Reopen(bool);

impl Drop for Reopen {
    fn drop(&mut self) {
        if self.0 {
            let names = ask::reopen(&state_dir());
            if !names.is_empty() {
                eprintln!("machine-slot: reopened {}", names.join(", "));
            }
        }
    }
}

/// How a turn is asked for, past its kind and label.
pub struct Opts {
    pub needs: Vec<calm::Need>,
    pub asking: bool,
    /// None: the default (a solo stops at 3 × its usual time, 10 min at
    /// least); Some(0): no limit.
    pub timeout: Option<u64>,
    /// Join the queue only at this time (Unix seconds), holding nothing
    /// until then.
    pub at: Option<u64>,
}

/// The next time the clock shows «HH:MM», today or tomorrow, as Unix seconds.
pub fn next_clock_time(hhmm: &str, now: u64) -> Option<u64> {
    let (h, m) = hhmm.split_once(':')?;
    let (h, m): (u64, u64) = (h.parse().ok()?, m.parse().ok()?);
    if h > 23 || m > 59 {
        return None;
    }
    // Local midnight from date(1), to keep time zone rules out of here.
    let out = std::process::Command::new("date")
        .args(["+%H %M %S"])
        .output()
        .ok()?;
    let t = String::from_utf8_lossy(&out.stdout);
    let f: Vec<u64> = t
        .split_whitespace()
        .filter_map(|w| w.parse().ok())
        .collect();
    let since_midnight = f.first()? * 3600 + f.get(1)? * 60 + f.get(2)?;
    let mut at = now - since_midnight + h * 3600 + m * 60;
    if at <= now {
        at += 86400;
    }
    Some(at)
}

/// A solo that hangs holds the whole machine: without a limit of its own
/// it's stopped at 3 × its usual time, never under 10 minutes.
pub fn time_limit(solo: bool, asked: Option<u64>, usual: Option<u64>) -> Option<u64> {
    match asked {
        Some(0) => None,
        Some(s) => Some(s),
        None if solo => usual.map(|u| (u * 3).max(600)),
        None => None,
    }
}

/// «90», «90s», «20m», «2h», «1h30m» → seconds.
pub fn parse_duration(s: &str) -> Option<u64> {
    let mut total = 0u64;
    let mut num = String::new();
    for c in s.trim().chars() {
        if c.is_ascii_digit() {
            num.push(c);
            continue;
        }
        let n: u64 = num.parse().ok()?;
        num.clear();
        total += n * match c {
            's' => 1,
            'm' => 60,
            'h' => 3600,
            _ => return None,
        };
    }
    if !num.is_empty() {
        total += num.parse::<u64>().ok()?;
    }
    Some(total)
}

fn take_turn(solo: bool, opts: &Opts, label: &str, cmd: &[String]) -> i32 {
    let (needs, asking) = (&opts.needs[..], opts.asking);
    let mode = if solo { "solo" } else { "shared" };
    if let Some(at) = opts.at {
        run::catch_stops();
        eprintln!(
            "machine-slot: {mode} «{label}» joins the queue at {} ({} from now)",
            stamp(at),
            clock(at.saturating_sub(sched::now()))
        );
        while sched::now() < at {
            if let Some(sig) = run::stop_requested() {
                eprintln!("machine-slot: {mode} «{label}» stopped before its time");
                return 128 + sig;
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    }
    // Already inside a turn: a solo has the machine; a shared inside a
    // shared is still shared; a solo inside a shared would wait on itself.
    match std::env::var("MACHINE_SLOT").as_deref() {
        Ok("solo") => return exec(cmd),
        Ok("shared") if !solo => return exec(cmd),
        Ok("shared") => {
            eprintln!(
                "machine-slot: a solo inside a shared turn would wait on itself; run it on its own"
            );
            return 2;
        }
        _ => {}
    }
    run::catch_stops();
    let label: String = label.replace(['\t', '\n'], " ");
    let key = record::key_of(&label);
    let id = identity(cmd);
    let runs = record::read(&record_path());
    let usual = record::usual_for(&runs, mode, &id.cmd_id, &key);
    // A solo runs on a quiet machine; a shared one at today's load.
    let usual_now = usual.map(|u| {
        if solo {
            u.secs
        } else {
            u.now(sys::load1(), sys::cpus() as f64)
        }
    });
    let mut slot = match Slot::enter(
        &state_dir(),
        solo,
        &label,
        usual.zip(usual_now).map(|(u, now)| (now, u.loaded)),
    ) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("machine-slot: can't use {}: {e}", state_dir().display());
            return 2;
        }
    };
    let queued_at = sched::now();
    let mut said = 0u64;
    loop {
        if let Some(sig) = run::stop_requested() {
            eprintln!("machine-slot: {mode} «{label}» stopped while waiting");
            return 128 + sig;
        }
        match slot.try_start() {
            Ok(true) => break,
            Ok(false) => {}
            Err(e) => {
                eprintln!("machine-slot: {e}");
                return 2;
            }
        }
        let waited = sched::now() - queued_at;
        if waited >= said {
            let jobs = sched::peek(&state_dir());
            let eta = jobs
                .iter()
                .find(|j| j.pid == sys::me())
                .and_then(|me| starts_in(&jobs, me, sched::now()))
                .map(|s| format!(", should start in ~{}", clock(s)))
                .unwrap_or_default();
            eprintln!(
                "machine-slot: {mode} «{label}» waits ({}{eta}):",
                clock(waited)
            );
            for l in status_text(&sched::peek(&state_dir())).lines() {
                eprintln!("  {l}");
            }
            said = waited + 30;
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    let wait_s = sched::now() - queued_at;
    if wait_s > 0 {
        eprintln!(
            "machine-slot: {mode} «{label}» starts after {}{}",
            clock(wait_s),
            usual_now
                .map(|u| format!(" (usually ~{})", clock(u)))
                .unwrap_or_default()
        );
    }
    // A solo has the queue to itself; it also waits a little for the rest of
    // the Mac (indexing, other apps) to calm down.
    if solo {
        let left = ask::pending(&state_dir());
        if !left.is_empty() {
            eprintln!(
                "machine-slot: an earlier run left these apps closed: {} (`machine-slot reopen` reopens them)",
                left.join(", ")
            );
        }
    }
    let mut closed = Vec::new();
    let calm = if solo {
        let note = slot.calm_path();
        let c = settle(&label, needs, asking, &mut closed, &note);
        let _ = std::fs::remove_file(&note);
        Some(c)
    } else {
        None
    };
    let _reopen = Reopen(!closed.is_empty());
    if let Some(sig) = run::stop_requested() {
        eprintln!("machine-slot: {mode} «{label}» stopped before starting");
        return 128 + sig;
    }
    let load_start = sys::load1();
    let start = sched::now();
    let progress = slot.progress_path();
    let live = slot.live_path();
    let limit = time_limit(solo, opts.timeout, usual.map(|u| u.secs));
    let out = run::run(
        cmd,
        mode,
        &progress,
        &live,
        limit.map(Duration::from_secs),
        || slot.others_running(),
    );
    drop(slot);
    let quiet = match &calm {
        Some(c) => c.settled,
        None => load_start < sys::cpus() as f64 * 0.3,
    };
    let r = Run {
        start,
        mode: mode.into(),
        key,
        wait_s,
        run_s: out.run_s,
        exit: out.exit,
        label: label.clone(),
        cpu_user_s: Some(out.cpu_user_s),
        cpu_sys_s: Some(out.cpu_sys_s),
        max_rss_mb: Some(out.max_rss_mb),
        peak_tree_mb: Some(out.peak_tree_mb),
        load_start: Some(load_start),
        concurrent_max: Some(out.concurrent_max),
        clean: out.concurrent_max == 0 && quiet,
        cmd_id: id.cmd_id,
        repo: id.repo,
        head: id.head,
        command: id.command,
        needs: if solo {
            calm::Need::list(needs)
        } else {
            String::new()
        },
        settle_s: calm.as_ref().map(|c| c.waited_s),
        not_quiet: calm
            .as_ref()
            .map(|c| c.not_quiet.clone())
            .unwrap_or_default(),
        power: calm::power_note(),
        disk_mb: Some(out.disk_mb),
        peak_cpu_pct: Some(out.peak_cpu_pct),
        timed_out: out.timed_out,
        closed_apps: closed
            .iter()
            .map(|a| a.name.clone())
            .collect::<Vec<_>>()
            .join(","),
    };
    let _ = std::fs::create_dir_all(record_dir());
    if let Err(e) = record::append(&record_path(), &r) {
        eprintln!("machine-slot: can't write the record: {e}");
    }
    let log = format!(
        "{}  {:<6} {:>5} wait {:>6} run  exit {:<3} cpu {:>6}  peak {:>5}MB  {}{}\n",
        stamp(start),
        mode,
        clock(wait_s),
        clock(out.run_s),
        out.exit,
        clock((out.cpu_user_s + out.cpu_sys_s).round() as u64),
        out.peak_tree_mb,
        label,
        match (r.clean, out.timed_out) {
            (_, true) => "  [timeout]",
            (false, _) => "  [loaded]",
            _ => "",
        }
    );
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(record_dir().join("log.txt"))
    {
        let _ = f.write_all(log.as_bytes());
    }
    write_times();
    out.exit
}

/// Local time as «2026-10-28 19:05:00», via date(1) to keep tz rules out of here.
fn stamp(t: u64) -> String {
    std::process::Command::new("date")
        .args(["-r", &t.to_string(), "+%F %T"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| t.to_string())
}

fn exec(cmd: &[String]) -> i32 {
    use std::os::unix::process::CommandExt;
    let err = std::process::Command::new(&cmd[0]).args(&cmd[1..]).exec();
    eprintln!("machine-slot: can't run {}: {err}", cmd[0]);
    127
}

fn state(json: bool) {
    let jobs = sched::peek(&state_dir());
    if json {
        let arr = jobs
            .iter()
            .map(|j| {
                json::Json::obj([
                    ("kind", json::Json::str(j.kind.name())),
                    ("label", json::Json::str(j.label.clone())),
                    ("start", json::Json::Num(j.start as f64)),
                    (
                        "usual",
                        j.usual
                            .map(|u| json::Json::Num(u as f64))
                            .unwrap_or(json::Json::Null),
                    ),
                    ("loaded", json::Json::Bool(j.loaded)),
                    ("pid", json::Json::Num(j.pid as f64)),
                    (
                        "progress",
                        fraction(j, sched::now())
                            .map(|(p, _)| json::Json::Num(p))
                            .unwrap_or(json::Json::Null),
                    ),
                    ("progress_measured", json::Json::Bool(j.progress.is_some())),
                    (
                        "waiting_for_quiet",
                        j.settling
                            .as_ref()
                            .map(|(_, what)| json::Json::str(what.clone()))
                            .unwrap_or(json::Json::Null),
                    ),
                    (
                        "starts_in",
                        starts_in(&jobs, j, sched::now())
                            .map(|s| json::Json::Num(s as f64))
                            .unwrap_or(json::Json::Null),
                    ),
                    (
                        "live",
                        j.live
                            .map(|(c, m, d)| {
                                json::Json::obj([
                                    ("cpu_pct", json::Json::Num(c)),
                                    ("memory_mb", json::Json::Num(m as f64)),
                                    ("disk_mbs", json::Json::Num(d)),
                                    ("quiet_s", json::Json::Num(j.quiet_s.unwrap_or(0) as f64)),
                                ])
                            })
                            .unwrap_or(json::Json::Null),
                    ),
                ])
            })
            .collect();
        println!("{}", json::Json::Arr(arr).dump());
    } else {
        for j in &jobs {
            let f = fraction(j, sched::now());
            println!(
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                j.kind.name(),
                j.label,
                j.start,
                j.usual.map(|u| u.to_string()).unwrap_or_default(),
                j.pid,
                f.map(|(p, _)| format!("{p:.3}")).unwrap_or_default(),
                f.map(|(_, measured)| if measured { "measured" } else { "estimated" })
                    .unwrap_or_default(),
                j.live
                    .map(|l| format!("{:.0}\t{}\t{:.1}", l.0, l.1, l.2))
                    .unwrap_or_else(|| "\t\t".into()),
                starts_in(&jobs, j, sched::now())
                    .map(|s| s.to_string())
                    .unwrap_or_default(),
                j.quiet_s.map(|q| q.to_string()).unwrap_or_default(),
                j.settling
                    .as_ref()
                    .map(|(since, what)| format!("{}\t{what}", since))
                    .unwrap_or_else(|| "\t".into())
            );
        }
    }
}

/// The live jobs a label names: the exact label, else the same task key.
fn named(label: &str) -> Vec<Job> {
    let jobs = sched::peek(&state_dir());
    let exact: Vec<Job> = jobs.iter().filter(|j| j.label == label).cloned().collect();
    if !exact.is_empty() {
        return exact;
    }
    let key = record::key_of(label);
    jobs.into_iter()
        .filter(|j| record::key_of(&j.label) == key)
        .collect()
}

/// Stops a job the way Ctrl-C would: machine-slot stops its whole command,
/// records the run and frees the turn.
fn cancel(label: &str) -> i32 {
    let jobs = named(label);
    match jobs.len() {
        0 => {
            eprintln!("machine-slot: no job «{label}»");
            1
        }
        1 => {
            sys::send(jobs[0].pid, sys::SIGTERM);
            println!("stopping «{}» ({})", jobs[0].label, jobs[0].kind.name());
            0
        }
        _ => {
            eprintln!(
                "machine-slot: «{label}» names {} jobs; use the exact label:",
                jobs.len()
            );
            for j in &jobs {
                eprintln!("  {}", j.label);
            }
            1
        }
    }
}

/// Waits until no job by that label is left; exits as its last run did.
fn wait_for(label: &str) -> i32 {
    let since = sched::now();
    let mut seen: Vec<String> = Vec::new();
    loop {
        let jobs = named(label);
        if jobs.is_empty() {
            break;
        }
        for j in jobs {
            if !seen.contains(&j.label) {
                seen.push(j.label);
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    if seen.is_empty() {
        eprintln!("machine-slot: no job «{label}»");
        return 1;
    }
    record::read(&record_path())
        .iter()
        .rev()
        .find(|r| seen.contains(&r.label) && r.start + r.wait_s + r.run_s + 2 >= since)
        .map(|r| r.exit)
        .unwrap_or(0)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let a = |i: usize| args.get(i).map(String::as_str);
    let code = match a(0) {
        Some(m @ ("solo" | "shared")) => {
            let mut i = 1;
            let mut needs = calm::DEFAULT.to_vec();
            let mut asking = false;
            let mut timeout = None;
            let mut at = None;
            // Options may come before or after the label, up to `--`.
            let mut label: Option<&str> = None;
            loop {
                match a(i) {
                    Some("--at") => {
                        match a(i + 1).and_then(|t| next_clock_time(t, sched::now())) {
                            Some(t) => at = Some(t),
                            None => {
                                eprintln!("machine-slot: --at takes a clock time, e.g. 23:30");
                                std::process::exit(2);
                            }
                        }
                        i += 2;
                    }
                    Some("--timeout") => {
                        match a(i + 1).and_then(parse_duration) {
                            Some(t) => timeout = Some(t),
                            None => {
                                eprintln!(
                                    "machine-slot: --timeout takes a time, e.g. 90s, 20m, 1h30m (0: none)"
                                );
                                std::process::exit(2);
                            }
                        }
                        i += 2;
                    }
                    Some("--needs") => {
                        match a(i + 1).map(calm::Need::parse_list) {
                            Some(Ok(n)) => needs = n,
                            Some(Err(e)) => {
                                eprintln!("machine-slot: {e}");
                                std::process::exit(2);
                            }
                            None => {
                                eprintln!(
                                    "machine-slot: --needs takes a list, e.g. cpu,disk,ram,gpu,screen"
                                );
                                std::process::exit(2);
                            }
                        }
                        i += 2;
                    }
                    Some("--ask") => {
                        asking = true;
                        i += 1;
                    }
                    Some("--") | None => break,
                    Some(l) if label.is_none() && !l.starts_with("--") => {
                        label = Some(l);
                        i += 1;
                    }
                    Some(other) => {
                        eprintln!("machine-slot: unexpected «{other}» before --");
                        std::process::exit(2);
                    }
                }
            }
            let (Some(label), Some("--")) = (label, a(i)) else {
                eprintln!(
                    "usage: machine-slot {m} [--needs cpu,disk,ram,gpu,screen] [--ask] [--timeout T] [--at HH:MM] LABEL -- CMD…"
                );
                std::process::exit(2);
            };
            if args.len() < i + 2 {
                eprintln!("machine-slot: no command");
                std::process::exit(2);
            }
            let opts = Opts {
                needs,
                asking,
                timeout,
                at,
            };
            take_turn(m == "solo", &opts, label, &args[i + 1..])
        }
        Some("status") => {
            print!("{}", status_text(&sched::peek(&state_dir())));
            0
        }
        Some("repeats") => {
            print!("{}", record::repeats(&record::read(&record_path())));
            0
        }
        Some("times") => {
            print!("{}", record::times_table(&record::read(&record_path())));
            0
        }
        Some("log") => {
            let n: usize = a(1).and_then(|s| s.parse().ok()).unwrap_or(20);
            let text = std::fs::read_to_string(record_dir().join("log.txt")).unwrap_or_default();
            let lines: Vec<&str> = text.lines().collect();
            for l in &lines[lines.len().saturating_sub(n)..] {
                println!("{l}");
            }
            0
        }
        Some("estimate") => match a(1) {
            Some(label) => {
                print!("{}", estimate_text(label, a(2).unwrap_or("shared")));
                0
            }
            None => {
                eprintln!("usage: machine-slot estimate LABEL [solo|shared]");
                2
            }
        },
        Some("state") => {
            state(a(1) == Some("--json"));
            0
        }
        Some("cancel") => match a(1) {
            Some(label) => cancel(label),
            None => {
                eprintln!("usage: machine-slot cancel LABEL");
                2
            }
        },
        Some("wait") => match a(1) {
            Some(label) => wait_for(label),
            None => {
                eprintln!("usage: machine-slot wait LABEL");
                2
            }
        },
        Some("reopen") => {
            let names = ask::reopen(&state_dir());
            println!(
                "{}",
                if names.is_empty() {
                    "nothing to reopen".to_string()
                } else {
                    format!("reopened {}", names.join(", "))
                }
            );
            0
        }
        Some("paths") => {
            println!(
                "state  {}\ndata   {}\nconfig {}",
                state_dir().display(),
                record_dir().display(),
                paths::config_file().display()
            );
            0
        }
        Some("--version" | "-V") => {
            println!("machine-slot {}", env!("CARGO_PKG_VERSION"));
            0
        }
        Some("mcp") => {
            mcp::serve();
            0
        }
        Some("hook") => hook::main(),
        _ => {
            eprintln!("{HELP}");
            2
        }
    };
    std::process::exit(code);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_and_limits() {
        assert_eq!(parse_duration("90"), Some(90));
        assert_eq!(parse_duration("20m"), Some(1200));
        assert_eq!(parse_duration("1h30m"), Some(5400));
        assert_eq!(parse_duration("2x"), None);
        assert_eq!(time_limit(true, None, Some(100)), Some(600));
        assert_eq!(time_limit(true, None, Some(1000)), Some(3000));
        assert_eq!(time_limit(true, None, None), None);
        assert_eq!(time_limit(false, None, Some(1000)), None);
        assert_eq!(time_limit(false, Some(60), None), Some(60));
        assert_eq!(time_limit(true, Some(0), Some(100)), None);
    }

    #[test]
    fn clock_times_are_today_or_tomorrow() {
        let now = sched::now();
        let at = next_clock_time("03:00", now).unwrap();
        assert!(at > now && at <= now + 86400, "{at} vs {now}");
        assert!(next_clock_time("25:00", now).is_none());
        assert!(next_clock_time("tonight", now).is_none());
    }
}
