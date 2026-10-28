//! Linux: everything comes from /proc and /sys. There is no general way to
//! read GPU use or whether someone is at the keyboard, so `gpu` and the
//! input part of `screen` are not checked here.

use super::ProcUsage;
use std::fs;
use std::os::raw::{c_int, c_long};

unsafe extern "C" {
    fn sysconf(name: c_int) -> c_long;
}

const SC_CLK_TCK: c_int = 2;

/// Clock ticks per second, the unit of /proc's CPU times.
fn clk_tck() -> u64 {
    // SAFETY: sysconf only reads a constant.
    let t = unsafe { sysconf(SC_CLK_TCK) };
    if t > 0 { t as u64 } else { 100 }
}

/// The fields of /proc/<pid>/stat after the command name, so `rest[0]` is
/// field 3 (state). The name is in parentheses and may hold spaces.
fn stat_fields(pid: i32) -> Option<Vec<u64>> {
    let text = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let rest = &text[text.rfind(')')? + 1..];
    Some(
        rest.split_whitespace()
            .map(|w| w.parse().unwrap_or(0))
            .collect(),
    )
}

/// Whole-machine CPU ticks: (busy, total), from the first line of /proc/stat.
pub fn cpu_ticks() -> Option<(u64, u64)> {
    let text = fs::read_to_string("/proc/stat").ok()?;
    let line = text.lines().next()?.strip_prefix("cpu ")?;
    let t: Vec<u64> = line
        .split_whitespace()
        .map(|w| w.parse().unwrap_or(0))
        .collect();
    // user nice system idle iowait irq softirq steal
    let at = |i: usize| t.get(i).copied().unwrap_or(0);
    let busy = at(0) + at(1) + at(2) + at(5) + at(6) + at(7);
    Some((busy, busy + at(3) + at(4)))
}

/// Resident memory of one process, in bytes; None once it's gone.
pub fn footprint(pid: i32) -> Option<u64> {
    let status = fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    status.lines().find_map(|l| {
        let kb: u64 = l
            .strip_prefix("VmRSS:")?
            .split_whitespace()
            .next()?
            .parse()
            .ok()?;
        Some(kb * 1024)
    })
}

pub fn usage(pid: i32) -> Option<ProcUsage> {
    let f = stat_fields(pid)?;
    let ns = |ticks: u64| ticks * 1_000_000_000 / clk_tck();
    let at = |i: usize| f.get(i).copied().unwrap_or(0);
    // Fields 14–17: utime stime cutime cstime.
    let (utime, stime, cutime, cstime) = (at(11), at(12), at(13), at(14));
    let io = fs::read_to_string(format!("/proc/{pid}/io")).unwrap_or_default();
    let io_field = |name: &str| {
        io.lines()
            .find_map(|l| l.strip_prefix(name)?.trim().parse().ok())
            .unwrap_or(0)
    };
    Some(ProcUsage {
        cpu_ns: ns(utime + stime),
        child_cpu_ns: ns(cutime + cstime),
        footprint: footprint(pid).unwrap_or(0),
        disk_read: io_field("read_bytes:"),
        disk_written: io_field("write_bytes:"),
    })
}

/// The direct children of `pid`: from each thread's `children` list, or by
/// scanning every process's parent where that list isn't available.
pub fn children(pid: i32) -> Vec<i32> {
    if let Ok(tasks) = fs::read_dir(format!("/proc/{pid}/task")) {
        let mut kids = Vec::new();
        let mut listed = false;
        for t in tasks.flatten() {
            if let Ok(text) = fs::read_to_string(t.path().join("children")) {
                listed = true;
                kids.extend(
                    text.split_whitespace()
                        .filter_map(|w| w.parse::<i32>().ok()),
                );
            }
        }
        if listed {
            return kids;
        }
    }
    let Ok(all) = fs::read_dir("/proc") else {
        return Vec::new();
    };
    all.flatten()
        .filter_map(|e| e.file_name().to_str()?.parse::<i32>().ok())
        .filter(|&p| stat_fields(p).is_some_and(|f| f.get(1).copied() == Some(pid as u64)))
        .collect()
}

/// Memory pressure from PSI: 1 normal, 2 warning (tasks stalled on memory
/// over 10 % of the last 10 s), 4 critical (all tasks stalled over 5 %).
pub fn memory_pressure() -> i32 {
    let Ok(text) = fs::read_to_string("/proc/pressure/memory") else {
        return 1;
    };
    let avg10 = |kind: &str| -> f64 {
        text.lines()
            .find(|l| l.starts_with(kind))
            .and_then(|l| l.split_whitespace().find_map(|w| w.strip_prefix("avg10=")))
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.0)
    };
    if avg10("full") > 5.0 {
        4
    } else if avg10("some") > 10.0 {
        2
    } else {
        1
    }
}

/// Bytes read + written by the real disks since boot (/proc/diskstats;
/// partitions and virtual devices on top of disks are left out so nothing
/// counts twice).
pub fn disk_bytes() -> Option<u64> {
    let text = fs::read_to_string("/proc/diskstats").ok()?;
    let disks: Vec<String> = fs::read_dir("/sys/block")
        .ok()?
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| {
            !["loop", "ram", "dm-", "zram", "md", "sr"]
                .iter()
                .any(|v| n.starts_with(v))
        })
        .collect();
    let mut total = 0u64;
    let mut any = false;
    for l in text.lines() {
        let f: Vec<&str> = l.split_whitespace().collect();
        if f.len() < 10 || !disks.iter().any(|d| d == f[2]) {
            continue;
        }
        let sectors = |i: usize| f[i].parse::<u64>().unwrap_or(0);
        total += (sectors(5) + sectors(9)) * 512;
        any = true;
    }
    any.then_some(total)
}

pub fn gpu_util() -> Option<u32> {
    None
}

pub fn input_idle_s() -> Option<f64> {
    None
}

pub fn display_asleep() -> bool {
    false
}

pub fn screen_locked() -> bool {
    false
}
