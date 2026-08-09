//! Boot-time sweep for untracked same-bundle harness processes, plus low-level
//! process-tree helpers shared with the periodic orphan sweeps in `runtime.rs`.
//!
//! The env-var and PID-file sweeps cannot see a harness whose receipt is gone
//! or that predates `BUZZ_MANAGED_AGENT` injection. This sweep derives the
//! expected `buzz-acp` path from the running executable and kills any process
//! whose exe matches exactly, minus the tracked set. The PID enumeration,
//! procargs, parent/PGID lookups, and live-descendant classification helpers
//! collected here are also called directly by the periodic orphan sweeps.

// Re-declare the macOS process-info FFI so sweep.rs can call it independently.
// Multiple extern "C" declarations of the same symbol are legal in Rust; the
// linker sees one symbol regardless of how many translation units declare it.
#[cfg(target_os = "macos")]
extern "C" {
    fn proc_listallpids(buffer: *mut libc::c_int, buffersize: libc::c_int) -> libc::c_int;
}

// ── Shared low-level helpers ──────────────────────────────────────────────

/// Collect all PIDs currently on the system.
///
/// Loops until the buffer is large enough to hold all PIDs — under a fork
/// storm the count can grow between the probe and the fill call. Returns an
/// empty vec on any kernel error.
#[cfg(target_os = "macos")]
pub(super) fn collect_all_pids() -> Vec<libc::c_int> {
    let mut pids: Vec<libc::c_int>;
    loop {
        let count = unsafe { proc_listallpids(std::ptr::null_mut(), 0) };
        if count <= 0 {
            return Vec::new();
        }
        let buf_len = (count as usize) * 2;
        pids = vec![0; buf_len];
        let actual = unsafe {
            proc_listallpids(
                pids.as_mut_ptr(),
                (buf_len * std::mem::size_of::<libc::c_int>()) as libc::c_int,
            )
        };
        if actual <= 0 {
            return Vec::new();
        }
        pids.truncate(actual as usize);
        if (actual as usize) < buf_len {
            return pids;
        }
    }
}

/// Read the raw `KERN_PROCARGS2` buffer for the given PID.
///
/// Two-phase sysctl: first probe for the required buffer size, then fill.
/// A process's arguments are immutable after `execve`, so the size reported
/// by the probe is stable — no grow-and-retry loop is needed.
///
/// Returns `None` if either sysctl call fails (e.g. the process has already
/// exited or we lack permission).
#[cfg(target_os = "macos")]
pub(super) fn procargs2_buffer(pid: u32) -> Option<Vec<u8>> {
    let mut mib: [libc::c_int; 3] = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid as libc::c_int];
    let mut buf_size: libc::size_t = 0;

    if unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            std::ptr::null_mut(),
            &mut buf_size,
            std::ptr::null_mut(),
            0,
        )
    } != 0
    {
        return None;
    }

    let mut buf: Vec<u8> = vec![0; buf_size];
    if unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            buf.as_mut_ptr() as *mut libc::c_void,
            &mut buf_size,
            std::ptr::null_mut(),
            0,
        )
    } != 0
    {
        return None;
    }
    buf.truncate(buf_size);
    Some(buf)
}

// ── Ancestor walk ────────────────────────────────────────────────────────

/// True if walking `start`'s parent chain reaches any PID in `skip_pids`.
/// Bounded to 32 hops to guard against PPID cycles from PID reuse; a lookup
/// failure or reaching PID ≤ 1 ends the walk (process is not a descendant of
/// any tracked harness).
///
/// The candidate itself being in `skip_pids` is handled at the call site —
/// this function checks strict ancestors only.
#[cfg(unix)]
pub(super) fn walk_has_tracked_ancestor(
    start: u32,
    skip_pids: &[u32],
    parent_of: impl Fn(u32) -> Option<u32>,
) -> bool {
    const MAX_DEPTH: usize = 32;
    let mut cur = start;
    for _ in 0..MAX_DEPTH {
        let Some(parent) = parent_of(cur) else {
            return false;
        };
        if parent <= 1 || parent == cur {
            return false;
        }
        if skip_pids.contains(&parent) {
            return true;
        }
        cur = parent;
    }
    false
}

