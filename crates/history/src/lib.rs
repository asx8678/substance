//! The git engine, the GitHub forge client and registry lookups; the history,
//! provenance and dependency checks.
//!
//! M1-T01 lands the snapshot layer: every external response is saved under the run
//! directory and can be replayed with `--snapshot-from`.

pub mod snapshot;

pub use snapshot::{
    SnapshotBody, SnapshotError, SnapshotHttp, SnapshotKind, SnapshotRequest, SnapshotResponse,
};
