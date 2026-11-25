#[derive(Clone, Copy)]
pub(super) struct Snapshot {
    pub current: u64,
    pub peak: Option<u64>,
}

#[cfg(target_os = "macos")]
pub(super) const METRIC: &str = "physical_footprint";
#[cfg(target_os = "windows")]
pub(super) const METRIC: &str = "working_set";
#[cfg(target_os = "linux")]
pub(super) const METRIC: &str = "rss";
#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
pub(super) const METRIC: &str = "unavailable";

#[cfg(target_os = "macos")]
#[allow(deprecated)]
pub(super) fn snapshot() -> Option<Snapshot> {
    use std::mem::{offset_of, size_of};

    #[repr(C)]
    #[derive(Default)]
    struct TaskVmInfoRev3 {
        rev0: [u64; 18],
        phys_footprint: u64,
        address_range: [u64; 2],
        ledger_phys_footprint_peak: i64,
        other_ledgers: [i64; 20],
    }

    const TASK_VM_INFO: libc::task_flavor_t = 22;
    const WORD_BYTES: usize = size_of::<libc::natural_t>();
    const REV1_COUNT: usize = offset_of!(TaskVmInfoRev3, address_range) / WORD_BYTES;
    const REV3_COUNT: usize = size_of::<TaskVmInfoRev3>() / WORD_BYTES;
    let mut info = TaskVmInfoRev3::default();
    let mut count = REV3_COUNT as libc::mach_msg_type_number_t;
    // SAFETY: the initialized REV3 buffer is writable for the requested word count.
    let result = unsafe {
        libc::task_info(
            libc::mach_task_self_,
            TASK_VM_INFO,
            (&mut info as *mut TaskVmInfoRev3).cast(),
            &mut count,
        )
    };
    (result == libc::KERN_SUCCESS && count as usize >= REV1_COUNT).then(|| Snapshot {
        current: info.phys_footprint,
        peak: (count as usize >= REV3_COUNT)
            .then(|| u64::try_from(info.ledger_phys_footprint_peak).ok())
            .flatten(),
    })
}

#[cfg(target_os = "macos")]
pub(super) fn mem_class_mb() -> Option<u64> {
    let mut bytes = 0u64;
    let mut len = std::mem::size_of_val(&bytes);
    // SAFETY: hw.memsize is a read-only u64 and len is the writable buffer size.
    let result = unsafe {
        libc::sysctlbyname(
            c"hw.memsize".as_ptr(),
            (&mut bytes as *mut u64).cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    (result == 0 && len == std::mem::size_of_val(&bytes)).then_some(bytes / (1024 * 1024))
}

#[cfg(target_os = "windows")]
pub(super) fn snapshot() -> Option<Snapshot> {
    use windows::Win32::System::{
        ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX},
        Threading::GetCurrentProcess,
    };

    let mut counters = PROCESS_MEMORY_COUNTERS_EX::default();
    let size = std::mem::size_of_val(&counters) as u32;
    // SAFETY: the size selects the EX layout and the buffer is writable for that size.
    unsafe {
        GetProcessMemoryInfo(
            GetCurrentProcess(),
            (&mut counters as *mut PROCESS_MEMORY_COUNTERS_EX).cast(),
            size,
        )
    }
    .ok()?;
    Some(Snapshot {
        current: counters.WorkingSetSize as u64,
        peak: Some(counters.PeakWorkingSetSize as u64),
    })
}

#[cfg(target_os = "windows")]
pub(super) fn mem_class_mb() -> Option<u64> {
    use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

    let mut status = MEMORYSTATUSEX {
        dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    // SAFETY: status is a writable MEMORYSTATUSEX with its required length set.
    unsafe { GlobalMemoryStatusEx(&mut status) }.ok()?;
    Some(status.ullTotalPhys / (1024 * 1024))
}

#[cfg(target_os = "linux")]
pub(super) fn snapshot() -> Option<Snapshot> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    Some(Snapshot {
        current: proc_kb(&status, "VmRSS:")?.checked_mul(1024)?,
        peak: proc_kb(&status, "VmHWM:").and_then(|kb| kb.checked_mul(1024)),
    })
}

#[cfg(target_os = "linux")]
pub(super) fn mem_class_mb() -> Option<u64> {
    let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
    Some(proc_kb(&meminfo, "MemTotal:")? / 1024)
}

#[cfg(target_os = "linux")]
fn proc_kb(text: &str, field: &str) -> Option<u64> {
    text.lines()
        .find_map(|line| line.strip_prefix(field))?
        .split_ascii_whitespace()
        .next()?
        .parse()
        .ok()
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
pub(super) fn snapshot() -> Option<Snapshot> {
    None
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
pub(super) fn mem_class_mb() -> Option<u64> {
    None
}
