//! Before a solo starts: is the rest of the Mac quiet in what it needs?
//! cpu (whole-machine busy share), disk (MB/s of the block storage
//! drivers), ram (the kernel's memory pressure level), gpu (IOAccelerator
//! utilization), screen (display awake, session unlocked, nobody on the
//! mouse or keyboard). All are read in place, so waiting for quiet starts
//! no processes. Nothing here closes or pauses anything.

use crate::sys;
#[cfg(target_os = "macos")]
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
    prev_disk: Option<(u64, std::time::Instant)>,
}

impl Probe {
    pub fn new() -> Probe {
        Probe {
            prev_cpu: sys::cpu_ticks(),
            prev_disk: sys::disk_bytes().map(|b| (b, std::time::Instant::now())),
        }
    }

    /// Takes a second: every reading is over the second since the last.
    pub fn look(&mut self, needs: &[Need]) -> Vec<String> {
        std::thread::sleep(std::time::Duration::from_secs(1));
        self.look_now(needs)
    }

    /// The same over the time since the probe was made or last looked,
    /// without waiting (callers that already pace themselves).
    pub fn look_now(&mut self, needs: &[Need]) -> Vec<String> {
        let mut busy = Vec::new();
        let disk = sys::disk_bytes().map(|b| (b, std::time::Instant::now()));
        if needs.contains(&Need::Disk)
            && let (Some((b0, t0)), Some((b1, t1))) = (self.prev_disk, disk)
        {
            let mbs = b1.saturating_sub(b0) as f64 / 1048576.0 / (t1 - t0).as_secs_f64().max(0.1);
            if mbs > DISK_MAX_MBS {
                busy.push(format!("disk {mbs:.0} MB/s"));
            }
        }
        self.prev_disk = disk;
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
            let level = sys::memory_pressure();
            if level > 1 {
                busy.push(format!(
                    "memory pressure {}",
                    if level >= 4 { "critical" } else { "warning" }
                ));
            }
        }
        if needs.contains(&Need::Gpu)
            && let Some(u) = sys::gpu_util()
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

fn screen_busy() -> Vec<String> {
    let mut busy = Vec::new();
    if sys::display_asleep() {
        busy.push("display asleep".into());
    }
    if sys::screen_locked() {
        busy.push("screen locked".into());
    }
    if let Some(idle) = sys::input_idle_s()
        && idle < INPUT_IDLE_S
    {
        busy.push(format!(
            "someone is using the mouse/keyboard ({idle:.0} s ago)"
        ));
    }
    busy
}

/// For the record: thermal pressure, low power mode and battery, as pmset
/// says.
#[cfg(target_os = "macos")]
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

/// For the record: running on battery, as the kernel's power supplies say.
#[cfg(target_os = "linux")]
pub fn power_note() -> String {
    let on_battery = std::fs::read_dir("/sys/class/power_supply")
        .into_iter()
        .flatten()
        .flatten()
        .any(|e| {
            std::fs::read_to_string(e.path().join("status"))
                .is_ok_and(|s| s.trim() == "Discharging")
        });
    if on_battery {
        "battery".into()
    } else {
        String::new()
    }
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

    /// The value `ioreg` prints for a key, as the oracle.
    #[cfg(target_os = "macos")]
    fn ioreg(class: &str, key: &str) -> Option<i64> {
        let out = Command::new("ioreg")
            .args(["-r", "-c", class])
            .output()
            .ok()?;
        String::from_utf8_lossy(&out.stdout)
            .split(&format!("\"{key}\"")[..])
            .skip(1)
            .find_map(|s| {
                let s = s.trim_start_matches([' ', '=']);
                s.chars()
                    .take_while(char::is_ascii_digit)
                    .collect::<String>()
                    .parse()
                    .ok()
            })
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn probes_read_what_ioreg_shows() {
        let gpu = sys::gpu_util().expect("IOAccelerator");
        assert!(gpu <= 100);
        assert!(ioreg("IOAccelerator", "Device Utilization %").is_some());
        let idle = sys::input_idle_s().expect("HIDIdleTime");
        let oracle = ioreg("IOHIDSystem", "HIDIdleTime").unwrap() as f64 / 1e9;
        assert!((idle - oracle).abs() < 2.0, "{idle} vs ioreg {oracle}");
        // Writing 32 MB to disk shows in the drivers' byte counts.
        let before = sys::disk_bytes().expect("IOBlockStorageDriver");
        let path = std::env::temp_dir().join(format!("machine-slot-disk-{}", std::process::id()));
        {
            use std::io::Write;
            let mut f = std::fs::File::create(&path).unwrap();
            f.write_all(&vec![7u8; 32 << 20]).unwrap();
            f.sync_all().unwrap();
        }
        let after = sys::disk_bytes().unwrap();
        let _ = std::fs::remove_file(&path);
        assert!(after - before >= 30 << 20, "{} bytes", after - before);
    }

    #[test]
    fn probes_read_this_mac() {
        assert!([1, 2, 4].contains(&sys::memory_pressure()));
        assert!(sys::disk_bytes().is_some());
        let mut p = Probe::new();
        let _ = p.look(&[Need::Cpu, Need::Ram, Need::Gpu]);
    }
}
