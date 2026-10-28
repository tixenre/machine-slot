//! The few macOS calls std doesn't wrap. Layouts are from the SDK headers
//! (sys/resource.h, libproc.h); the tests check them against real processes.

use std::os::raw::{c_int, c_void};

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct Timeval {
    sec: i64,
    usec: i32,
}

/// `struct rusage`: two timevals, then 14 longs (ru_maxrss first, in bytes).
#[repr(C)]
#[derive(Default)]
struct Rusage {
    utime: Timeval,
    stime: Timeval,
    maxrss: i64,
    rest: [i64; 13],
}

/// `struct rusage_info_v0`.
#[repr(C)]
#[derive(Default)]
struct RusageInfoV0 {
    uuid: [u8; 16],
    user_time: u64,
    system_time: u64,
    pkg_idle_wkups: u64,
    interrupt_wkups: u64,
    pageins: u64,
    wired_size: u64,
    resident_size: u64,
    phys_footprint: u64,
    proc_start_abstime: u64,
    proc_exit_abstime: u64,
}

/// `struct rusage_info_v2`: v0 plus children's times and disk I/O.
#[repr(C)]
#[derive(Default)]
struct RusageInfoV2 {
    v0: RusageInfoV0,
    child_user_time: u64,
    child_system_time: u64,
    child_pkg_idle_wkups: u64,
    child_interrupt_wkups: u64,
    child_pageins: u64,
    child_elapsed_abstime: u64,
    diskio_bytesread: u64,
    diskio_byteswritten: u64,
}

#[repr(C)]
#[derive(Default)]
struct TimebaseInfo {
    numer: u32,
    denom: u32,
}

const RUSAGE_INFO_V2: c_int = 2;
const RUSAGE_CHILDREN: c_int = -1;
const RUSAGE_INFO_V0: c_int = 0;
pub const SIGINT: c_int = 2;
pub const SIGTERM: c_int = 15;
pub const SIGKILL: c_int = 9;

unsafe extern "C" {
    fn getrusage(who: c_int, usage: *mut Rusage) -> c_int;
    fn proc_pid_rusage(pid: c_int, flavor: c_int, buffer: *mut c_void) -> c_int;
    fn proc_listchildpids(ppid: c_int, buffer: *mut c_void, buffersize: c_int) -> c_int;
    fn kill(pid: c_int, sig: c_int) -> c_int;
    fn signal(sig: c_int, handler: extern "C" fn(c_int)) -> usize;
    fn getpid() -> c_int;
    fn mach_timebase_info(info: *mut TimebaseInfo) -> c_int;
    fn getloadavg(loadavg: *mut f64, nelem: c_int) -> c_int;
    fn sysctlbyname(
        name: *const u8,
        oldp: *mut c_void,
        oldlenp: *mut usize,
        newp: *mut c_void,
        newlen: usize,
    ) -> c_int;
}

unsafe extern "C" {
    fn mach_host_self() -> u32;
    fn host_statistics(host: u32, flavor: c_int, info: *mut u32, count: *mut u32) -> c_int;
}

const HOST_CPU_LOAD_INFO: c_int = 3;

/// Whole-machine CPU ticks: (busy, total), user + system + nice as busy.
pub fn cpu_ticks() -> Option<(u64, u64)> {
    // struct host_cpu_load_info { natural_t cpu_ticks[4]; } user, system, idle, nice.
    let mut t = [0u32; 4];
    let mut count = 4u32;
    // SAFETY: t holds HOST_CPU_LOAD_INFO_COUNT natural_t's; count says so.
    let r = unsafe {
        host_statistics(
            mach_host_self(),
            HOST_CPU_LOAD_INFO,
            t.as_mut_ptr(),
            &mut count,
        )
    };
    (r == 0).then(|| {
        let busy = t[0] as u64 + t[1] as u64 + t[3] as u64;
        (busy, busy + t[2] as u64)
    })
}

