//! Working copy handling, host processes, port allocation, the external-tool runner,
//! and control of the egress proxy, Toxiproxy and Mailpit.

pub mod tool;

pub use tool::{Captured, ToolError, run_tool_capture, run_tool_capture_any};
