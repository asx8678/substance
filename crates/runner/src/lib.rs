//! Working copy handling, host processes, port allocation, the external-tool runner,
//! and control of the egress proxy, Toxiproxy and Mailpit.

pub mod blobs;
pub mod budget;
pub mod egress;
pub mod interrupt;
pub mod limits;
pub mod logs;
pub mod mail;
pub mod oneshot;
pub mod permit;
pub mod port;
pub mod process;
pub mod tool;
pub mod workcopy;

pub use blobs::{BlobError, BlobStore, reference_of};
pub use budget::Budget;
pub use interrupt::{Cleanup, InterruptGuard, Registrar, Resource, install};
pub use limits::Limits;
pub use logs::{LogEntry, LogSink, Stream};
pub use oneshot::{OneShot, OneShotError, StepOutput};
pub use permit::ExecPermit;
pub use port::free_port;
pub use process::{
    AppProcess, ExitInfo, HOME_DIR, OwnedApp, ProcessError, ProcessGuard, ProcessSpec,
    ServiceProcess, StopLatch, StoppedApp, home_env, kill_group_now, prepare_home,
};
pub use tool::{Captured, ToolError, ToolOutput, run_tool, run_tool_capture, run_tool_capture_any};
pub use workcopy::{WorkCopy, WorkCopyError};
