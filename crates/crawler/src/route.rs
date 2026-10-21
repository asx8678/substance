//! Route templates: which path segments are identifiers (M4-T01).
//!
//! ## Why this is not `core::normalize::template_of`
//!
//! `template_of` templates a *request* path, for the idle baseline and the settle counter, and it is
//! deliberately narrow: a slug like `invoices` stays as it is, because merging two request paths that
//! differ only in a slug would let the baseline swallow a real call.
//!
//! The crawler asks a different question. It wants one entry per *screen*, and the plan's rule is
//! wider: "numeric, UUID and digit-bearing segments become `:id`" (M4-T01). `/invoices/2024-01` and
//! `/invoices/2024-02` are one screen with a filter, not two screens; a function that kept them apart
//! would crawl the same page once per credit-card statement and blow the screen budget. So the
//! segment rule here is "all digits, a UUID, or contains a digit", and that difference from
//! `template_of` is the point rather than an oversight. Both agree on the plain cases
//! (`/invoices/42`); they part company on a digit-bearing slug, where the two questions differ.

/// The segment that replaces an identifier.
pub const ID: &str = ":id";

/// The templated form of a screen path: identifier segments become `:id`.
///
/// The query is dropped rather than templated: a screen's identity for the crawl is its path, and a
/// query a form submitted is the action's business, not a new screen's. The fragment is dropped for
/// the same reason -- it never reaches the server.
///
/// A trailing slash is kept as it is, because `/invoices/` and `/invoices` really are different
/// requests; a caller that wants them merged can normalize before calling.
#[must_use]
pub fn template_path(path: &str) -> String {
    let head = head_of(path);
    if head.is_empty() {
        return "/".to_owned();
    }
    // Empty segments are collapsed rather than kept: a browser reports `//invoices/42`, and two
    // templates for one screen would make the crawl visit it twice and spend the screen budget on a
    // spelling. The leading slash is restored once at the end, so the path stays absolute.
    let segments: Vec<&str> = head
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect();
    if segments.is_empty() {
        return "/".to_owned();
    }
    let templated = segments
        .into_iter()
        .map(|segment| match is_identifier(segment) {
            true => ID,
            false => segment,
        })
        .collect::<Vec<_>>()
        .join("/");
    format!("/{templated}")
}

/// The path without query or fragment.
#[must_use]
pub fn head_of(path: &str) -> &str {
    let without_fragment = match path.split_once('#') {
        Some((head, _)) => head,
        None => path,
    };
    match without_fragment.split_once('?') {
        Some((head, _)) => head,
        None => without_fragment,
    }
}

/// Whether one path segment is an identifier rather than a word.
///
/// The plan's rule: all digits, a UUID, or a segment that contains a digit. `invoices` is a word and
/// stays; `invoices-2024` contains digits and becomes `:id`, which is what keeps a date-filtered list
/// from being crawled once per date.
#[must_use]
pub fn is_identifier(segment: &str) -> bool {
    if segment.is_empty() || segment == ID {
        return false;
    }
    if segment.bytes().all(|byte| byte.is_ascii_digit()) {
        return true;
    }
    if is_uuid(segment) {
        return true;
    }
    segment.bytes().any(|byte| byte.is_ascii_digit())
}

/// Whether `value` is a UUID in its canonical 8-4-4-4-12 form.
#[must_use]
pub fn is_uuid(value: &str) -> bool {
    value.len() == 36
        && value.char_indices().all(|(at, ch)| match at {
            8 | 13 | 18 | 23 => ch == '-',
            _ => ch.is_ascii_hexdigit(),
        })
}

/// The absolute URL for a path on `origin`.
#[must_use]
pub fn url_for(origin: &str, path: &str) -> String {
    if path.starts_with("http://") || path.starts_with("https://") {
        return path.to_owned();
    }
    if path.starts_with('/') {
        return format!("{origin}{path}");
    }
    format!("{origin}/{path}")
}

/// Whether a URL is on the app's own origin, which is the only place the crawl follows.
///
/// Everything else -- another host, `mailto:`, `tel:`, `javascript:`, a download -- is recorded and
/// never fetched (M4-T01). This is a pure comparison, so the decision does not depend on the network.
#[must_use]
pub fn is_own_origin(origin: &str, url: &str) -> bool {
    match (origin_of(origin), origin_of(url)) {
        (Some(app), Some(candidate)) => app == candidate,
        _ => false,
    }
}

/// The scheme and authority of a URL, without a trailing slash.
fn origin_of(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let authority = rest
        .split(['/', '?', '#'])
        .next()
        .filter(|part| !part.is_empty())?;
    Some(format!("{scheme}://{authority}").to_lowercase())
}
