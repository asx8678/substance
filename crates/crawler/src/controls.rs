//! Control order and selector preference (M4-T01).
//!
//! ## Order
//!
//! The plan says actions are sorted by (role, accessible name, selector). That order is what makes a
//! crawl reproducible *and* what makes it stable across a restyle: two runs that find the same
//! controls act on them in the same sequence, and a control that gains a CSS class does not move in
//! the sequence because the CSS path is the last tie-break, not the first.
//!
//! ## Selectors
//!
//! The driver already reports candidates in preference order -- role plus name, then test id, then
//! CSS -- because it is the only side that can see the DOM. This module keeps that order and adds
//! the two decisions that belong to the crawler: which candidate the crawl *uses*, and what to do
//! when that candidate stops matching. The answer to the second is "re-explore the screen", never
//! "fail": a selector breaking is a fact about the app's markup, and a crawler that gave up on the
//! screen would lose every other control on it.

use substance_core::model::{Control, ControlPurpose};

/// The selector the crawl addresses `control` with, or \`None\` when the driver found none.
///
/// The first candidate the driver listed. Preference is the driver's because only it can tell a role
/// and name pair from a CSS path; re-deriving it here would let the two disagree.
#[must_use]
pub fn selector_of(control: &Control) -> Option<&str> {
    control.selectors.first().map(String::as_str)
}

/// Controls in the plan's order: (role, accessible name, selector).
///
/// The whole list is sorted rather than each screen's, so a caller holding every control in the model
/// gets one deterministic sequence. \`sort_by\` is stable, so two controls that tie on all three keep
/// the driver's document order, which is the only information left and is itself deterministic.
pub fn sort_controls(controls: &mut [Control]) {
    controls.sort_by(|left, right| {
        (
            left.role.as_str(),
            left.name.as_str(),
            left.selectors.first(),
        )
            .cmp(&(
                right.role.as_str(),
                right.name.as_str(),
                right.selectors.first(),
            ))
    });
}

/// Whether a control can be acted on: it has a selector and is not the login or logout control.
///
/// The login control is handled by the login path, and the logout control by the logout rule -- both
/// have their own machinery, and a crawl that clicked them as ordinary actions would end the session
/// the rest of the crawl needs.
#[must_use]
pub fn is_actionable(control: &Control) -> bool {
    selector_of(control).is_some()
        && !matches!(
            control.purpose,
            ControlPurpose::Login | ControlPurpose::Logout
        )
}

/// The two halves of a login form: the identifier field and the password field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginFields {
    /// The selector of the identifier (email, username) field.
    pub identifier: String,
    /// The selector of the password field.
    pub password: String,
    /// The selector of the submit control in the same form.
    pub submit: String,
}

/// Whether \`control\` is a password input.
#[must_use]
pub fn is_password_field(control: &Control) -> bool {
    let name = control.name.to_lowercase();
    name.contains("password")
        || control.form.as_ref().is_some_and(|form| {
            form.fields.iter().any(|field| {
                field.kind == "password" && field.name.eq_ignore_ascii_case(&control.name)
            })
        })
}

/// Two controls are in the same form when their form selectors match and are present.
#[must_use]
pub fn same_form(left: &Control, right: &Control) -> bool {
    match (&left.form, &right.form) {
        (Some(a), Some(b)) => a.selector == b.selector,
        _ => false,
    }
}

/// The login form among \`controls\`, if exactly one password-bearing form has exactly one submit.
///
/// The plan's rule (review fix 25): "the first screen in frontier order with exactly one visible
/// password input and one submit control in the same form becomes the \`ui\` login". This function is
/// the second half of that sentence for one screen's already-sorted control list; the frontier picks
/// the screen. A screen with two password forms is ambiguous -- a login plus a change-password panel
/// looks exactly like that -- and guessing which is which is how a crawl locks itself out, so it is
/// refused rather than resolved by order.
#[must_use]
pub fn detect_login(controls: &[Control]) -> Option<LoginFields> {
    let passwords: Vec<&Control> = controls.iter().filter(|c| is_password_field(c)).collect();
    if passwords.is_empty() {
        return None;
    }
    // One password field, or several that are all in one form: a login form with a "show password"
    // duplicate is still one form.
    let mut forms: Vec<&str> = passwords
        .iter()
        .filter_map(|control| control.form.as_ref().map(|form| form.selector.as_str()))
        .collect();
    forms.sort_unstable();
    forms.dedup();
    if forms.len() != 1 {
        return None;
    }
    // The form selector is not needed past this point: `same_form` compares the controls' own
    // copies, and keeping the local would be a second source of truth for it.
    let password = passwords
        .iter()
        .find(|control| selector_of(control).is_some())?;
    let submit = controls.iter().find(|control| {
        selector_of(control).is_some()
            && matches!(
                control.purpose,
                ControlPurpose::Submit | ControlPurpose::Login
            )
            && same_form(control, password)
            && !is_password_field(control)
    })?;
    // The identifier: a text or email field in the same form that is not the password.
    let identifier = controls
        .iter()
        .filter(|control| same_form(control, password) && !is_password_field(control))
        .find(|control| is_identifier_field(control))?;
    Some(LoginFields {
        identifier: selector_of(identifier)?.to_owned(),
        password: selector_of(password)?.to_owned(),
        submit: selector_of(submit)?.to_owned(),
    })
}

/// Whether a control names the identifier half of a login form.
///
/// By name, because the driver reports an accessible name and not an input type for every control:
/// "Email", "Username", "E-mail address" all appear, and a control with no name at all is skipped
/// so a stray text box is not mistaken for the identifier.
#[must_use]
fn is_identifier_field(control: &Control) -> bool {
    let name = control.name.to_lowercase();
    [
        "email",
        "e-mail",
        "username",
        "user name",
        "login",
        "identifier",
    ]
    .iter()
    .any(|word| name.contains(word))
}
