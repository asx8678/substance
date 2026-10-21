//! Creating the test accounts a run needs (M2-T04, ADR-0019).
//!
//! ## The methods, and why each exists
//!
//! | `create` | how | why it is not the only one |
//! | --- | --- | --- |
//! | `api` | the `[signup]` request template | needs the app to accept JSON signup |
//! | `sql` | an `INSERT` against the run database | needs the schema, which is the app's |
//! | `supabase` | the GoTrue admin API | only for a Supabase stack |
//! | `ui` | the app's own signup form | the driver lands in M4 |
//! | `existing` | nothing: the account must appear after seeding | only for a repo that seeds users |
//!
//! A config names one per account because no single method works for every app, and the run should
//! use the cheapest that does. `existing` is cheapest of all and the most fragile: it means the
//! repository's own seed created the account, and M4-T01 verifies it can log in rather than
//! assuming it.
//!
//! ## Passwords never leave memory
//!
//! A `[[users]]` entry names an environment variable, not a password.
//!
//! The value is read here, used to fill one template, and dropped. It is never written to the run
//! directory, the manifest, a log or an error: [`render`] returns a body the caller sends and does
//! not store, every error names the account by its handle, and [`Secret`]'s `Debug` prints
//! `<redacted>` so a stray `{:?}` cannot leak one either.
//!
//! ## Why substitution is a function, not a `format!`
//!
//! The template is data from the repository's config, so a `{field}` it does not recognise must be
//! an error naming the field rather than a panic or a silently empty string. A body with an empty
//! password would create an account that cannot log in, and the failure would surface in M4 as
//! "login fails" with no sign the real cause was a typo in a template.

use substance_core::config::{ProjectConfig, UserConfig, UserCreate};

/// A password, readable only on purpose.
///
/// A newtype rather than a `String` so the one place that needs the characters must ask for them by
/// name. `Debug` and `Display` both print a redaction, because those are the two ways a value leaks
/// into an error message.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    /// Wraps a value read from the environment.
    #[must_use]
    pub fn new(value: String) -> Self {
        Self(value)
    }

    /// The characters, for the one call that fills a template.
    #[must_use]
    pub fn expose(&self) -> &str {
        self.0.as_str()
    }

    /// Whether the value is empty, which is a config problem rather than a login problem.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("<redacted>")
    }
}

impl std::fmt::Display for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("<redacted>")
    }
}

/// One value a signup template can use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placeholder {
    /// `{email}`
    Email,
    /// `{password}`
    Password,
    /// `{handle}`
    Handle,
}

impl Placeholder {
    /// The name as a template writes it, without braces.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Email => "email",
            Self::Password => "password",
            Self::Handle => "handle",
        }
    }

    /// The placeholder a name refers to, when it is one this module knows.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "email" => Some(Self::Email),
            "password" => Some(Self::Password),
            "handle" => Some(Self::Handle),
            _ => None,
        }
    }

    /// Every placeholder, in a fixed order.
    pub const ALL: &'static [Self] = &[Self::Email, Self::Password, Self::Handle];
}

/// Why a template could not be filled, or an account cannot be created.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AccountError {
    /// The template names a placeholder this build does not fill.
    #[error("{template}: unknown placeholder {{{name}}}")]
    UnknownPlaceholder {
        /// Which template, for the message.
        template: String,
        /// The name it used.
        name: String,
    },
    /// The template opens `{` and never closes it.
    #[error("{template}: an unclosed '{{' at byte {at}")]
    UnclosedPlaceholder {
        /// Which template.
        template: String,
        /// Where the brace was.
        at: usize,
    },
    /// The account names an environment variable that is not set.
    #[error("users: {handle} names {variable}, which is not set")]
    MissingPassword {
        /// The account's handle.
        handle: String,
        /// The variable it named.
        variable: String,
    },
    /// The account names a variable that is set but empty.
    #[error("users: {handle} names {variable}, which is empty")]
    EmptyPassword {
        /// The account's handle.
        handle: String,
        /// The variable it named.
        variable: String,
    },
}

