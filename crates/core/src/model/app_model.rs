//! The app model: what exploration found (M4-T01).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Everything the crawler learned about the app at one commit.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct AppModel {
    /// The commit the model describes.
    pub commit_sha: String,
    /// The cache key: commit SHA, config hash, master seed, rule-pack hash and driver
    /// versions, so a stale model is never reused (review issue 40).
    pub cache_key: String,
    /// Screens in crawl order.
    pub screens: Vec<Screen>,
    /// Controls and forms in screen order.
    pub controls: Vec<Control>,
    /// The request map: which callable triggers which endpoint.
    pub requests: Vec<RequestTrigger>,
    /// Entities inferred from responses and database tables.
    pub entities: Vec<Entity>,
    /// Test users per role; role and handle only, never secrets.
    pub users: Vec<TestUser>,
    /// Selectors the agent layer supplied, when the feature is on.
    pub labels: Option<Labels>,
    /// Tables the crawl wrote to, sorted.
    pub tables_touched: Vec<String>,
}

/// One route the crawl reached.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct Screen {
    /// The route template, for example `"/invoices/:id"`.
    pub route: String,
    /// Crawl depth from the entry point.
    pub depth: u32,
    /// Whether the screen needs a session.
    pub requires_auth: bool,
}

/// One interactive control, with its selector candidates in preference order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct Control {
    /// The route the control is on.
    pub screen: String,
    /// The ARIA role.
    pub role: String,
    /// The accessible name.
    pub name: String,
    /// Selector candidates: role plus name first, then test ID, then CSS.
    pub selectors: Vec<String>,
    /// The form the control belongs to, when it is in one.
    pub form: Option<Form>,
    /// `login`, `logout`, `submit` or `action`.
    pub purpose: ControlPurpose,
}

/// A form and its fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct Form {
    /// The form's selector.
    pub selector: String,
    /// Fields in document order.
    pub fields: Vec<Field>,
}

/// One form field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct Field {
    /// The field name.
    pub name: String,
    /// `text`, `email`, `password`, `number`, `date`, `checkbox` or `select`.
    pub kind: String,
    /// Whether the field is required.
    pub required: bool,
    /// The HTML `pattern`, when present.
    pub pattern: Option<String>,
    /// Minimum length or value, when present.
    pub min: Option<String>,
    /// Maximum length or value, when present.
    pub max: Option<String>,
}

/// What a control is for.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema, TS,
)]
#[serde(rename_all = "snake_case")]
pub enum ControlPurpose {
    /// The login control.
    Login,
    /// The logout control.
    Logout,
    /// A form submit control.
    Submit,
    /// Anything else.
    Action,
}

/// Which control and step trigger which endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct RequestTrigger {
    /// The control's selector.
    pub control: String,
    /// The route the control was used on.
    pub screen: String,
    /// The HTTP method.
    pub method: String,
    /// The path, with query parameters.
    pub path: String,
}

/// An entity inferred from JSON arrays and database tables.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct Entity {
    /// The entity name, usually the table or the JSON key.
    pub name: String,
    /// The field names, sorted.
    pub fields: Vec<String>,
    /// The identifier field, when one was recognized.
    pub id_field: Option<String>,
}

/// A test account: role and handle only, never a password.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct TestUser {
    /// The role, for example `"user"` or `"admin"`.
    pub role: String,
    /// The handle from `[[users]]`.
    pub handle: String,
}

/// Selectors the agent layer labeled (M12); absent while the feature is off.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct Labels {
    /// The success-signal selector.
    pub success_signal: Option<String>,
    /// The route of the entity list screen.
    pub list_screen: Option<String>,
    /// The route of the entity detail screen.
    pub detail_screen: Option<String>,
    /// The search field selector.
    pub search_field: Option<String>,
}
