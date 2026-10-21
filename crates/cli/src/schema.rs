//! `substance schema export` and `substance schema validate` (M0-T07).
//!
//! Export builds the JSON Schemas and the driver's TypeScript types in core, then writes
//! them here: core does no IO. Validate reads a document, parses it (TOML for the config
//! kind), and reports every schema error with its JSON pointer.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use substance_core::schema_export::{TYPESCRIPT_DIR, json_schemas, typescript_files};

use crate::cli::{SchemaExportArgs, SchemaKind, SchemaValidateArgs};
use crate::error::{CliError, EXIT_FAILURE, EXIT_OK};
use crate::output::stdout_line;

/// The subdirectory of `--out` that holds the JSON Schemas.
const JSON_DIR: &str = "json";

/// Runs `schema export`.
///
/// The JSON Schemas go under `<out>/json/`, and the driver's types always go to
/// [`TYPESCRIPT_DIR`], whatever `--out` says. Both directories are replaced wholesale, so
/// a type that disappears from core cannot leave a stale file behind.
///
/// # Errors
///
/// Returns [`CliError::Internal`] when a schema cannot be built or a file cannot be written.
pub fn export(args: &SchemaExportArgs) -> Result<ExitCode, CliError> {
    let out = args.out.as_path();
    let json_dir = out.join(JSON_DIR);
    replace_directory(&json_dir, "schema", |push| {
        for file in json_schemas().map_err(|error| {
            CliError::Internal(format!("cannot build the JSON schemas: {error}"))
        })? {
            push(
                PathBuf::from(format!("{}.schema.json", file.slug)),
                file.bytes,
            );
        }
        Ok(())
    })?;
    replace_directory(Path::new(TYPESCRIPT_DIR), "typescript", |push| {
        for file in typescript_files().map_err(|error| {
            CliError::Internal(format!("cannot build the TypeScript types: {error}"))
        })? {
            push(PathBuf::from(file.name), file.contents.into_bytes());
        }
        Ok(())
    })?;
    stdout_line(&format!(
        "wrote {} and {}",
        json_dir.display(),
        TYPESCRIPT_DIR
    ))?;
    Ok(ExitCode::from(EXIT_OK))
}

/// Replaces `target` with exactly the files `build` pushes.
///
/// The files are first written into a fresh temporary directory beside the target (under
/// `target/` in the repository, so the rename stays on one filesystem), then the old
/// directory is removed and the new one renamed into place. Nothing but the pushed files
/// can survive the swap.
fn replace_directory(
    target: &Path,
    label: &str,
    build: impl FnOnce(&mut dyn FnMut(PathBuf, Vec<u8>)) -> Result<(), CliError>,
) -> Result<(), CliError> {
    let staging_root = Path::new("target").join(format!("schema-export-{label}"));
    if staging_root.exists() {
        remove_dir(&staging_root)?;
    }
    fs::create_dir_all(&staging_root).map_err(|error| {
        CliError::Internal(format!("cannot create {}: {error}", staging_root.display()))
    })?;
    let mut write_error: Option<CliError> = None;
    {
        let mut push = |name: PathBuf, bytes: Vec<u8>| {
            if write_error.is_some() {
                return;
            }
            if let Err(error) = write_file(&staging_root, &name, &bytes) {
                write_error = Some(error);
            }
        };
        build(&mut push)?;
    }
    if let Some(error) = write_error {
        return Err(error);
    }
    if target.exists() {
        remove_dir(target)?;
    }
    if let Some(parent) = target.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).map_err(|error| {
            CliError::Internal(format!("cannot create {}: {error}", parent.display()))
        })?;
    }
    fs::rename(&staging_root, target).map_err(|error| {
        CliError::Internal(format!(
            "cannot move {} to {}: {error}",
            staging_root.display(),
            target.display()
        ))
    })
}

/// Writes one file inside the staging directory.
fn write_file(root: &Path, name: &Path, bytes: &[u8]) -> Result<(), CliError> {
    let path = root.join(name);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            CliError::Internal(format!("cannot create {}: {error}", parent.display()))
        })?;
    }
    fs::write(&path, bytes)
        .map_err(|error| CliError::Internal(format!("cannot write {}: {error}", path.display())))
}

/// Removes a directory tree.
fn remove_dir(path: &Path) -> Result<(), CliError> {
    fs::remove_dir_all(path)
        .map_err(|error| CliError::Internal(format!("cannot remove {}: {error}", path.display())))
}

/// Runs `schema validate`.
///
/// The schema comes from the compiled types, not from the exported files, so validating
/// never depends on whether `schema export` has run.
///
/// # Errors
///
/// Returns [`CliError::Usage`] (exit 2) when the file cannot be read or parsed, and
/// [`CliError::Internal`] when the schema itself cannot be built.
pub fn validate(args: &SchemaValidateArgs) -> Result<ExitCode, CliError> {
    let Some(kind) = core_kind(args.kind) else {
        return Err(CliError::NotImplemented("M5"));
    };
    let instance = read_instance(args.kind, args.file.as_path())?;
    let schema = kind
        .schema()
        .map_err(|error| CliError::Internal(format!("cannot build the schema: {error}")))?;
    let validator = jsonschema::validator_for(&schema)
        .map_err(|error| CliError::Internal(format!("cannot compile the schema: {error}")))?;
    let mut errors: Vec<String> = validator
        .iter_errors(&instance)
        .map(|error| {
            let pointer = error.instance_path().to_string();
            let pointer = if pointer.is_empty() {
                String::from("/")
            } else {
                pointer
            };
            format!("{}: {}", pointer, error)
        })
        .collect();
    errors.sort();
    if errors.is_empty() {
        return Ok(ExitCode::from(EXIT_OK));
    }
    for error in &errors {
        stdout_line(error.as_str())?;
    }
    Ok(ExitCode::from(EXIT_FAILURE))
}

/// Maps the CLI's kind to a core kind; `injection` has no type until M5-T01.
fn core_kind(kind: SchemaKind) -> Option<substance_core::schema_export::SchemaKind> {
    use substance_core::schema_export::SchemaKind as Core;
    match kind {
        SchemaKind::EffectRecord => Some(Core::EffectRecord),
        SchemaKind::Finding => Some(Core::Finding),
        SchemaKind::Manifest => Some(Core::Manifest),
        SchemaKind::Report => Some(Core::Report),
        SchemaKind::Config => Some(Core::Config),
        SchemaKind::Injection => None,
    }
}

/// Reads the document to validate.
///
/// JSON is the default. The config kind also accepts TOML, which is converted to JSON
/// first; the file extension decides which parser runs.
fn read_instance(kind: SchemaKind, path: &Path) -> Result<serde_json::Value, CliError> {
    let text = fs::read_to_string(path)
        .map_err(|error| CliError::Usage(format!("cannot read {}: {error}", path.display())))?;
    if kind == SchemaKind::Config && path.extension().is_some_and(|ext| ext == "toml") {
        let value: toml::Value = toml::from_str(text.as_str())
            .map_err(|error| CliError::Usage(format!("{}: {error}", path.display())))?;
        return serde_json::to_value(value)
            .map_err(|error| CliError::Usage(format!("{}: {error}", path.display())));
    }
    serde_json::from_str(text.as_str())
        .map_err(|error| CliError::Usage(format!("{}: {error}", path.display())))
}
