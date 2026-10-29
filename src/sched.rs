//! Who may run now. Every job is a file in `jobs/` that its owner keeps
//! flock'ed for as long as it lives, so a crashed or killed job frees its
//! place by itself. Every decision is taken under `gate.lock`.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A solo waiting its turn in the queue.
    Queued,
    /// The solo whose turn it is, waiting for running shared work to end.
    Draining,
    Solo,
    /// A solo between two parts of its work (`machine-slot yield`): shared
    /// work that fits before `yield_until` may run meanwhile.
    Yielding,
    Shared,
    /// A shared waiting to start (not shown as running).
    Waiting,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Queued => "queued",
            Kind::Draining => "draining",
            Kind::Solo => "solo",
            Kind::Yielding => "yielding",
            Kind::Shared => "shared",
            Kind::Waiting => "waiting",
        }
    }
    fn parse(s: &str) -> Option<Kind> {
        Some(match s {
            "queued" => Kind::Queued,
            "draining" => Kind::Draining,
            "solo" => Kind::Solo,
            "yielding" => Kind::Yielding,
            "shared" => Kind::Shared,
            "waiting" => Kind::Waiting,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Job {
    pub id: String,
    pub kind: Kind,
    pub label: String,
    pub pid: i32,
    pub ticket: u64,
    /// When it started running (or queued, while waiting).
    pub start: u64,
    pub usual: Option<u64>,
    pub loaded: bool,
    /// What the command itself reports through `MACHINE_SLOT_PROGRESS`,
    /// 0 to 1; None when it doesn't.
    pub progress: Option<f64>,
    /// While it runs: CPU % (of one core), memory MB and disk MB/s of its
    /// whole process tree, refreshed every second.
    pub live: Option<(f64, u64, f64)>,
    /// While it runs: seconds its tree has used no CPU and no disk.
    pub quiet_s: Option<u64>,
    /// While it runs with a time limit: when it would be stopped.
    pub limit_at: Option<u64>,
    /// A solo waiting for a quiet machine: since when, and what isn't quiet.
    pub settling: Option<(u64, String)>,
    /// Where it was started from: «folder · branch», or MACHINE_SLOT_OWNER.
    pub owner: String,
    /// A yielding solo takes the machine back at this time.
    pub yield_until: Option<u64>,
}

impl Job {
    fn to_text(&self) -> String {
        format!(
            "kind={}\nlabel={}\npid={}\nticket={}\nstart={}\nusual={}\nloaded={}\nowner={}\nyield_until={}\n",
            self.kind.name(),
            self.label.replace('\n', " "),
            self.pid,
            self.ticket,
            self.start,
            self.usual.map(|u| u.to_string()).unwrap_or_default(),
            self.loaded as u8,
            self.owner.replace('\n', " "),
            self.yield_until.map(|u| u.to_string()).unwrap_or_default()
        )
    }
    /// A job of nobody's, for asking "what if one joined now".
    pub fn blank() -> Job {
        Job {
            id: String::new(),
            kind: Kind::Waiting,
            label: String::new(),
            pid: 0,
            ticket: 0,
            start: now(),
            usual: None,
            loaded: false,
            progress: None,
            live: None,
            limit_at: None,
            quiet_s: None,
            settling: None,
            owner: String::new(),
            yield_until: None,
        }
    }
    fn from_text(id: &str, text: &str) -> Option<Job> {
        let m: BTreeMap<&str, &str> = text.lines().filter_map(|l| l.split_once('=')).collect();
        Some(Job {
            id: id.to_string(),
            kind: Kind::parse(m.get("kind")?)?,
            label: m.get("label").unwrap_or(&"").to_string(),
            pid: m.get("pid")?.parse().ok()?,
            ticket: m.get("ticket")?.parse().ok()?,
            start: m.get("start")?.parse().ok()?,
            usual: m.get("usual").and_then(|s| s.parse().ok()),
            loaded: m.get("loaded") == Some(&"1"),
            progress: None,
            live: None,
            limit_at: None,
            quiet_s: None,
            settling: None,
            owner: m.get("owner").unwrap_or(&"").to_string(),
            yield_until: m.get("yield_until").and_then(|s| s.parse().ok()),
        })
    }
}

/// «37», «37 %», «0.37» or «120/300»: the last line wins.
pub fn parse_progress(text: &str) -> Option<f64> {
    let line = text.lines().rev().find(|l| !l.trim().is_empty())?.trim();
    let v = if let Some((done, total)) = line.split_once('/') {
        done.trim().parse::<f64>().ok()? / total.trim().parse::<f64>().ok().filter(|t| *t > 0.0)?
    } else {
        let n: f64 = line.trim_end_matches('%').trim().parse().ok()?;
        if line.ends_with('%') || n > 1.0 {
            n / 100.0
        } else {
            n
        }
    };
    v.is_finite().then_some(v.clamp(0.0, 1.0))
}

#[derive(Debug, PartialEq, Eq)]
pub enum Decision {
    Go,
    Wait,
    /// A solo whose turn has come: it now waits for shared work to drain.
    Drain,
}

/// The whole policy, on a snapshot of the live jobs (`me` included).
/// - A solo runs alone: it waits for its turn in ticket order, then for the
///   running shared work to end; nothing starts while it runs.
/// - A shared runs alongside other shared work. While a solo is queued or
///   draining, it starts only if its usual time ends before the running
///   shared work should; without history it waits.
pub fn decide(jobs: &[Job], me: &Job, now: u64) -> Decision {
    let others = || jobs.iter().filter(|j| j.id != me.id);
    match me.kind {
        Kind::Queued => {
            let turn_taken =
                others().any(|j| matches!(j.kind, Kind::Draining | Kind::Solo | Kind::Yielding));
            let first = others()
                .filter(|j| j.kind == Kind::Queued)
                .all(|j| j.ticket > me.ticket);
            if !turn_taken && first {
                Decision::Drain
            } else {
                Decision::Wait
            }
        }
        Kind::Draining => {
            if others().any(|j| j.kind == Kind::Shared) {
                Decision::Wait
            } else {
                Decision::Go
            }
        }
        Kind::Waiting => {
            if others().any(|j| j.kind == Kind::Solo) {
                return Decision::Wait;
            }
            // A solo between two parts: only work that ends before it takes
            // the machine back.
            if let Some(y) = others().find(|j| j.kind == Kind::Yielding) {
                return match (me.usual, y.yield_until) {
                    (Some(mine), Some(until)) if now + mine <= until => Decision::Go,
                    _ => Decision::Wait,
                };
            }
            if !others().any(|j| matches!(j.kind, Kind::Queued | Kind::Draining)) {
                return Decision::Go;
            }
            let Some(mine) = me.usual else {
                return Decision::Wait;
            };
            let running: Vec<&Job> = others().filter(|j| j.kind == Kind::Shared).collect();
            let mut ends = 0;
            for j in &running {
                let Some(u) = j.usual else {
                    return Decision::Wait;
                };
                ends = ends.max(j.start + u);
            }
            if !running.is_empty() && now + mine <= ends {
                Decision::Go
            } else {
                Decision::Wait
            }
        }
        Kind::Solo | Kind::Shared | Kind::Yielding => Decision::Go,
    }
}

pub struct Slot {
    dir: PathBuf,
    path: PathBuf,
    /// Held exclusively for the job's whole life: its liveness.
    _lock: File,
    pub job: Job,
}

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn gate(dir: &Path) -> std::io::Result<File> {
    let f = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("gate.lock"))?;
    f.lock()?;
    Ok(f)
}

/// The live jobs; files whose owner is gone are removed. Call under the gate.
pub fn live(dir: &Path) -> Vec<Job> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(dir.join("jobs")) else {
        return out;
    };
    for e in entries.flatten() {
        let path = e.path();
        if path.extension().is_none_or(|x| x != "job") {
            continue;
        }
        let Ok(f) = File::open(&path) else { continue };
        // Getting the lock means nobody holds it: the owner is dead.
        if f.try_lock().is_ok() {
            for ext in [
                "job", "progress", "live", "calm", "go", "yield", "limit", "extend",
            ] {
                let _ = fs::remove_file(path.with_extension(ext));
            }
            continue;
        }
        let id = path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        if let Some(mut j) = fs::read_to_string(&path)
            .ok()
            .and_then(|t| Job::from_text(&id, &t))
        {
            j.progress = fs::read_to_string(path.with_extension("progress"))
                .ok()
                .and_then(|t| parse_progress(&t));
            j.settling = fs::read_to_string(path.with_extension("calm"))
                .ok()
                .and_then(|t| {
                    let (since, what) = t.trim().split_once('\t')?;
                    Some((since.parse().ok()?, what.to_string()))
                });
            j.limit_at = fs::read_to_string(path.with_extension("limit"))
                .ok()
                .and_then(|t| t.trim().parse().ok());
            j.quiet_s = fs::read_to_string(path.with_extension("live"))
                .ok()
                .and_then(|t| t.trim().split('\t').nth(3)?.parse().ok());
            j.live = fs::read_to_string(path.with_extension("live"))
                .ok()
                .and_then(|t| {
                    let mut f = t.trim().split('\t');
                    Some((
                        f.next()?.parse().ok()?,
                        f.next()?.parse().ok()?,
                        f.next()?.parse().ok()?,
                    ))
                });
            out.push(j);
        }
    }
    out.sort_by_key(|j| (j.ticket, j.start));
    out
}

/// A snapshot without taking the gate (for status, the bar, the MCP).
pub fn peek(dir: &Path) -> Vec<Job> {
    match gate(dir) {
        Ok(_g) => live(dir),
        Err(_) => Vec::new(),
    }
}

impl Slot {
    /// Registers a job (a queued solo or a waiting shared).
    pub fn enter(
        dir: &Path,
        solo: bool,
        label: &str,
        usual: Option<(u64, bool)>,
        owner: &str,
    ) -> std::io::Result<Slot> {
        fs::create_dir_all(dir.join("jobs"))?;
        let _g = gate(dir)?;
        let seq_path = dir.join("seq");
        let ticket = fs::read_to_string(&seq_path)
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
            .unwrap_or(0)
            + 1;
        fs::write(&seq_path, ticket.to_string())?;
        let pid = crate::sys::me();
        let id = format!("{pid}-{ticket}");
        let path = dir.join("jobs").join(format!("{id}.job"));
        let lock = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&path)?;
        lock.lock()?;
        let job = Job {
            id,
            kind: if solo { Kind::Queued } else { Kind::Waiting },
            label: label.to_string(),
            pid,
            ticket,
            start: now(),
            usual: usual.map(|u| u.0),
            loaded: usual.is_some_and(|u| u.1),
            progress: None,
            live: None,
            limit_at: None,
            quiet_s: None,
            settling: None,
            owner: owner.to_string(),
            yield_until: None,
        };
        let slot = Slot {
            dir: dir.to_path_buf(),
            path,
            _lock: lock,
            job,
        };
        slot.write()?;
        Ok(slot)
    }

    fn write(&self) -> std::io::Result<()> {
        // Rewritten in place through the locked handle's path; readers only
        // read under the gate, which the writer holds.
        let mut f = OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&self.path)?;
        f.write_all(self.job.to_text().as_bytes())
    }

    /// One look at the queue: true once this job may run.
    pub fn try_start(&mut self) -> std::io::Result<bool> {
        let _g = gate(&self.dir)?;
        let jobs = live(&self.dir);
        let now = now();
        match decide(&jobs, &self.job, now) {
            Decision::Wait => Ok(false),
            Decision::Drain => {
                self.job.kind = Kind::Draining;
                self.write()?;
                // It may already be free.
                Ok(match decide(&live(&self.dir), &self.job, now) {
                    Decision::Go => self.run_now()?,
                    _ => false,
                })
            }
            Decision::Go => self.run_now(),
        }
    }

    /// Starts now, whatever the queue says (`machine-slot go`).
    pub fn force_start(&mut self) -> std::io::Result<bool> {
        let _g = gate(&self.dir)?;
        self.run_now()
    }

    /// A running solo lends the machine until `until`, or takes it back.
    pub fn set_yield(&mut self, until: Option<u64>) -> std::io::Result<()> {
        let _g = gate(&self.dir)?;
        self.job.kind = if until.is_some() {
            Kind::Yielding
        } else {
            Kind::Solo
        };
        self.job.yield_until = until;
        self.write()
    }

    /// Asked to start now by `machine-slot go`.
    pub fn go_requested(&self) -> bool {
        self.path.with_extension("go").exists()
    }

    /// Where a `machine-slot yield` inside this solo asks for a pause.
    pub fn yield_path(&self) -> PathBuf {
        self.path.with_extension("yield")
    }

    /// The job file, for the command (`MACHINE_SLOT_JOB`).
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Running shared jobs right now, other than this one.
    pub fn shared_running(&self) -> u32 {
        peek(&self.dir)
            .iter()
            .filter(|j| j.id != self.job.id && j.kind == Kind::Shared)
            .count() as u32
    }

    /// Waiting shared jobs right now.
    pub fn shared_waiting(&self) -> Vec<Job> {
        peek(&self.dir)
            .into_iter()
            .filter(|j| j.kind == Kind::Waiting)
            .collect()
    }

    fn run_now(&mut self) -> std::io::Result<bool> {
        self.job.kind = if matches!(self.job.kind, Kind::Draining | Kind::Queued | Kind::Solo) {
            Kind::Solo
        } else {
            Kind::Shared
        };
        self.job.start = now();
        self.write()?;
        Ok(true)
    }

    /// The file the command may write its progress to (`MACHINE_SLOT_PROGRESS`).
    pub fn progress_path(&self) -> PathBuf {
        self.path.with_extension("progress")
    }

    /// Where a solo says what keeps the machine from being quiet.
    pub fn calm_path(&self) -> PathBuf {
        self.path.with_extension("calm")
    }

    /// Where the runner writes the live CPU / memory / disk of the command.
    pub fn live_path(&self) -> PathBuf {
        self.path.with_extension("live")
    }

    /// Other running jobs right now (for telling a clean run from a loaded one).
    pub fn others_running(&self) -> u32 {
        peek(&self.dir)
            .iter()
            .filter(|j| {
                j.id != self.job.id && matches!(j.kind, Kind::Solo | Kind::Shared | Kind::Yielding)
            })
            .count() as u32
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        let _ = fs::remove_file(self.progress_path());
        let _ = fs::remove_file(self.live_path());
        let _ = fs::remove_file(self.calm_path());
        let _ = fs::remove_file(self.path.with_extension("go"));
        let _ = fs::remove_file(self.yield_path());
        let _ = fs::remove_file(self.path.with_extension("limit"));
        let _ = fs::remove_file(self.path.with_extension("extend"));
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(id: &str, kind: Kind, ticket: u64, start: u64, usual: Option<u64>) -> Job {
        Job {
            id: id.into(),
            kind,
            label: id.into(),
            pid: 1,
            ticket,
            start,
            usual,
            loaded: false,
            progress: None,
            live: None,
            limit_at: None,
            quiet_s: None,
            settling: None,
            owner: String::new(),
            yield_until: None,
        }
    }

    #[test]
    fn a_yielding_solo_lets_in_only_what_fits() {
        let mut y = job("y", Kind::Yielding, 1, 0, Some(600));
        y.yield_until = Some(1000 + 120);
        let fits = job("a", Kind::Waiting, 2, 1000, Some(90));
        let long = job("b", Kind::Waiting, 3, 1000, Some(300));
        let unknown = job("c", Kind::Waiting, 4, 1000, None);
        let queued = job("q", Kind::Queued, 5, 1000, Some(10));
        let all = vec![
            y,
            fits.clone(),
            long.clone(),
            unknown.clone(),
            queued.clone(),
        ];
        assert_eq!(decide(&all, &fits, 1000), Decision::Go);
        assert_eq!(decide(&all, &long, 1000), Decision::Wait);
        assert_eq!(decide(&all, &unknown, 1000), Decision::Wait);
        assert_eq!(decide(&all, &queued, 1000), Decision::Wait);
    }

    #[test]
    fn shared_runs_together_when_no_solo_waits() {
        let a = job("a", Kind::Shared, 1, 0, None);
        let me = job("me", Kind::Waiting, 2, 0, None);
        assert_eq!(decide(&[a, me.clone()], &me, 5), Decision::Go);
    }

    #[test]
    fn nothing_starts_during_a_solo() {
        let s = job("s", Kind::Solo, 1, 0, Some(100));
        let me = job("me", Kind::Waiting, 2, 0, Some(1));
        assert_eq!(decide(&[s, me.clone()], &me, 5), Decision::Wait);
    }

    #[test]
    fn solos_go_in_ticket_order_and_drain_first() {
        let sh = job("sh", Kind::Shared, 1, 0, None);
        let q1 = job("q1", Kind::Queued, 2, 0, None);
        let q2 = job("q2", Kind::Queued, 3, 0, None);
        let all = [sh.clone(), q1.clone(), q2.clone()];
        assert_eq!(decide(&all, &q2, 1), Decision::Wait);
        assert_eq!(decide(&all, &q1, 1), Decision::Drain);
        let d1 = job("q1", Kind::Draining, 2, 0, None);
        assert_eq!(
            decide(&[sh, d1.clone(), q2.clone()], &d1, 1),
            Decision::Wait
        );
        assert_eq!(decide(&[d1.clone(), q2.clone()], &d1, 1), Decision::Go);
        assert_eq!(decide(&[d1, q2.clone()], &q2, 1), Decision::Wait);
    }

    #[test]
    fn short_shared_work_fits_in_the_drain_window() {
        // Running shared work should end at 0 + 10 = 10.
        let long = job("long", Kind::Shared, 1, 0, Some(10));
        let d = job("d", Kind::Draining, 2, 0, None);
        let short = job("short", Kind::Waiting, 3, 0, Some(3));
        let huge = job("huge", Kind::Waiting, 4, 0, Some(20));
        let fresh = job("fresh", Kind::Waiting, 5, 0, None);
        let all = [
            long.clone(),
            d.clone(),
            short.clone(),
            huge.clone(),
            fresh.clone(),
        ];
        assert_eq!(decide(&all, &short, 2), Decision::Go);
        assert_eq!(decide(&all, &short, 8), Decision::Wait, "no longer fits");
        assert_eq!(decide(&all, &huge, 2), Decision::Wait);
        assert_eq!(
            decide(&all, &fresh, 2),
            Decision::Wait,
            "no history, no squeezing in"
        );
        // Running work without history: no window to fit in.
        let blind = job("blind", Kind::Shared, 1, 0, None);
        assert_eq!(
            decide(&[blind, d.clone(), short.clone()], &short, 2),
            Decision::Wait
        );
        // Nothing running: the solo is about to start, nothing squeezes in.
        assert_eq!(decide(&[d, short.clone()], &short, 2), Decision::Wait);
    }

    #[test]
    fn progress_reads_the_usual_ways_of_saying_it() {
        assert_eq!(parse_progress("37\n"), Some(0.37));
        assert_eq!(parse_progress("10\n37 %\n"), Some(0.37));
        assert_eq!(parse_progress("0.5"), Some(0.5));
        assert_eq!(parse_progress("120/300"), Some(0.4));
        assert_eq!(parse_progress("400/300"), Some(1.0));
        assert_eq!(parse_progress("1/0"), None);
        assert_eq!(parse_progress("soon"), None);
    }

    #[test]
    fn job_files_round_trip() {
        let j = Job {
            id: "9-3".into(),
            kind: Kind::Draining,
            label: "engine: baseline".into(),
            pid: 9,
            ticket: 3,
            start: 42,
            usual: Some(60),
            loaded: true,
            progress: None,
            live: None,
            limit_at: None,
            quiet_s: None,
            settling: None,
            owner: String::new(),
            yield_until: None,
        };
        assert_eq!(Job::from_text("9-3", &j.to_text()), Some(j));
    }
}
