//! Questions about the host that need IO (M3-T06).
//!
//! crates/core is the IO-free layer (§4.2), so it can say which paths `libfaketime.so.1` could be at
//! but not whether any of them exists. This module is the other half: the candidate list comes from
//! [`substance_core::clock_controls::libfaketime_candidates`], and the probe happens here.
//!
//! It lives in `runner` because `runner` is the crate that touches the machine on the run's behalf --
//! it spawns processes, finds ports, writes logs -- and because both callers (the run's own manifest
//! decision and `substance doctor`) already depend on it. One function for both is the point: a doctor
//! that searched a different list than the run would report a host ready for a frozen clock that then
//! ran on the real one.

use std::path::Path;

use substance_core::clock_controls::{Libfaketime, libfaketime_candidates};

/// Whether `libfaketime.so.1` is installed on this host, and where.
///
/// Asks the filesystem only for the candidates core named, in the order it named them, so the answer is
/// a function of the host rather than of this module's opinion. Deterministic per host: two runs on one
/// machine always agree, which is what makes it safe for the answer to be part of the run id.
#[must_use]
pub fn detect_libfaketime() -> Libfaketime {
    let os = std::env::consts::OS;
    for candidate in libfaketime_candidates(os, std::env::consts::ARCH) {
        if Path::new(&candidate).is_file() {
            return Libfaketime::Found(candidate);
        }
    }
    // Off Linux the candidate list is empty and the answer is "this platform has none", not "it is
    // missing": the caller must not be told to install something that does not exist for it.
    Libfaketime::absent(os)
}
