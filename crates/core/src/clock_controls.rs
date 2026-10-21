//! Freezing the clock, the timezone and the locale (M3-T06, ADR-0022).
//!
//! The plan's "freeze the inputs" has three parts, and each fails differently:
//!
//! - the **browser** clock is frozen by Playwright, inside the driver;
//! - the **server** clock is frozen with `libfaketime`, by an `LD_PRELOAD` in the app's environment;
//! - the timezone and locale are environment variables, and are set always, because a run that
//!   formatted a date in the developer's zone would differ from one formatted in UTC for a reason
//!   no diff shows.
//!
//! This module is the pure part: it decides *what* the environment should contain for a given host,
//! and does no IO beyond asking the filesystem whether a library exists. The runner applies the
//! answer to the processes it spawns, so there is one place that decides and one place that acts.
//!
//! ## Why `@` and not a fixed instant
//!
//! `FAKETIME="@2026-10-21 09:30:00"` *starts* the clock there and lets it advance, which is what a
//! server needs: a fixed instant freezes every `setTimeout`, every retry backoff and every cache TTL,
//! and a server whose timers never fire is a server that hangs rather than one that is deterministic.
//!
//! ## Why `FAKETIME_DONT_RESET`
//!
//! `libfaketime` resets the clock in every child process that inherits the preload unless told not to.
//! An app started by `npm run dev` runs in a child of npm, so without this the clock would restart at
//! `as_of` for node after npm had already advanced it -- and, worse, two children started a second
//! apart would disagree about the current time. `FAKETIME_DONT_RESET=1` makes the offset a property
//! of the process tree rather than of each process.
//!
//! ## What this cannot do
//!
//! A statically linked binary with its own clock implementation never calls the libc symbols
//! `libfaketime` interposes, so it sees the real clock even with `LD_PRELOAD` set. Go programs are the
//! common case. Detecting that cheaply is not possible -- it would mean inspecting every binary the
//! repository runs -- so the manifest records `server_clock: real` whenever `libfaketime` cannot be
//! applied at all, and the checks that depend on a moved server clock report `not_measured`. A
//! statically linked binary on a Linux host with `libfaketime` installed is the gap that remains, and
//! it is recorded as a consequence of ADR-0022 rather than papered over.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::Timestamp;
use crate::model::ServerClock;

/// The timezone every app process runs in.
pub const TIMEZONE: &str = "UTC";

/// The locale every app process runs in, as a POSIX locale name.
pub const LOCALE: &str = "en_US.UTF-8";

/// The browser's locale, as a BCP 47 tag rather than a POSIX locale name.
pub const BROWSER_LOCALE: &str = "en-US";

/// The browser's timezone id, as Playwright spells it.
pub const BROWSER_TIMEZONE: &str = "UTC";

/// The shared object `libfaketime` installs.
const FAKETIME_LIBRARY: &str = "libfaketime.so.1";

/// The directories a distribution puts `libfaketime.so.1` in, for `os` and `arch`.
///
/// Debian and Ubuntu install it as `.../faketime/libfaketime.so.1`; Fedora, Arch and a local install
/// use the plain name. Both are candidates, in that order, in each directory.
///
/// Pure: this returns the paths to *try*, and something outside this crate checks whether they exist.
/// crates/core is the IO-free layer (§4.2), so the caller does the `is_file` -- and the same list
/// reaches `substance doctor` and the run, which is what stops them disagreeing about a host.
#[must_use]
pub fn libfaketime_candidates(os: &str, arch: &str) -> Vec<PathBuf> {
    if os != "linux" {
        // macOS and Windows have no libfaketime at all. Returning an empty list rather than a set of
        // paths that cannot exist keeps "not applicable" and "not installed" distinguishable at the
        // only place that can tell them apart: the caller, which knows the platform.
        return Vec::new();
    }
    let triple = match arch {
        "x86_64" => Some("x86_64-linux-gnu"),
        "aarch64" => Some("aarch64-linux-gnu"),
        "arm" => Some("arm-linux-gnueabihf"),
        "i686" => Some("i386-linux-gnu"),
        // An architecture this list does not name still gets the untripled directories below, where
        // a distribution that does not use multiarch keeps it.
        _ => None,
    };
    let mut directories: Vec<String> = Vec::new();
    if let Some(triple) = triple {
        directories.push(format!("/usr/lib/{triple}"));
    }
    for dir in ["/usr/lib64", "/usr/lib", "/usr/local/lib"] {
        directories.push(dir.to_owned());
    }
    let mut candidates = Vec::with_capacity(directories.len() * 2);
    for dir in directories {
        candidates.push(Path::new(&dir).join("faketime").join(FAKETIME_LIBRARY));
        candidates.push(Path::new(&dir).join(FAKETIME_LIBRARY));
    }
    candidates
}

/// Whether `libfaketime` can be used on this host, and where its library is.
///
/// The detection is shared with `substance doctor`, so the tool that says the library is missing and
/// the run that fails to freeze the clock cannot disagree.
/// Not `#[non_exhaustive]`: the set of answers is closed (found, cannot apply here, not installed),
/// and a caller forced to write a catch-all arm could turn a new answer into a silent one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Libfaketime {
    /// Found at this path.
    Found(PathBuf),
    /// This platform has no `libfaketime` at all: macOS and Windows. Not a defect, and not a gap in
    /// the install either, so the report distinguishes it from [`Self::Missing`].
    UnsupportedPlatform,
    /// Linux, but the library is not installed.
    Missing,
}