pub fn sysctl_int(name: &str) -> Option<i32> {
    let cname = std::ffi::CString::new(name).ok()?;
    let mut n: i32 = 0;
    let mut len = std::mem::size_of::<i32>();
    // SAFETY: an int-valued sysctl; n and len describe the buffer.
    let r = unsafe {
        sysctlbyname(
            cname.as_ptr() as *const u8,
            &mut n as *mut _ as *mut c_void,
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    (r == 0).then_some(n)
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGMainDisplayID() -> u32;
    fn CGDisplayIsAsleep(display: u32) -> u32;
    fn CGSessionCopyCurrentDictionary() -> *const c_void;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFStringCreateWithCString(
        alloc: *const c_void,
        s: *const std::os::raw::c_char,
        encoding: u32,
    ) -> *const c_void;
    fn CFDictionaryGetValue(dict: *const c_void, key: *const c_void) -> *const c_void;
    fn CFBooleanGetValue(b: *const c_void) -> u8;
    fn CFGetTypeID(cf: *const c_void) -> usize;
    fn CFBooleanGetTypeID() -> usize;
    fn CFRelease(cf: *const c_void);
}

pub fn display_asleep() -> bool {
    // SAFETY: plain CoreGraphics queries.
    unsafe { CGDisplayIsAsleep(CGMainDisplayID()) != 0 }
}

/// The login session's screen is locked (or there is no GUI session).
pub fn screen_locked() -> bool {
    // SAFETY: Copy rule: the dictionary and the key are ours to release;
    // the value is borrowed from the dictionary.
    unsafe {
        let dict = CGSessionCopyCurrentDictionary();
        if dict.is_null() {
            return true;
        }
        let key = CFStringCreateWithCString(
            std::ptr::null(),
            c"CGSSessionScreenIsLocked".as_ptr(),
            0x0800_0100,
        );
        let v = CFDictionaryGetValue(dict, key);
        let locked =
            !v.is_null() && CFGetTypeID(v) == CFBooleanGetTypeID() && CFBooleanGetValue(v) != 0;
        CFRelease(key);
        CFRelease(dict);
        locked
    }
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
    sysctl_int("hw.ncpu")
        .filter(|&n| n > 0)
        .map_or(1, |n| n as u32)
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
    (secs(r.utime), secs(r.stime), r.maxrss.max(0) as u64)
}

/// Physical footprint of one process, in bytes (what Activity Monitor calls
/// Memory); None once it's gone.
pub fn footprint(pid: i32) -> Option<u64> {
    let mut info = RusageInfoV0::default();
    // SAFETY: info is a writable rusage_info_v0, the flavor asked for.
    let r = unsafe { proc_pid_rusage(pid, RUSAGE_INFO_V0, &mut info as *mut _ as *mut c_void) };
    (r == 0).then_some(info.phys_footprint)
}

/// One process's counters: CPU (ns, user + system), physical footprint and
/// disk bytes read / written, all since it started.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ProcUsage {
    pub cpu_ns: u64,
    pub footprint: u64,
    pub disk_read: u64,
    pub disk_written: u64,
}

pub fn usage(pid: i32) -> Option<ProcUsage> {
    let mut info = RusageInfoV2::default();
    // SAFETY: info is a writable rusage_info_v2, the flavor asked for.
    let r = unsafe { proc_pid_rusage(pid, RUSAGE_INFO_V2, &mut info as *mut _ as *mut c_void) };
    if r != 0 {
        return None;
    }
    // ri_*_time are in mach absolute time units.
    let mut tb = TimebaseInfo::default();
    // SAFETY: tb is a writable mach_timebase_info.
    unsafe { mach_timebase_info(&mut tb) };
    let ticks = info.v0.user_time + info.v0.system_time;
    let ns = if tb.denom == 0 {
        ticks
    } else {
        (ticks as u128 * tb.numer as u128 / tb.denom as u128) as u64
    };
    Some(ProcUsage {
        cpu_ns: ns,
        footprint: info.v0.phys_footprint,
        disk_read: info.diskio_bytesread,
        disk_written: info.diskio_byteswritten,
    })
}

/// The direct children of `pid`.
pub fn children(pid: i32) -> Vec<i32> {
    let mut buf = vec![0i32; 256];
    loop {
        let bytes = (buf.len() * 4) as c_int;
        // SAFETY: buf has `bytes` writable bytes.
        let n = unsafe { proc_listchildpids(pid, buf.as_mut_ptr() as *mut c_void, bytes) };
        if n <= 0 {
            return Vec::new();
        }
        // libproc returns a count of pids; a full buffer may have more.
        let n = n as usize;
        if n < buf.len() {
            buf.truncate(n);
            return buf.into_iter().filter(|&p| p > 0).collect();
        }
        buf.resize(buf.len() * 2, 0);
    }
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
