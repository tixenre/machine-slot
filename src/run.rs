//! Runs the command in its own process group, samples the memory of its
//! whole tree and the other work around it, and stops it on Ctrl-C / TERM.

use crate::sys;
use std::collections::HashMap;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::Command;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{Duration, Instant};

static SIGNALED: AtomicI32 = AtomicI32::new(0);

extern "C" fn on_stop(sig: i32) {
    SIGNALED.store(sig, Ordering::SeqCst);
}

/// From the start, also while waiting in the queue: a stop asked for then
/// leaves cleanly instead of dying unrecorded.
pub fn catch_stops() {
    sys::on_signal(sys::SIGINT, on_stop);
    sys::on_signal(sys::SIGTERM, on_stop);
}

/// The signal asked to stop with, if any.
pub fn stop_requested() -> Option<i32> {
    let s = SIGNALED.load(Ordering::SeqCst);
    (s != 0).then_some(s)
}

pub struct Outcome {
    pub exit: i32,
    pub run_s: u64,
    pub cpu_user_s: f64,
    pub cpu_sys_s: f64,
    pub max_rss_mb: u64,
    pub peak_tree_mb: u64,
    pub concurrent_max: u32,
    /// Disk read + written by the whole tree, as last seen per process.
    pub disk_mb: u64,
    /// The busiest second, in % of one core (400 = four cores).
    pub peak_cpu_pct: u64,
    /// Stopped for running past its time limit.
    pub timed_out: bool,
}

/// Below this, a second of the tree counts as doing nothing (% of one core).
const QUIET_CPU_PCT: f64 = 1.0;

/// `others` says how many other slot jobs run right now; asked every few
/// seconds.
/// `progress` is handed to the command as `MACHINE_SLOT_PROGRESS`: a file it
/// may write «37», «37 %» or «120/300» to, for an exact progress bar.
/// `live` gets, every second, «cpu % \t memory MB \t disk MB/s \t quiet s»
/// of the tree, the last being how long it has done nothing.
/// Past `limit` the command is stopped as Ctrl-C would, and the outcome
/// says it timed out.
pub fn run(
    cmd: &[String],
    mode: &str,
    progress: &std::path::Path,
    live: &std::path::Path,
    limit: Option<Duration>,
    mut others: impl FnMut() -> u32,
) -> Outcome {
    catch_stops();
    let started = Instant::now();
    let mut child = match Command::new(&cmd[0])
        .args(&cmd[1..])
        .env("MACHINE_SLOT", mode)
        .env("MACHINE_SLOT_PROGRESS", progress)
        .process_group(0)
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("machine-slot: can't run {}: {e}", cmd[0]);
            return Outcome {
                exit: 127,
                run_s: 0,
                cpu_user_s: 0.0,
                cpu_sys_s: 0.0,
                max_rss_mb: 0,
                peak_tree_mb: 0,
                concurrent_max: 0,
                disk_mb: 0,
                peak_cpu_pct: 0,
                timed_out: false,
            };
        }
    };
    let pgid = child.id() as i32;
    let (mut peak, mut concurrent, mut stopping): (u64, u32, Option<Instant>) = (0, 0, None);
    let mut tick = 0u32;
    // Per process, the counters last seen: rates come from what changed.
    let mut seen: HashMap<i32, sys::ProcUsage> = HashMap::new();
    let (mut last_sample, mut peak_cpu) = (Instant::now(), 0f64);
    // CPU of the tree so far: each live process with its ended children.
    // A process that ends moves into its parent's children time, so the sum
    // only drops when the ended one's parent is outside the tree.
    let mut last_cpu: Option<u64> = None;
    let (mut quiet_since, mut timed_out) = (Instant::now(), false);
    let status = loop {
        if let Ok(Some(st)) = child.try_wait() {
            break st;
        }
        if tick.is_multiple_of(5) {
            let dt = last_sample.elapsed().as_secs_f64().max(0.05);
            last_sample = Instant::now();
            let (mut mem, mut cpu_total, mut dio) = (0u64, 0u64, 0u64);
            for pid in sys::tree(pgid) {
                let Some(u) = sys::usage(pid) else { continue };
                mem += u.footprint;
                cpu_total += u.cpu_ns + u.child_cpu_ns;
                if let Some(p) = seen.get(&pid) {
                    dio +=
                        (u.disk_read + u.disk_written).saturating_sub(p.disk_read + p.disk_written);
                }
                seen.insert(pid, u);
            }
            peak = peak.max(mem);
            let dcpu = last_cpu.map(|l| cpu_total.saturating_sub(l));
            last_cpu = Some(cpu_total);
            if let Some(dcpu) = dcpu {
                let cpu = dcpu as f64 / 1e9 / dt * 100.0;
                peak_cpu = peak_cpu.max(cpu);
                if cpu >= QUIET_CPU_PCT || dio > 0 {
                    quiet_since = Instant::now();
                }
                let line = format!(
                    "{cpu:.0}\t{}\t{:.1}\t{}\n",
                    mem >> 20,
                    dio as f64 / 1048576.0 / dt,
                    quiet_since.elapsed().as_secs()
                );
                let _ = std::fs::write(live, line);
            }
        }
        if tick.is_multiple_of(15) {
            concurrent = concurrent.max(others());
        }
        if !timed_out && limit.is_some_and(|l| started.elapsed() > l) {
            eprintln!(
                "machine-slot: past its time limit ({}s): stopping it",
                limit.unwrap().as_secs()
            );
            timed_out = true;
        }
        let sig = match SIGNALED.load(Ordering::SeqCst) {
            0 if timed_out => sys::SIGTERM,
            s => s,
        };
        if sig != 0 {
            match stopping {
                None => {
                    sys::send(-pgid, sig);
                    stopping = Some(Instant::now());
                }
                Some(t) if t.elapsed() > Duration::from_secs(10) => sys::send(-pgid, sys::SIGKILL),
                _ => {}
            }
        }
        tick += 1;
        std::thread::sleep(Duration::from_millis(200));
    };
    let _ = std::fs::remove_file(live);
    let disk: u64 = seen.values().map(|u| u.disk_read + u.disk_written).sum();
    let (user, system, rss) = sys::children_usage();
    let mb = |b: u64| (b + (1 << 19)) >> 20;
    Outcome {
        // 124, as timeout(1) says it.
        exit: if timed_out {
            124
        } else {
            status
                .code()
                .unwrap_or_else(|| 128 + status.signal().unwrap_or(0))
        },
        run_s: started.elapsed().as_secs_f64().round() as u64,
        cpu_user_s: user,
        cpu_sys_s: system,
        max_rss_mb: mb(rss),
        peak_tree_mb: mb(peak),
        concurrent_max: concurrent,
        disk_mb: mb(disk),
        peak_cpu_pct: peak_cpu.round() as u64,
        timed_out,
    }
}
