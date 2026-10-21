//! Building the committed artifacts: JSON Schemas and TypeScript declarations (M0-T07).
//!
//! This module only builds bytes; writing them is the CLI's job, because core does no IO.
//! Both outputs are deterministic: JSON keys are sorted with two-space indentation and a
//! trailing newline, and the TypeScript files are sorted by name, so running
//! `substance schema export` twice produces identical files.

use std::any::TypeId;
use std::collections::BTreeSet;

use schemars::generate::SchemaSettings;
use ts_rs::{Config, TS, TypeVisitor};

use crate::config::ProjectConfig;
use crate::hash::{CanonicalJsonError, canonical_json_pretty_value};
use crate::model::{
    AppModel, CheckVerdict, DriverCommand, DriverEvent, EffectRecord, Finding, RecordPart, Report,
    RunManifest,
};

/// Where the driver's generated types live, relative to the repository root.
///
/// `schema export` writes here whatever its `--out` says, so `tsc` can compile the driver
/// without reaching outside `driver/`.
pub const TYPESCRIPT_DIR: &str = "driver/src/generated";

/// A schema document that is written to disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaFile {
    /// The slug, also the file name without the extension: `effect-record`.
    pub slug: &'static str,
    /// The canonical pretty JSON, ending with a newline.
    pub bytes: Vec<u8>,
}

/// A TypeScript file that is written to disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeScriptFile {
    /// The file name, for example `EffectRecord.ts`.
    pub name: String,
    /// The file contents, ending with a newline.
    pub contents: String,
}

/// Why TypeScript could not be generated.
#[derive(Debug, thiserror::Error)]
pub enum TypeScriptError {
    /// A type has no TypeScript declaration, or one of its dependencies does not.
    #[error("cannot generate TypeScript: {0}")]
    Export(#[from] ts_rs::ExportError),
    /// A generated file has no usable name.
    #[error("generated TypeScript has no file name")]
    MissingName,
}

/// The documents `schema validate` understands.
///
/// `injection` is absent on purpose: its type arrives with the benchmark in M5-T01.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SchemaKind {
    /// One effect record.
    EffectRecord,
    /// One finding.
    Finding,
    /// One check verdict.
    CheckVerdict,
    /// The app model.
    AppModel,
    /// A run manifest.
    Manifest,
    /// A report.
    Report,
    /// A `substance.toml`.
    Config,
    /// One driver command.
    DriverCommand,
    /// One driver event.
    DriverEvent,
    /// One record part.
    RecordPart,
}

impl SchemaKind {
    /// Every kind, in the order `schema export` writes them.
    pub const ALL: &'static [Self] = &[
        Self::EffectRecord,
        Self::Finding,
        Self::CheckVerdict,
        Self::AppModel,
        Self::Manifest,
        Self::Report,
        Self::Config,
        Self::DriverCommand,
        Self::DriverEvent,
        Self::RecordPart,
    ];

    /// The kebab-case slug, used as the file name.
    #[must_use]
    pub const fn slug(self) -> &'static str {
        match self {
            Self::EffectRecord => "effect-record",
            Self::Finding => "finding",
            Self::CheckVerdict => "check-verdict",
            Self::AppModel => "app-model",
            Self::Manifest => "manifest",
            Self::Report => "report",
            Self::Config => "config",
            Self::DriverCommand => "driver-command",
            Self::DriverEvent => "driver-event",
            Self::RecordPart => "record-part",
        }
    }

    /// Builds the JSON Schema for this kind, as a JSON value.
    ///
    /// # Errors
    ///
    /// Returns [`CanonicalJsonError`] if the schema cannot be represented as JSON, which
    /// would be a bug in schemars.
    pub fn schema(self) -> Result<serde_json::Value, CanonicalJsonError> {
        let settings = SchemaSettings::draft2020_12();
        let generator = settings.into_generator();
        let schema = match self {
            Self::EffectRecord => generator.into_root_schema_for::<EffectRecord>(),
            Self::Finding => generator.into_root_schema_for::<Finding>(),
            Self::CheckVerdict => generator.into_root_schema_for::<CheckVerdict>(),
            Self::AppModel => generator.into_root_schema_for::<AppModel>(),
            Self::Manifest => generator.into_root_schema_for::<RunManifest>(),
            Self::Report => generator.into_root_schema_for::<Report>(),
            Self::Config => generator.into_root_schema_for::<ProjectConfig>(),
            Self::DriverCommand => generator.into_root_schema_for::<DriverCommand>(),
            Self::DriverEvent => generator.into_root_schema_for::<DriverEvent>(),
            Self::RecordPart => generator.into_root_schema_for::<RecordPart>(),
        };
        Ok(serde_json::to_value(schema)?)
    }

    /// Builds the canonical pretty JSON of this kind's schema.
    ///
    /// # Errors
    ///
    /// Returns [`CanonicalJsonError`] when the schema cannot be built.
    pub fn bytes(self) -> Result<Vec<u8>, CanonicalJsonError> {
        let value = self.schema()?;
        Ok(canonical_json_pretty_value(&value))
    }
}

