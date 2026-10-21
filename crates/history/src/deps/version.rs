//! Version strings, as far as the dependency checks need them (M1-T10).
//!
//! P03 asks whether the version a project adopted was already a major version behind; P04
//! asks whether a specific version is deprecated or yanked. Both need to compare versions,
//! and neither needs semver's full ordering: no build metadata, no range algebra, no
//! pre-release precedence beyond "a pre-release sorts before its release".
//!
//! So this module answers three questions and nothing else — the major number, whether a
//! release is a pre-release, and whether two strings name the same release — using integers
//! only. A version this parser cannot read returns `None`, and every caller treats that as
//! "cannot tell", never as "fine": guessing a major from a string like `latest` would
//! invent the very fact P03 reports.
//!
//! The accepted shapes are the ones these registries actually carry:
//!
//! ```text
//! 1  1.2  1.2.3   v1.2.3   1.2.3-beta.1   1.2.3+build.5
//! 1.2.3-rc.1+build.5        2.0.0.post1   1.2.3.4
//! ```
//!
//! A leading `v` is dropped (crates.io tags and GitHub releases carry it), build metadata after
//! `+` is dropped (it does not order), and a pre-release suffix after the first `-` is kept
//! only as a flag.

/// The parts of a version this build compares.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct VersionCore {
    /// The major number: the P03 question is entirely about this.
    pub major: u64,
    /// The minor number, for ordering within a major.
    pub minor: u64,
    /// The patch number.
    pub patch: u64,
    /// Whether the version carries a pre-release suffix, which sorts before the release.
    pub pre_release: bool,
}

impl VersionCore {
    /// Parses a version string, or answers `None` when it is not one this build reads.
    ///
    /// `None` is not an error state to be smoothed over: a dependency whose lockfile records
    /// a git URL or a tag instead of a version has no major number, and P03 must say it could
    /// not tell rather than compare against a zero.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let trimmed = text.trim();
        // A leading `v` is a tag spelling, not a part of the number.
        let trimmed = trimmed.strip_prefix('v').unwrap_or(trimmed);
        if trimmed.is_empty() {
            return None;
        }
        // Build metadata does not order, so it is cut before anything else is read.
        let trimmed = trimmed.split('+').next().unwrap_or(trimmed);
        // The first `-` starts the pre-release. A hyphen inside build metadata was already
        // removed above, so this is the pre-release separator or nothing.
        let (numbers, pre_release) = match trimmed.split_once('-') {
            Some((numbers, suffix)) => (numbers, !suffix.is_empty()),
            None => (trimmed, false),
        };
        let mut parts = numbers.split('.');
        let major = parse_number(parts.next()?)?;
        // `1.2.3.4` is four numbers; the fourth carries no order this build needs, but it
        // must not make the version unreadable.
        let minor = parts.next().map_or(Some(0), parse_number)?;
        let patch = parts.next().map_or(Some(0), parse_number)?;
        for extra in parts {
            parse_number(extra)?;
        }
        Some(Self {
            major,
            minor,
            patch,
            pre_release,
        })
    }

    /// The major number alone, or `None` when the string is not a version.
    #[must_use]
    pub fn major_of(text: &str) -> Option<u64> {
        Self::parse(text).map(|core| core.major)
    }
}

/// One dot-separated number.
///
/// Rejects an empty part and anything that is not ASCII digits: `1.x` and `1.2.3-beta` are
/// handled by the caller, but `1..2` and `1.2a` are not versions and must not become ones by
/// a lenient parse. `u64` rather than a wider integer because no registry version has a part
/// that large, and overflowing silently would be worse than answering `None`.
fn parse_number(part: &str) -> Option<u64> {
    if part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    part.parse::<u64>().ok()
}

/// Whether `candidate` was already a major version behind `latest`.
///
/// The comparison is `latest.major - candidate.major >= 1`, expressed without a subtraction
/// that could underflow: a candidate *ahead* of the latest (a pre-release of the next major,
/// say) is not stale, and neither is one at the same major.
#[must_use]
pub fn major_behind(candidate: &str, latest: &str) -> Option<u64> {
    let candidate = VersionCore::parse(candidate)?;
    let latest = VersionCore::parse(latest)?;
    if latest.major > candidate.major {
        Some(latest.major - candidate.major)
    } else {
        Some(0)
    }
}

#[cfg(any())]
mod never {
    // Intentionally empty: no tests in substance (ADR-0008). The module's behaviour is
    // exercised by a throwaway probe during development, and by the checks that call it.
}