/// Fills a signup body template.
///
/// Substitutes `{email}`, `{password}` and `{handle}`; everything else, including a lone `}` with no
/// opener, is copied through. `{{` is an escaped brace and becomes one `{`, so a body that needs a
/// literal brace has a way to say so.
///
/// # Errors
///
/// Returns [`AccountError::UnknownPlaceholder`] for a `{name}` this build does not fill and
/// [`AccountError::UnclosedPlaceholder`] for a `{` that never closes. Both are config problems and
/// name the template, because a template is written once and used for every account.
pub fn render(
    template: &str,
    template_name: &str,
    email: &str,
    password: &Secret,
    handle: &str,
) -> Result<String, AccountError> {
    let mut out = String::with_capacity(template.len() + email.len() + handle.len());
    let mut chars = template.chars().peekable();
    while let Some(character) = chars.next() {
        match character {
            // `{{` and `}}` are literal braces, checked wherever they appear. A JSON body template
            // needs both: without the escaped closer, `{\"a\":\"{handle}\"}` would end with a
            // doubled brace and there would be no way to write a well-formed body at all.
            '{' if chars.peek() == Some(&'{') => {
                let _ = chars.next();
                out.push('{');
            }
            '}' if chars.peek() == Some(&'}') => {
                let _ = chars.next();
                out.push('}');
            }
            '{' => {
                let mut name = String::new();
                let mut closed = false;
                for next in chars.by_ref() {
                    if next == '}' {
                        closed = true;
                        break;
                    }
                    name.push(next);
                }
                if !closed {
                    return Err(AccountError::UnclosedPlaceholder {
                        template: template_name.to_owned(),
                        at: template.len() - name.len() - 1,
                    });
                }
                let Some(placeholder) = Placeholder::parse(&name) else {
                    return Err(AccountError::UnknownPlaceholder {
                        template: template_name.to_owned(),
                        name,
                    });
                };
                out.push_str(match placeholder {
                    Placeholder::Email => email,
                    Placeholder::Password => password.expose(),
                    Placeholder::Handle => handle,
                });
            }
            // A lone `}` cannot be a placeholder, so it is copied through: rejecting it would stop
            // a body template from containing one at all.
            other => out.push(other),
        }
    }
    Ok(out)
}

/// A signup request with the password already filled in.
///
/// Held only long enough to send it. Deliberately not `Clone`, not `Serialize`, and with a `Debug`
/// that redacts the body: the body contains a password, and none of those traits has a use here
/// worth the risk of one being called on the way to a log.
pub struct SignupRequest {
    /// The account's handle, for messages.
    pub handle: String,
    /// The HTTP method.
    pub method: String,
    /// The path, relative to the app's origin.
    pub path: String,
    /// The rendered body, when the template has one.
    pub body: Option<String>,
}

impl std::fmt::Debug for SignupRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SignupRequest")
            .field("handle", &self.handle)
            .field("method", &self.method)
            .field("path", &self.path)
            .field("body", &self.body.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

/// What one account needs before it can be created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedAccount {
    /// The account's configuration.
    pub user: UserConfig,
    /// The canary value written into its data.
    pub canary: String,
}

/// Reads the password for one account from the environment.
///
/// # Errors
///
/// Returns [`AccountError::MissingPassword`] when the variable is unset and
/// [`AccountError::EmptyPassword`] when it is empty: an empty password is not a password, and an
/// account created with one produces a login failure in M4 that looks like the app's fault.
pub fn password_for(user: &UserConfig) -> Result<Secret, AccountError> {
    let value = std::env::var(&user.password_env).map_err(|_| AccountError::MissingPassword {
        handle: user.handle.clone(),
        variable: user.password_env.clone(),
    })?;
    if value.is_empty() {
        return Err(AccountError::EmptyPassword {
            handle: user.handle.clone(),
            variable: user.password_env.clone(),
        });
    }
    Ok(Secret::new(value))
}

