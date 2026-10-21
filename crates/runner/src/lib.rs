//! Working copy handling, host processes, port allocation, the external-tool runner,
//! and control of the egress proxy, Toxiproxy and Mailpit.

pub mod blobs;
pub mod permit;
pub mod tool;

pub use blobs::{BlobError, BlobStore, reference_of};
pub use permit::ExecPermit;
pub use tool::{Captured, ToolError, ToolOutput, run_tool, run_tool_capture, run_tool_capture_any};
