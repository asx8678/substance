//! Judging login and logout by effect (M4-T01, review fix 25).
//!
//! The plan forbids words as evidence: "success is judged by effect, not by words" and "logout is
//! detected by effect, not by words". So these functions read what the browser *did* -- which storage
//! keys appeared, which disappeared, whether an error signal showed -- and never what the page said.
//! A button labelled "Sign out" that does nothing is not a logout control, and a button labelled
//! "Continue" that clears the session is one.
//!
//! ## What "auth state" means here
//!
//! A cookie or storage key is auth state when its name looks like a session: the plan's `changed`
//! list already reports `cookie:sid` and `localStorage:token`, and the login result carries it. The
//! test is on the *key name*, which is not a word on the page -- it is the app's own identifier for
//! the thing that carries the session, and the alternative (guessing from the value) would need the
//! secret the effect is about.

use std::collections::BTreeSet;

use substance_core::model::{Control, ControlPurpose, EffectRecord, Storage};

/// The parts of a key name that mark it as auth state.
const AUTH_MARKERS: [&str; 8] = [
    "token", "session", "auth", "sid", "jwt", "access", "refresh", "user",
];

/// Whether a storage key name carries a session.
#[must_use]
pub fn is_auth_key(key: &str) -> bool {
    let lower = key.to_lowercase();
    AUTH_MARKERS.iter().any(|marker| lower.contains(marker))
}

/// Every auth-bearing storage key in `storage`, as `cookie:sid` and `localStorage:token`.
///
/// The same spelling the driver's login result uses, so a caller can compare the two without
/// translating: one is what a login changed, the other is what a later record still holds.
#[must_use]
pub fn auth_keys(storage: &Storage) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    for name in storage.cookies.keys() {
        if is_auth_key(name) {
            keys.insert(format!("cookie:{name}"));
        }
    }
    for name in storage.local.keys() {
        if is_auth_key(name) {
            keys.insert(format!("localStorage:{name}"));
        }
    }
    for name in storage.session.keys() {
        if is_auth_key(name) {
            keys.insert(format!("sessionStorage:{name}"));
        }
    }
    keys
}

/// Whether a screen still offers a password input, from the controls the driver listed.
#[must_use]
pub fn offers_password(controls: &[Control]) -> bool {
    controls
        .iter()
        .any(|control| control.name.to_lowercase().contains("password"))
}

/// Why a login did not work, or \`None\` when it did.
///
/// The three conditions the plan names, in the order that gives the most useful message: no new auth
/// key at all, an error signal shown, or still being asked for a password where the login landed.
#[must_use]
pub fn login_failure(changed: &[String], error_signal: bool, landed: &[Control]) -> Option<String> {
    let auth: Vec<&String> = changed.iter().filter(|key| is_auth_key(key)).collect();
    if auth.is_empty() {
        return Some("no auth cookie or storage key appeared".to_owned());
    }
    if error_signal {
        return Some("the page showed an error signal".to_owned());
    }
    if offers_password(landed) {
        return Some("the landing page still asks for a password".to_owned());
    }
    None
}

/// Whether an action cleared the session, judged from the storage before and after.
///
/// "Cleared" is the strict reading: auth keys were present before and none of them is present after.
/// A record that lost one of two keys changed the session without ending it, and calling that a
/// logout would make the crawler log in again in the middle of an action that was not one.
#[must_use]
pub fn cleared_session(before: &Storage, after: &Storage) -> bool {
    let had = auth_keys(before);
    let has = auth_keys(after);
    !had.is_empty() && had.is_disjoint(&has)
}

/// Whether a record's action logged the session out, given the storage before it ran.
///
/// A record carries only the storage *after* the action, so `before` is the storage the crawl read
/// before clicking: the crawl is the only side that knows both, and pairing them here keeps the rule
/// in one place instead of leaving it to the caller to remember which side is which.
#[must_use]
pub fn logged_out_since(record: &EffectRecord, before: &Storage) -> bool {
    cleared_session(before, &record.storage)
}

/// Marks `index` as the logout control, once an action on it was seen to clear the session.
///
/// Returns whether anything changed. The purpose is set rather than the control removed: the model
/// keeps what the screen offers, and "this is the logout button" is a fact about the screen worth
/// recording; the crawl is what stops using it.
#[must_use]
pub fn mark_logout(control: &mut Control) -> bool {
    if matches!(control.purpose, ControlPurpose::Logout) {
        return false;
    }
    control.purpose = ControlPurpose::Logout;
    true
}
