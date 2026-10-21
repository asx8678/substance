//! Working copy handling, host processes, port allocation, the external-tool runner,
//! and control of the egress proxy, Toxiproxy and Mailpit.

pub mod blobs;
pub mod budget;
pub mod limits;
pub mod logs;
pub mod permit;
pub mod port;
pub mod process;
pub mod tool;
pub mod workcopy;

pub use blobs::{BlobError, BlobStore, reference_of};
pub use budget::Budget;
pub use limits::Limits;
pub use logs::{LogEntry, LogSink, Stream};
pub use permit::ExecPermit;
pub use port::free_port;
pub use process::{AppProcess, ExitInfo, ProcessError, ProcessGuard, ProcessSpec, ServiceProcess};
pub use tool::{Captured, ToolError, ToolOutput, run_tool, run_tool_capture, run_tool_capture_any};
pub use workcopy::{WorkCopy, WorkCopyError};
