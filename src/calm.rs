//! Before a solo starts: is the rest of the Mac quiet in what it needs?
//! cpu (whole-machine busy share), disk (MB/s, iostat), ram (the kernel's
//! memory pressure level), gpu (IOAccelerator utilization), screen (display
//! awake, session unlocked, nobody on the mouse or keyboard). Nothing here
//! closes or pauses anything.

use crate::sys;
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Need {
    Cpu,
    Disk,
    Ram,
    Gpu,
    Screen,
}

impl Need {
    pub fn name(self) -> &'static str {
        match self {
            Need::Cpu => "cpu",
            Need::Disk => "disk",
            Need::Ram => "ram",
            Need::Gpu => "gpu",
            Need::Screen => "screen",
        }
    }
    pub fn parse_list(s: &str) -> Result<Vec<Need>, String> {
        s.split(',')
            .filter(|w| !w.is_empty())
            .map(|w| match w.trim() {
                "cpu" => Ok(Need::Cpu),
                "disk" => Ok(Need::Disk),
                "ram" => Ok(Need::Ram),
                "gpu" => Ok(Need::Gpu),
                "screen" => Ok(Need::Screen),
                other => Err(format!(
                    "unknown need «{other}» (cpu, disk, ram, gpu, screen)"
                )),
            })
            .collect()
    }
    pub fn list(needs: &[Need]) -> String {
        needs.iter().map(|n| n.name()).collect::<Vec<_>>().join(",")
    }
}

pub const DEFAULT: &[Need] = &[Need::Cpu, Need::Disk, Need::Ram];

/// Quiet means: CPU under 15 % of the machine, disk under 5 MB/s, memory
/// pressure normal, GPU under 10 %, and for the screen: awake, unlocked and
/// 10 s without input.
const CPU_MAX: f64 = 0.15;
const DISK_MAX_MBS: f64 = 5.0;
const GPU_MAX: u32 = 10;
const INPUT_IDLE_S: f64 = 10.0;

/// One second of looking. Returns what wasn't quiet, with its reading.
pub struct Probe {
    prev_cpu: Option<(u64, u64)>,
}

impl Probe {
    pub fn new() -> Probe {
        Probe {
            prev_cpu: sys::cpu_ticks(),
        }
    }

    /// Takes about a second (iostat's own interval, or a sleep).
    pub fn look(&mut self, needs: &[Need]) -> Vec<String> {
        let mut busy = Vec::new();
        if needs.contains(&Need::Disk) {
            if let Some(mbs) = disk_mbs()
                && mbs > DISK_MAX_MBS
            {
                busy.push(format!("disk {mbs:.0} MB/s"));
            }
        } else {
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
        let cur = sys::cpu_ticks();
        if needs.contains(&Need::Cpu)
            && let (Some(p), Some(c)) = (self.prev_cpu, cur)
        {
            let share = (c.0 - p.0) as f64 / (c.1 - p.1).max(1) as f64;
            if share > CPU_MAX {
                busy.push(format!("cpu {:.0} %", share * 100.0));
            }
        }
        self.prev_cpu = cur;
        if needs.contains(&Need::Ram) {
            let level = sys::sysctl_int("kern.memorystatus_vm_pressure_level").unwrap_or(1);
            if level > 1 {
                busy.push(format!(
                    "memory pressure {}",
                    if level >= 4 { "critical" } else { "warning" }
                ));
            }
        }
        if needs.contains(&Need::Gpu)
            && let Some(u) = gpu_util()
            && u > GPU_MAX
        {
            busy.push(format!("gpu {u} %"));
        }
        if needs.contains(&Need::Screen) {
            busy.extend(screen_busy());
        }
        busy
    }
}

/// Sum of MB/s over the disks iostat lists, over one second.
fn disk_mbs() -> Option<f64> {
    let out = Command::new("iostat")
        .args(["-d", "-c", "2", "-w", "1"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    // Columns come in threes per disk: KB/t tps MB/s. The last line is the
    // second sample, the one over the last second.
    let last = text.lines().last()?;
    let nums: Vec<f64> = last
        .split_whitespace()
        .filter_map(|w| w.parse().ok())
        .collect();
    (!nums.is_empty() && nums.len().is_multiple_of(3)).then(|| nums.chunks(3).map(|c| c[2]).sum())
}

fn gpu_util() -> Option<u32> {
    let out = Command::new("ioreg")
        .args(["-r", "-d", "1", "-c", "IOAccelerator"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.split("\"Device Utilization %\"=")
        .skip(1)
        .filter_map(|s| {
            s.chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
                .parse()
                .ok()
        })
        .max()
}

/// Seconds since the last mouse or keyboard event.
fn input_idle_s() -> Option<f64> {
    let out = Command::new("ioreg")
        .args(["-c", "IOHIDSystem", "-d", "4"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let ns: f64 = text
        .split("\"HIDIdleTime\" = ")
        .nth(1)?
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .ok()?;
    Some(ns / 1e9)
}

fn screen_busy() -> Vec<String> {
    let mut busy = Vec::new();
    if sys::display_asleep() {
        busy.push("display asleep".into());
    }
    if sys::screen_locked() {
        busy.push("screen locked".into());
    }
    if let Some(idle) = input_idle_s()
        && idle < INPUT_IDLE_S
    {
        busy.push(format!(
            "someone is using the mouse/keyboard ({idle:.0} s ago)"
        ));
    }
    busy
}

/// For the record: thermal pressure and low power mode, as pmset says.
pub fn power_note() -> String {
    let therm = Command::new("pmset")
        .args(["-g", "therm"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();
    let mut notes = Vec::new();
    if therm
        .lines()
        .any(|l| l.contains("CPU_Speed_Limit") && !l.trim_end().ends_with("100"))
        || therm.contains("thermal warning level") && !therm.contains("No thermal warning")
    {
        notes.push("thermal");
    }
    let pm = Command::new("pmset")
        .arg("-g")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();
    if pm
        .lines()
        .any(|l| l.contains("lowpowermode") && l.trim_end().ends_with('1'))
    {
        notes.push("lowpower");
    }
    let batt = Command::new("pmset")
        .args(["-g", "batt"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();
    if batt.contains("Battery Power") {
        notes.push("battery");
    }
    notes.join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn needs_parse() {
        assert_eq!(
            Need::parse_list("cpu,gpu,screen").unwrap(),
            vec![Need::Cpu, Need::Gpu, Need::Screen]
        );
        assert!(Need::parse_list("cpu,vram").is_err());
        assert_eq!(Need::list(DEFAULT), "cpu,disk,ram");
    }

    #[test]
    fn probes_read_this_mac() {
        assert!(disk_mbs().is_some(), "iostat");
        assert!(gpu_util().is_some(), "ioreg IOAccelerator");
        assert!(input_idle_s().is_some(), "ioreg HIDIdleTime");
        assert!(sys::sysctl_int("kern.memorystatus_vm_pressure_level").is_some());
        let mut p = Probe::new();
        let _ = p.look(&[Need::Cpu, Need::Ram, Need::Gpu]);
    }
}
