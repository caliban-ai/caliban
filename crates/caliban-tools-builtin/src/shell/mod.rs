//! Shell tools — `Bash` (foreground exec) plus the background-job registry
//! and its companion tools (`BashOutput`, `KillShell`).

pub mod bash;
pub mod bash_bg;

pub use bash::BashTool;
pub use bash_bg::{
    BashBgRegistry, BashJob, BashOutputTool, BashStatus, KillShellTool, RingBuffer, global_registry,
};
