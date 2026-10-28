//! macOS: libproc, mach, sysctl, CoreGraphics and IOKit. Layouts are from
//! the SDK headers (libproc.h, mach/host_info.h); the tests check them
//! against real processes and against `ioreg`.

use super::ProcUsage;
use std::os::raw::{c_int, c_void};

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
const RUSAGE_INFO_V0: c_int = 0;

unsafe extern "C" {
    fn proc_pid_rusage(pid: c_int, flavor: c_int, buffer: *mut c_void) -> c_int;
    fn proc_listchildpids(ppid: c_int, buffer: *mut c_void, buffersize: c_int) -> c_int;
    fn mach_timebase_info(info: *mut TimebaseInfo) -> c_int;
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
    fn CFNumberGetValue(n: *const c_void, kind: c_int, out: *mut c_void) -> u8;
    fn CFNumberGetTypeID() -> usize;
    fn CFDictionaryGetTypeID() -> usize;
}

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOServiceMatching(name: *const std::os::raw::c_char) -> *const c_void;
    fn IOServiceGetMatchingServices(port: u32, matching: *const c_void, it: *mut u32) -> c_int;
    fn IOIteratorNext(it: u32) -> u32;
    fn IOObjectRelease(obj: u32) -> c_int;
    fn IORegistryEntryCreateCFProperty(
        entry: u32,
        key: *const c_void,
        alloc: *const c_void,
        options: u32,
    ) -> *const c_void;
}

const UTF8: u32 = 0x0800_0100;
const CF_NUMBER_SINT64: c_int = 4;

/// A CFString for a key; the caller releases it.
unsafe fn cfstr(s: &std::ffi::CStr) -> *const c_void {
    // SAFETY: s is NUL-terminated.
    unsafe { CFStringCreateWithCString(std::ptr::null(), s.as_ptr(), UTF8) }
}

/// For every IOKit service of `class`: the number at `path` (a property,
/// then keys into dictionaries), read in place, with no `ioreg` process.
pub fn io_numbers(class: &std::ffi::CStr, path: &[&std::ffi::CStr]) -> Vec<i64> {
    let mut out = Vec::new();
    let Some((first, rest)) = path.split_first() else {
        return out;
    };
    // SAFETY: IOServiceGetMatchingServices consumes `matching`; every
    // service and the iterator are released; Create/Copy results are
    // released, Get results are borrowed from their dictionary.
    unsafe {
        let matching = IOServiceMatching(class.as_ptr());
        let mut it = 0u32;
        if matching.is_null() || IOServiceGetMatchingServices(0, matching, &mut it) != 0 {
            return out;
        }
        loop {
            let svc = IOIteratorNext(it);
            if svc == 0 {
                break;
            }
            let key = cfstr(first);
            let prop = IORegistryEntryCreateCFProperty(svc, key, std::ptr::null(), 0);
            CFRelease(key);
            if !prop.is_null() {
                let mut v = prop;
                for k in rest {
                    if v.is_null() || CFGetTypeID(v) != CFDictionaryGetTypeID() {
                        v = std::ptr::null();
                        break;
                    }
                    let key = cfstr(k);
                    v = CFDictionaryGetValue(v, key);
                    CFRelease(key);
                }
                let mut n = 0i64;
                if !v.is_null()
                    && CFGetTypeID(v) == CFNumberGetTypeID()
                    && CFNumberGetValue(v, CF_NUMBER_SINT64, &mut n as *mut i64 as *mut c_void) != 0
                {
                    out.push(n);
                }
                CFRelease(prop);
            }
            IOObjectRelease(svc);
        }
        IOObjectRelease(it);
    }
    out
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
        let key = cfstr(c"CGSSessionScreenIsLocked");
        let v = CFDictionaryGetValue(dict, key);
        let locked =
            !v.is_null() && CFGetTypeID(v) == CFBooleanGetTypeID() && CFBooleanGetValue(v) != 0;
        CFRelease(key);
        CFRelease(dict);
        locked
    }
}

/// Physical footprint of one process, in bytes (what Activity Monitor calls
/// Memory); None once it's gone.
pub fn footprint(pid: i32) -> Option<u64> {
    let mut info = RusageInfoV0::default();
    // SAFETY: info is a writable rusage_info_v0, the flavor asked for.
    let r = unsafe { proc_pid_rusage(pid, RUSAGE_INFO_V0, &mut info as *mut _ as *mut c_void) };
    (r == 0).then_some(info.phys_footprint)
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
    let ns = |ticks: u64| {
        if tb.denom == 0 {
            ticks
        } else {
            (ticks as u128 * tb.numer as u128 / tb.denom as u128) as u64
        }
    };
    Some(ProcUsage {
        cpu_ns: ns(info.v0.user_time + info.v0.system_time),
        child_cpu_ns: ns(info.child_user_time + info.child_system_time),
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

/// The kernel's memory pressure: 1 normal, 2 warning, 4 critical.
pub fn memory_pressure() -> i32 {
    sysctl_int("kern.memorystatus_vm_pressure_level").unwrap_or(1)
}

/// Bytes read + written by every disk since boot, from the block storage
/// drivers' statistics.
pub fn disk_bytes() -> Option<u64> {
    let read = io_numbers(c"IOBlockStorageDriver", &[c"Statistics", c"Bytes (Read)"]);
    let written = io_numbers(c"IOBlockStorageDriver", &[c"Statistics", c"Bytes (Write)"]);
    (!read.is_empty()).then(|| read.iter().chain(&written).map(|&b| b.max(0) as u64).sum())
}

/// GPU utilization, %, of the busiest accelerator.
pub fn gpu_util() -> Option<u32> {
    io_numbers(
        c"IOAccelerator",
        &[c"PerformanceStatistics", c"Device Utilization %"],
    )
    .into_iter()
    .max()
    .map(|u| u.clamp(0, 100) as u32)
}

/// Seconds since the last mouse or keyboard event.
pub fn input_idle_s() -> Option<f64> {
    io_numbers(c"IOHIDSystem", &[c"HIDIdleTime"])
        .into_iter()
        .min()
        .map(|ns| ns as f64 / 1e9)
}