/// OS-resolved parent-PID lookup for `walk_has_tracked_ancestor`.
/// Test-only: lets tests call a single platform-agnostic name without
/// cfg gates; production code calls `ppid_of_macos`/`ppid_of_linux` directly.
#[cfg(all(test, target_os = "macos"))]
pub(super) fn ppid_of(pid: u32) -> Option<u32> {
    ppid_of_macos(pid)
}

/// OS-resolved parent-PID lookup for `walk_has_tracked_ancestor`.
/// Test-only: lets tests call a single platform-agnostic name without
/// cfg gates; production code calls `ppid_of_macos`/`ppid_of_linux` directly.
#[cfg(all(test, unix, not(target_os = "macos")))]
pub(super) fn ppid_of(pid: u32) -> Option<u32> {
    ppid_of_linux(pid)
}

/// Return the parent PID of a process on macOS via `proc_pidinfo`.
/// Returns `None` if the syscall fails (process may have exited).
#[cfg(target_os = "macos")]
pub(super) fn ppid_of_macos(pid: u32) -> Option<u32> {
    let mut info = std::mem::MaybeUninit::<super::BSDInfo>::zeroed();
    let ret = unsafe {
        super::proc_pidinfo(
            pid as libc::c_int,
            super::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr() as *mut libc::c_void,
            std::mem::size_of::<super::BSDInfo>() as libc::c_int,
        )
    };
    if ret <= 0 {
        return None;
    }
    Some(unsafe { info.assume_init() }.pbi_ppid)
}

/// Parse the PPID and PGID fields from `/proc/<pid>/stat` in one read.
/// Fields after the last `)` (comm may contain spaces/parens): index 1 is
/// PPID, index 2 is PGID.
#[cfg(all(unix, not(target_os = "macos")))]
pub(super) fn proc_stat_ppid_pgid_linux(pid: u32) -> Option<(u32, u32)> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after_comm = stat.rsplit_once(')')?.1;
    // Fields after ')': " S ppid pgid ..."
    let mut fields = after_comm.split_whitespace();
    let _state = fields.next()?; // index 0: state
    let ppid = fields.next()?.parse::<u32>().ok()?; // index 1: PPID
    let pgid = fields.next()?.parse::<u32>().ok()?; // index 2: PGID
    Some((ppid, pgid))
}

/// Return the parent PID of a process from `/proc/<pid>/stat`.
#[cfg(all(unix, not(target_os = "macos")))]
pub(super) fn ppid_of_linux(pid: u32) -> Option<u32> {
    proc_stat_ppid_pgid_linux(pid).map(|(ppid, _)| ppid)
}

/// True if `pid` is a live descendant of any tracked harness in `skip_pids`.
///
/// Three complementary checks:
/// 1. Direct parent — `ppid` was already fetched by the caller's BSDInfo
///    UID gate, so this hop is free.
/// 2. Reparenting guard — if an intermediate in the ancestor chain died,
///    the process reparents to init (PPID 1) and the ancestor walk can no
///    longer reach the harness; a process that started inside the
///    harness's process group still has PGID == harness PID, so the PGID
///    check spares it. This is NOT a redundant fast-path — it covers a
///    case the walk cannot.
/// 3. Bounded ancestor walk from `ppid` — covers deeper live chains where
///    intermediates run in their own process groups (e.g. buzz-acp ->
///    node shim -> codex-acp).
#[cfg(target_os = "macos")]
pub(super) fn is_live_descendant_macos(pid: u32, ppid: u32, skip_pids: &[u32]) -> bool {
    if skip_pids.contains(&ppid) {
        return true;
    }
    let pgid = unsafe { libc::getpgid(pid as i32) };
    if pgid > 0 && skip_pids.contains(&(pgid as u32)) {
        return true;
    }
    walk_has_tracked_ancestor(ppid, skip_pids, ppid_of_macos)
}

