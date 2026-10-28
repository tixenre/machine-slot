//! The few OS calls std doesn't wrap: process trees and their CPU, memory
//! and disk, whole-machine CPU and disk, memory pressure, the GPU and the
//! screen. The common POSIX part is here; `macos` and `linux` read the rest
//! their own way. The tests check them against real processes.

use std::os::raw::c_int;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::*;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::*;

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct Timeval {
    sec: i64,
    usec: i32,
}

/// `struct rusage`: two timevals, then 14 longs (ru_maxrss first: bytes on
/// macOS, KiB on Linux). On Linux `usec` is a long; its low half, read here,
/// holds the value.
#[repr(C)]
#[derive(Default)]
struct Rusage {
    utime: Timeval,
    stime: Timeval,
    maxrss: i64,
    rest: [i64; 13],
}

const RUSAGE_CHILDREN: c_int = -1;
pub const SIGINT: c_int = 2;
pub const SIGTERM: c_int = 15;
pub const SIGKILL: c_int = 9;

unsafe extern "C" {
    fn getrusage(who: c_int, usage: *mut Rusage) -> c_int;
    fn kill(pid: c_int, sig: c_int) -> c_int;
    fn signal(sig: c_int, handler: extern "C" fn(c_int)) -> usize;
    fn getpid() -> c_int;
    fn getloadavg(loadavg: *mut f64, nelem: c_int) -> c_int;
}

/// One process's counters: CPU (ns, user + system), physical footprint and
/// disk bytes read / written, all since it started.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ProcUsage {
    pub cpu_ns: u64,
    /// CPU of its children that already ended and were waited for: a short
    /// `rustc` lives between two samples, and its time shows up here.
    pub child_cpu_ns: u64,
    pub footprint: u64,
    pub disk_read: u64,
    pub disk_written: u64,
}

/// The 1-minute load average.
pub fn load1() -> f64 {
    let mut l = [0f64; 3];
    // SAFETY: l holds the 3 doubles asked for.
    if unsafe { getloadavg(l.as_mut_ptr(), 3) } < 1 {
        return 0.0;
    }
    l[0]
}

/// Logical CPUs.
pub fn cpus() -> u32 {
    std::thread::available_parallelism().map_or(1, |n| n.get() as u32)
}

/// CPU seconds (user, system) and peak RSS (bytes) of every child this
/// process has waited for.
pub fn children_usage() -> (f64, f64, u64) {
    let mut r = Rusage::default();
    // SAFETY: r is a properly sized, writable struct rusage.
    if unsafe { getrusage(RUSAGE_CHILDREN, &mut r) } != 0 {
        return (0.0, 0.0, 0);
    }
    let secs = |t: Timeval| t.sec as f64 + t.usec as f64 / 1e6;
    let unit = if cfg!(target_os = "linux") { 1024 } else { 1 };
    (secs(r.utime), secs(r.stime), r.maxrss.max(0) as u64 * unit)
}

/// `pid` and everything under it.
pub fn tree(pid: i32) -> Vec<i32> {
    let mut all = vec![pid];
    let mut i = 0;
    while i < all.len() {
        let kids = children(all[i]);
        all.extend(kids);
        i += 1;
    }
    all
}

pub fn send(pid: i32, sig: c_int) {
    // SAFETY: plain kill(2); a gone pid only returns an error.
    unsafe { kill(pid, sig) };
}

pub fn me() -> i32 {
    // SAFETY: getpid never fails.
    unsafe { getpid() }
}

pub fn on_signal(sig: c_int, handler: extern "C" fn(c_int)) {
    // SAFETY: the handler only stores into an atomic.
    unsafe { signal(sig, handler) };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn rusage_sees_a_childs_cpu_and_memory() {
        // ~200 MB touched, a bit of CPU.
        let st = Command::new("python3")
            .args([
                "-c",
                "x = bytearray(200 * 1024 * 1024); s = sum(range(3 * 10**6))",
            ])
            .status()
            .unwrap();
        assert!(st.success());
        let (user, _sys, rss) = children_usage();
        assert!(user > 0.01, "user {user}");
        assert!(
            rss > 190 * 1024 * 1024 && rss < 2 * 1024 * 1024 * 1024,
            "rss {rss}"
        );
    }

    #[test]
    fn usage_counts_a_childs_cpu_and_disk() {
        let dir = std::env::temp_dir().join(format!("machine-slot-usage-{}", std::process::id()));
        let file = dir.join("w.bin");
        std::fs::create_dir_all(&dir).unwrap();
        // ~0.6 s of CPU, then 20 MB written and synced, then a pause to look.
        let script = format!(
            "import time, os\nt = time.process_time()\nwhile time.process_time() - t < 0.6: pass\nf = open({:?}, 'wb'); f.write(os.urandom(20 << 20)); f.flush(); os.fsync(f.fileno()); f.close()\ntime.sleep(1.5)",
            file.to_string_lossy()
        );
        let mut child = Command::new("python3")
            .args(["-c", &script])
            .spawn()
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1500));
        let u = usage(child.id() as i32).unwrap();
        child.wait().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            u.cpu_ns > 500_000_000 && u.cpu_ns < 3_000_000_000,
            "cpu {} ns",
            u.cpu_ns
        );
        assert!(u.disk_written >= 15 << 20, "written {}", u.disk_written);
        assert!(u.footprint > 1 << 20);
    }

    #[test]
    fn cpu_ticks_see_a_busy_core() {
        let busy_share = |work: bool| {
            let (b0, t0) = cpu_ticks().unwrap();
            let end = std::time::Instant::now() + std::time::Duration::from_millis(500);
            let mut x = 0u64;
            while std::time::Instant::now() < end {
                if work {
                    x = x.wrapping_mul(6364136223846793005).wrapping_add(1);
                } else {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
            }
            std::hint::black_box(x);
            let (b1, t1) = cpu_ticks().unwrap();
            (b1 - b0) as f64 / (t1 - t0).max(1) as f64
        };
        // One spinning core shows up as at least ~1/ncpu of the machine.
        let spin = busy_share(true);
        assert!(spin > 0.6 / cpus() as f64, "spin share {spin}");
        assert!((0.0..=1.0).contains(&spin));
    }

    #[test]
    fn footprint_and_children_of_a_live_tree() {
        let mut sh = Command::new("sh")
            .args(["-c", "sleep 5 & sleep 5 & wait"])
            .spawn()
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        let pid = sh.id() as i32;
        assert_eq!(children(pid).len(), 2);
        assert_eq!(tree(pid).len(), 3);
        let f = footprint(pid).unwrap();
        assert!(f > 100 * 1024 && f < 200 * 1024 * 1024, "footprint {f}");
        for p in tree(pid) {
            send(p, SIGKILL);
        }
        sh.wait().unwrap();
        assert!(footprint(pid).is_none());
    }
}
