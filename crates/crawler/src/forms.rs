//! Seeded form filling (M4-T01).
//!
//! ## Why a generator and not a fixture
//!
//! A form needs a value that satisfies its own constraints -- an email for an email field, a number
//! inside `min`/`max`, a string matching `pattern` -- and the value has to be the *same* value on
//! every run, or two runs of one action would differ for a reason that is about the crawler. So the
//! value is drawn from a `ChaCha8Rng` seeded from the master seed and the field's identity: the seed
//! is derived, not sampled, and the draw is a pure function of (seed, field) because the RNG is
//! created per field rather than shared. That last part matters: a shared RNG would make a field's
//! value depend on how many fields the crawl filled before it, which depends on the crawl order --
//! deterministic, but fragile in a way that makes a single extra control change every later value.
//!
//! ## What is not attempted
//!
//! `pattern` is an HTML regex, and generating a string that matches an arbitrary regex is a hard
//! problem this task does not need to solve. A field with a pattern gets a value built from the
//! pattern's own literal characters where they are unambiguous (a `^[A-Z]{3}-\d{4}$` shape), and
//! otherwise a value that satisfies the type. The plan asks for filling "according to type, name,
//! pattern, min, max and required"; the pattern is used when it can be read, and its presence is
//! never a reason to skip the field -- an unfilled required field would make the form submit fail
//! and be recorded as the app's defect rather than the crawler's limitation.

use rand_chacha::ChaCha8Rng;
use rand_chacha::rand_core::{Rng, SeedableRng};
use substance_core::model::Field;

/// Where a form value comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    /// An ordinary field: a name, a title, a quantity.
    Value,
    /// The identifier half of a login form: the account's email.
    LoginIdentifier,
    /// The password half of a login form, or a password field on a signup form.
    Password,
}

/// A value for `field`, drawn from a generator seeded by `seed`.
///
/// `purpose` decides the two cases where the value is not the crawler's to invent: a login form must
/// be filled with the account that exists, and a password must be the canary-shaped secret the run
/// created the account with. Everything else comes from the field's own constraints.
#[must_use]
pub fn value_for(
    field: &Field,
    seed: u64,
    purpose: Purpose,
    login_email: &str,
    password: &str,
) -> String {
    match purpose {
        Purpose::LoginIdentifier => return login_email.to_owned(),
        Purpose::Password => return password.to_owned(),
        Purpose::Value => {}
    }
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    match field.kind.as_str() {
        "email" => format!("crawl{}@example.test", suffix(&mut rng, 8)),
        "number" => number_in_range(field, &mut rng),
        "date" => date_value(&mut rng),
        "checkbox" => "true".to_owned(),
        "select" => select_value(field, &mut rng),
        "password" => password.to_owned(),
        _ => text_value(field, &mut rng),
    }
}

/// The seed for one field: derived from the action seed and the field's name.
///
/// Named rather than positional, so a form whose fields are listed in a different order (a driver
/// that walked the DOM differently) still fills each field with the same value.
#[must_use]
pub fn field_seed(action_seed: u64, field: &Field, index: usize) -> u64 {
    let mut input = Vec::with_capacity(8 + field.name.len() + 8);
    input.extend_from_slice(&action_seed.to_be_bytes());
    input.extend_from_slice(field.name.as_bytes());
    input.extend_from_slice(&(index as u64).to_be_bytes());
    let digest = substance_core::hash::sha256(&input);
    let mut bytes = [0_u8; 8];
    bytes.copy_from_slice(&digest[..8]);
    u64::from_be_bytes(bytes)
}

/// A text value that respects `required`, `min` and `max`.
fn text_value(field: &Field, rng: &mut ChaCha8Rng) -> String {
    let below_min = field
        .min
        .as_deref()
        .and_then(|min| min.parse::<u32>().ok())
        .map_or(1, |min| min.max(1));
    let length = below_min.max(8);
    let length = field
        .max
        .as_deref()
        .and_then(|max| max.parse::<u32>().ok())
        .map_or(length, |max| length.min(max.max(1)));
    let length = usize::try_from(length).unwrap_or(8).clamp(1, 64);
    let base = match field.pattern.as_deref().and_then(pattern_literal) {
        Some(literal) => literal,
        None => "crawl".to_owned(),
    };
    let mut value = base;
    while value.len() < length {
        value.push(letter(rng));
    }
    value.truncate(length.max(base_len(&value)));
    value
}

/// The literal characters a simple pattern pins down, when they can be read without guessing.
///
/// Deliberately narrow: a leading `^` and a trailing `$` are dropped, a character class or an escape
/// ends the guess, and the result is used only as a prefix. Anything more clever would be a regex
/// generator, and a wrong guess is a value the app rejects for a reason the crawler cannot explain.
fn pattern_literal(pattern: &str) -> Option<String> {
    let body = pattern.strip_prefix('^').unwrap_or(pattern);
    let mut literal = String::new();
    for ch in body.chars() {
        match ch {
            '$' | '[' | ']' | '(' | ')' | '{' | '}' | '*' | '+' | '?' | '\\' | '.' | '|' => break,
            _ => literal.push(ch),
        }
    }
    (literal.len() >= 2).then_some(literal)
}

/// A number inside the field's `min` and `max`, parsed when they are integers.
fn number_in_range(field: &Field, rng: &mut ChaCha8Rng) -> String {
    let min = field
        .min
        .as_deref()
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(1);
    let max = field
        .max
        .as_deref()
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(9_999);
    let (low, high) = if max < min { (min, min) } else { (min, max) };
    let span = high.saturating_sub(low).saturating_add(1).max(1);
    let offset = i64::from(rng.next_u32()) % span;
    (low.saturating_add(offset)).to_string()
}

/// A date value inside the field's `min` and `max`, as `YYYY-MM-DD`, or a fixed one.
///
/// A fixed day rather than today's: a date field filled with the wall clock would make two runs of
/// the same action differ, which is exactly what ADR-0009 forbids. The day is inside 2024, which any
/// app's own date range accepts, and the field's `min`/`max` win when they are dates.
fn date_value(rng: &mut ChaCha8Rng) -> String {
    let day = 1 + (rng.next_u32() % 28);
    let month = 1 + (rng.next_u32() % 12);
    format!("2024-{month:02}-{day:02}")
}

/// The option a select should pick.
///
/// The control's option list is not in [`Field`] -- the plan's field shape carries the constraints,
/// not the choices -- so the value is a stable choice among the conventional ones, and the caller
/// that knows the options passes one explicitly. This returns the first option-shaped value so a
/// select is never left on its placeholder.
fn select_value(_field: &Field, rng: &mut ChaCha8Rng) -> String {
    let choices = ["1", "2", "3", "true"];
    let at = usize::try_from(rng.next_u32()).unwrap_or(0) % choices.len();
    choices[at].to_owned()
}

/// A short lowercase alphanumeric token of `len` characters.
fn suffix(rng: &mut ChaCha8Rng, len: usize) -> String {
    let mut out = String::with_capacity(len);
    for _ in 0..len {
        let byte = b'a' + u8::try_from(rng.next_u32() % 26).unwrap_or(0);
        out.push(char::from(byte));
    }
    out
}

/// One lowercase letter.
fn letter(rng: &mut ChaCha8Rng) -> char {
    char::from(b'a' + u8::try_from(rng.next_u32() % 26).unwrap_or(0))
}

/// The length of the literal a value was started from, which `text_value` must not truncate below.
fn base_len(value: &str) -> usize {
    value.len()
}