/// The accounts a run will create, and why any of them cannot be.
///
/// Pure with respect to the environment: it reads no password and spawns nothing, so a caller can
/// ask what a config implies before it has a database, an app or a permit.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AccountPlan {
    /// One entry per `[[users]]` account, in config order.
    pub accounts: Vec<PlannedAccount>,
    /// Why an account cannot be created, sorted.
    pub problems: Vec<String>,
    /// Notes about methods that are deferred rather than impossible, sorted.
    pub notes: Vec<String>,
}

impl AccountPlan {
    /// Whether every account can be created.
    #[must_use]
    pub fn ready(&self) -> bool {
        self.problems.is_empty() && !self.accounts.is_empty()
    }

    /// The accounts of one role.
    #[must_use]
    pub fn in_role(&self, role: &str) -> Vec<&PlannedAccount> {
        self.accounts
            .iter()
            .filter(|account| account.user.role == role)
            .collect()
    }

    /// Plans every account in `cfg` for a run whose master seed is `master_seed`.
    ///
    /// A method this build cannot run yet is a note, not a problem: `ui` needs the driver (M4) and
    /// the checks that need a second user are the ones that should report `not_measured`, not the
    /// whole run. A method that is impossible for *this* config -- `sql` with no table to insert
    /// into, `api` with no `[signup] api` -- is a problem, because no later milestone fixes it.
    #[must_use]
    pub fn of(cfg: &ProjectConfig, master_seed: u64) -> Self {
        let mut plan = Self::default();
        for user in &cfg.users {
            let canary = substance_core::model::canary_value(master_seed, &user.handle);
            match user.create {
                UserCreate::Api => match &cfg.signup.api {
                    Some(_) => plan.accounts.push(PlannedAccount {
                        user: user.clone(),
                        canary,
                    }),
                    None => {
                        // Planned anyway, so the account still owns a canary: B13's file is about
                        // values substance generated for each user, and a user whose creation is
                        // blocked still has an identity the checks name.
                        plan.accounts.push(PlannedAccount {
                            user: user.clone(),
                            canary,
                        });
                        plan.problems.push(format!(
                            "users: {} asks for the api method but [signup] api is not set",
                            user.handle
                        ));
                    }
                },
                UserCreate::Existing => plan.accounts.push(PlannedAccount {
                    user: user.clone(),
                    canary,
                }),
                UserCreate::Sql => {
                    // Planned, with a canary, for the same reason as `api` above: the account's
                    // identity exists in the run's records even when nothing could create it.
                    plan.accounts.push(PlannedAccount {
                        user: user.clone(),
                        canary,
                    });
                    plan.notes.push(format!(
                        "users: {} asks for the sql method, which needs a schema this build does not read; \
                         it is not created",
                        user.handle
                    ));
                }
                UserCreate::Supabase => plan.accounts.push(PlannedAccount {
                    user: user.clone(),
                    canary,
                }),
                UserCreate::Ui => {
                    plan.accounts.push(PlannedAccount {
                        user: user.clone(),
                        canary,
                    });
                    plan.notes.push(format!(
                        "users: {} asks for the ui method, which arrives with the driver in M4",
                        user.handle
                    ));
                }
            }
        }
        // ADR-0019 wants two accounts per role: several checks compare two users, and one per role
        // cannot. A note rather than a problem, because the run is still useful; the checks that need
        // a second user report `not_measured`.
        let mut roles: std::collections::BTreeMap<&str, u32> = std::collections::BTreeMap::new();
        for account in &plan.accounts {
            *roles.entry(account.user.role.as_str()).or_default() += 1;
        }
        for (role, count) in roles {
            if count < 2 {
                plan.notes.push(format!(
                    "users: role {role:?} has {count} account(s); checks that compare two users per \
                     role report not_measured"
                ));
            }
        }
        plan.problems.sort();
        plan.notes.sort();
        plan.notes.dedup();
        plan
    }
}
