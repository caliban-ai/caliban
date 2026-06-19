//! Shell tools — `Bash` (foreground exec) plus the background-job registry
//! and its companion tools (`BashOutput`, `KillShell`).

pub mod bash;
pub mod bash_bg;

pub use bash::BashTool;
pub use bash_bg::{
    BashBgRegistry, BashJob, BashOutputTool, BashStatus, KillShellTool, RingBuffer, global_registry,
};

/// Send `signal` to the entire process group led by `pid`.
///
/// Shells are spawned with `process_group(0)` (see
/// [`bash_bg::build_shell`]), so the child's PID equals its PGID and a
/// negative target reaches every descendant. Shared by the foreground Bash
/// kill path (`SIGKILL`) and the background-job kill path (`SIGTERM` /
/// `SIGKILL`).
#[cfg(unix)]
#[allow(unsafe_code)] // libc::kill is a stable FFI call; signalling a process group (negative PID) is not exposed by std's safe API
pub(crate) fn signal_process_group(pid: u32, signal: libc::c_int) {
    if let Ok(pid_i32) = i32::try_from(pid) {
        // SAFETY: `libc::kill` takes two integers and returns an integer — no
        // pointer dereferences or aliasing. A negative first argument signals
        // the whole process group (PGID == the leader's PID). `ESRCH` on an
        // already-dead group is benign; the return is ignored.
        unsafe {
            libc::kill(-pid_i32, signal);
        }
    }
}