/// Builds every committed JSON Schema, in [`SchemaKind::ALL`] order.
///
/// # Errors
///
/// Returns [`CanonicalJsonError`] when a schema cannot be built.
pub fn json_schemas() -> Result<Vec<SchemaFile>, CanonicalJsonError> {
    let mut files = Vec::with_capacity(SchemaKind::ALL.len());
    for kind in SchemaKind::ALL {
        files.push(SchemaFile {
            slug: kind.slug(),
            bytes: kind.bytes()?,
        });
    }
    Ok(files)
}

/// Builds every TypeScript file for the committed types and their dependencies.
///
/// The files are sorted by name. Files are generated with [`TYPESCRIPT_DIR`] as the base
/// directory so that the relative imports in the output (`./Finding`) are the ones the
/// driver sees, whichever directory the CLI finally writes them into.
///
/// # Errors
///
/// Returns [`TypeScriptError`] when a type cannot be exported.
pub fn typescript_files() -> Result<Vec<TypeScriptFile>, TypeScriptError> {
    let config = Config::new().with_out_dir(TYPESCRIPT_DIR);
    let mut collector = Collector {
        config: &config,
        seen: BTreeSet::new(),
        files: Vec::new(),
        error: None,
    };
    collector.visit::<EffectRecord>();
    collector.visit::<Finding>();
    collector.visit::<CheckVerdict>();
    collector.visit::<AppModel>();
    collector.visit::<RunManifest>();
    collector.visit::<Report>();
    collector.visit::<ProjectConfig>();
    collector.visit::<DriverCommand>();
    collector.visit::<DriverEvent>();
    collector.visit::<RecordPart>();
    if let Some(error) = collector.error {
        return Err(error);
    }
    let mut files = collector.files;
    files.sort_by(|left, right| left.name.cmp(&right.name));
    files.dedup_by(|left, right| left.name == right.name);
    Ok(files)
}

/// Walks the type graph, rendering each exportable type once.
struct Collector<'a> {
    /// The ts-rs settings, including the base directory the imports are relative to.
    config: &'a Config,
    /// Types already rendered, so a diamond in the graph is visited once.
    seen: BTreeSet<TypeId>,
    /// The rendered files, in visit order until [`typescript_files`] sorts them.
    files: Vec<TypeScriptFile>,
    /// The first export failure, which stops the walk.
    error: Option<TypeScriptError>,
}

impl TypeVisitor for Collector<'_> {
    fn visit<T: TS + 'static + ?Sized>(&mut self) {
        if self.error.is_some() {
            return;
        }
        if !self.seen.insert(TypeId::of::<T>()) {
            return;
        }
        let Some(path) = T::output_path() else {
            return;
        };
        let contents = match T::export_to_string(self.config) {
            Ok(contents) => contents,
            Err(error) => {
                self.error = Some(TypeScriptError::Export(error));
                return;
            }
        };
        let Some(name) = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
        else {
            self.error = Some(TypeScriptError::MissingName);
            return;
        };
        self.files.push(TypeScriptFile { name, contents });
        T::visit_dependencies(self);
    }
}