impl Libfaketime {
    /// The answer for `os` when none of `candidates` existed.
    ///
    /// Off Linux the question does not apply, and that is a fact about the platform rather than a gap
    /// in the install, so the caller does not have to spell the distinction out twice.
    #[must_use]
    pub fn absent(os: &str) -> Self {
        if os == "linux" {
            Self::Missing
        } else {
            Self::UnsupportedPlatform
        }
    }

    /// Whether the platform could use one, whatever is installed.
    #[must_use]
    pub fn applicable(&self) -> bool {
        !matches!(self, Self::UnsupportedPlatform)
    }

    /// Whether the library was found.
    #[must_use]
    pub fn present(&self) -> bool {
        matches!(self, Self::Found(_))
    }

    /// The path, when one was found.
    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::Found(path) => Some(path.as_path()),
            _ => None,
        }
    }
}

/// How the server's clock will behave, decided from the host alone.
///
/// Deliberately takes the detection result rather than reading it: the same answer goes into the
/// manifest and into the environment, and a caller that computed it once cannot produce a manifest
/// that disagrees with what the app was actually given.
#[must_use]
pub fn server_clock(libfaketime: &Libfaketime) -> ServerClock {
    if libfaketime.present() {
        ServerClock::FixedStart
    } else {
        ServerClock::Real
    }
}

/// `as_of` as `libfaketime` wants it: `YYYY-MM-DD HH:MM:SS` in UTC, with no fractional seconds.
///
/// `Timestamp` is already truncated to whole seconds and normalized to UTC, so this is a format and
/// not a rounding. The space rather than the RFC 3339 `T` is `libfaketime`'s own syntax.
#[must_use]
pub fn faketime_value(as_of: Timestamp) -> String {
    let instant = as_of.as_offset_date_time();
    let mut text = String::with_capacity("@YYYY-MM-DD HH:MM:SS".len());
    text.push('@');
    push_padded(&mut text, instant.year(), 4);
    text.push('-');
    push_padded(&mut text, i32::from(instant.month() as u8), 2);
    text.push('-');
    push_padded(&mut text, i32::from(instant.day()), 2);
    text.push(' ');
    push_padded(&mut text, i32::from(instant.hour()), 2);
    text.push(':');
    push_padded(&mut text, i32::from(instant.minute()), 2);
    text.push(':');
    push_padded(&mut text, i32::from(instant.second()), 2);
    text
}

/// Appends `value` zero-padded to `width` digits, with a leading minus for a negative year.
fn push_padded(text: &mut String, value: i32, width: usize) {
    if value < 0 {
        text.push('-');
        push_padded(text, value.unsigned_abs().cast_signed(), width);
        return;
    }
    let digits = value.to_string();
    for _ in digits.len()..width {
        text.push('0');
    }
    text.push_str(digits.as_str());
}

/// The timezone and locale variables every app process gets (M3-T06).
///
/// A sorted map rather than a constant slice, because it is merged with the proxy, database and mail
/// variables before it reaches a process, and a merge over an unsorted iterator would make the order
/// depend on insertion.
#[must_use]
pub fn locale_env() -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    env.insert("LC_ALL".to_owned(), LOCALE.to_owned());
    env.insert("LANG".to_owned(), LOCALE.to_owned());
    env.insert("TZ".to_owned(), TIMEZONE.to_owned());
    env
}

/// The `libfaketime` variables for `as_of`, or nothing when the clock cannot be moved.
///
/// Nothing rather than a partial set when the library is missing: an `LD_PRELOAD` of a path that does
/// not exist makes glibc print a warning to every process's stderr, which would show up in the app's
/// log as noise nobody can act on. A missing library means `server_clock: real`, and the manifest is
/// where a reader learns that.
#[must_use]
pub fn faketime_env(libfaketime: &Libfaketime, as_of: Timestamp) -> BTreeMap<String, String> {
    let Some(path) = libfaketime.path() else {
        return BTreeMap::new();
    };
    let mut env = BTreeMap::new();
    env.insert("FAKETIME".to_owned(), faketime_value(as_of));
    // Without this, every child process restarts the clock at `as_of`: an app launched as
    // `npm run dev` would run node on a clock that began again after npm's had advanced.
    env.insert("FAKETIME_DONT_RESET".to_owned(), "1".to_owned());
    env.insert("LD_PRELOAD".to_owned(), path.display().to_string());
    env
}

/// The whole frozen-clock environment for `as_of` on a host with `libfaketime`.
///
/// The two parts together, because a caller that applied one and forgot the other would have an app in
/// UTC on the real clock: the failure looks like a timestamp that moved between runs, which is the
/// thing the task exists to stop.
#[must_use]
pub fn frozen_env(libfaketime: &Libfaketime, as_of: Timestamp) -> BTreeMap<String, String> {
    let mut env = locale_env();
    env.extend(faketime_env(libfaketime, as_of));
    env
}