/// Linux variant: reads PPID and PGID from `/proc/<pid>/stat` in a single
/// read, then applies the same three checks as the macOS variant. An
/// unreadable stat file (process exiting) yields `false` — the two-tick
/// grace in the periodic sweep absorbs transient failures.
#[cfg(all(unix, not(target_os = "macos")))]
pub(super) fn is_live_descendant_linux(pid: u32, skip_pids: &[u32]) -> bool {
    let Some((ppid, pgid)) = proc_stat_ppid_pgid_linux(pid) else {
        return false;
    };
    if skip_pids.contains(&ppid) || skip_pids.contains(&pgid) {
        return true;
    }
    walk_has_tracked_ancestor(ppid, skip_pids, ppid_of_linux)
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── walk_has_tracked_ancestor ────────────────────────────────────────

    #[cfg(unix)]
    fn map_parent(tree: &std::collections::HashMap<u32, u32>, pid: u32) -> Option<u32> {
        tree.get(&pid).copied()
    }

    #[cfg(unix)]
    #[test]
    fn walk_direct_child_of_tracked_harness_is_exempted() {
        // PID 101's parent is 100 (tracked) → live descendant, not an orphan.
        let tree: std::collections::HashMap<u32, u32> = [(101, 100)].into_iter().collect();
        assert!(walk_has_tracked_ancestor(101, &[100], |p| map_parent(
            &tree, p
        )));
    }

    #[cfg(unix)]
    #[test]
    fn walk_grandchild_via_own_group_wrapper_is_exempted() {
        // Production tree: harness(100) → node-wrapper(101, own group) → codex-acp(102).
        // One-level PPID check misses 102; the walk catches it.
        let tree: std::collections::HashMap<u32, u32> =
            [(101, 100), (102, 101)].into_iter().collect();
        assert!(walk_has_tracked_ancestor(102, &[100], |p| map_parent(
            &tree, p
        )));
    }

    #[cfg(unix)]
    #[test]
    fn walk_real_orphan_ending_at_pid1_returns_false() {
        // Genuine orphan: chain ends at init (PID 1), no tracked ancestor.
        let tree: std::collections::HashMap<u32, u32> = [(201, 1)].into_iter().collect();
        assert!(!walk_has_tracked_ancestor(201, &[100], |p| map_parent(
            &tree, p
        )));
    }

    #[cfg(unix)]
    #[test]
    fn walk_ppid_cycle_terminates_and_returns_false() {
        // PPID cycle (a → b → a) from PID reuse must terminate, not loop.
        let tree: std::collections::HashMap<u32, u32> =
            [(300, 301), (301, 300)].into_iter().collect();
        assert!(!walk_has_tracked_ancestor(300, &[999], |p| map_parent(
            &tree, p
        )));
    }

    #[cfg(unix)]
    #[test]
    fn walk_missing_parent_entry_returns_false() {
        // proc_pidinfo / /proc stat failure (process exited) → not a descendant.
        let tree: std::collections::HashMap<u32, u32> = [].into_iter().collect();
        assert!(!walk_has_tracked_ancestor(400, &[100], |p| map_parent(
            &tree, p
        )));
    }

    #[cfg(unix)]
    #[test]
    fn walk_finds_ancestor_at_exact_depth_cap() {
        // Chain with exactly 32 edges: 1000 → 1001 → … → 1032.
        // The ancestor at hop 32 is within MAX_DEPTH and must be found.
        let mut tree = std::collections::HashMap::new();
        for i in 0..32u32 {
            tree.insert(1000 + i, 1000 + i + 1);
        }
        assert!(walk_has_tracked_ancestor(1000, &[1032], |p| map_parent(
            &tree, p
        )));
    }

    #[cfg(unix)]
    #[test]
    fn walk_misses_ancestor_beyond_depth_cap() {
        // Chain with 33 edges: 1000 → 1001 → … → 1033.
        // Hop 33 exceeds MAX_DEPTH (32) — the ancestor must not be found.
        let mut tree = std::collections::HashMap::new();
        for i in 0..33u32 {
            tree.insert(1000 + i, 1000 + i + 1);
        }
        assert!(!walk_has_tracked_ancestor(1000, &[1033], |p| map_parent(
            &tree, p
        )));
    }
}
