//! Which authors are bots (M1-T02).
//!
//! History checks count humans: a bot that commits every night inflates "active weeks",
//! and a bot that opens issues is not outside usage. Two signals identify one: every
//! GitHub App account carries `[bot]` in its name or email, and the forge reports
//! `type: "Bot"` for the same accounts.

/// Whether `name` or `email` marks a bot, or `forge_type` says so.
///
/// `forge_type` is the account type from the forge's API (`"Bot"` or `"User"`); `None`
/// when no forge data was fetched, which is the normal case for triage.
#[must_use]
pub fn is_bot(name: &str, email: &str, forge_type: Option<&str>) -> bool {
    if forge_type.is_some_and(|kind| kind.eq_ignore_ascii_case("bot")) {
        return true;
    }
    name.contains("[bot]")
        || email.contains("[bot]")
        || name.to_ascii_lowercase().ends_with("-bot")
        || email
            .to_ascii_lowercase()
            .ends_with("-bot@users.noreply.github.com")
}

/// Whether a commit's author is a bot.
#[must_use]
pub fn is_bot_commit(author: &super::CommitIdentity) -> bool {
    is_bot(&author.name, &author.email, None)
}
