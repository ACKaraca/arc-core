//! The headless runner (CORE-CMD-031, feature `headless`): the subcommands, flags and exit codes
//! every `<product>-cli` shares.
//!
//! [`run`] is the whole command-line program as a library function: it takes the arguments
//! (without the program name), the [`Product`] and the three standard streams, and returns the
//! [`Exit`] code. A product's `main` only gathers these and returns the code, so tests call the
//! runner in-process. Standard output carries JSON only (one value per line, or the protocol
//! messages of `serve --stdio`); diagnostics and errors go to standard error as JSON lines.
//! Arguments are parsed by hand. The rules are specified in `docs/crates/arc-command.md`,
//! section "Headless runner as implemented".

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::io::{self, BufRead, Read, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use arc_base::{Error, Limits};
use arc_doc::graph::Evaluators;
use arc_doc::package::{self, LockOwner, Mode, Package};
use arc_doc::{Document, DocumentConfig, LoadOptions, TypeRegistry};
use serde_json::{Map, Value, json};

use crate::context::Clock;
use crate::error::codes;
use crate::protocol::{Log, LogLevel, Server, ServerConfig, errors};
use crate::registry::Registry;
use crate::roots::Roots;
use crate::schema::parse_params;
use crate::spec::Permission;

/// The exit code of a runner invocation; the numbers follow the headless CLI conventions of
/// the automation architecture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Exit {
    /// 0: the operation succeeded.
    Success,
    /// 1: the command or operation failed (not found, conflict, cancelled, input/output or a
    /// failed operation).
    Failed,
    /// 2: the arguments do not follow the grammar, or a required command parameter is missing.
    Usage,
    /// 3: the input is invalid or corrupt: parameters that fail validation, a package check
    /// with findings, malformed data.
    Invalid,
    /// 4: an unsupported format, version or feature.
    Unsupported,
    /// 5: a permission class is not granted or a path lies outside the roots.
    Denied,
    /// 6: a limit was exceeded.
    Limit,
    /// 70: an internal fault.
    Internal,
}

impl Exit {
    /// Every exit code, in numeric order.
    pub const ALL: [Self; 8] = [
        Self::Success,
        Self::Failed,
        Self::Usage,
        Self::Invalid,
        Self::Unsupported,
        Self::Denied,
        Self::Limit,
        Self::Internal,
    ];

    /// The process exit code.
    #[must_use]
    pub const fn code(self) -> u8 {
        match self {
            Self::Success => 0,
            Self::Failed => 1,
            Self::Usage => 2,
            Self::Invalid => 3,
            Self::Unsupported => 4,
            Self::Denied => 5,
            Self::Limit => 6,
            Self::Internal => 70,
        }
    }

    /// The exit code of an error of `category` (an [`arc_base::Category`] name).
    fn of_category(category: &str) -> Self {
        match category {
            "invalid_input" | "corrupt_data" => Self::Invalid,
            "unsupported" => Self::Unsupported,
            "permission_denied" => Self::Denied,
            "limit_exceeded" => Self::Limit,
            "internal" => Self::Internal,
            _ => Self::Failed,
        }
    }

    /// The exit code of a JSON-RPC error object: `cmd.cli.usage` and parameters missing a
    /// required member are usage errors; everything else goes by category.
    fn of_error(object: &Value) -> Self {
        let data = &object["data"];
        let required = data["details"]["violations"]
            .as_array()
            .is_some_and(|v| v.iter().any(|v| v["kind"] == "required"));
        match data["code"].as_str() {
            Some("cmd.cli.usage") => Self::Usage,
            Some("cmd.params.invalid") if required => Self::Usage,
            _ => Self::of_category(data["category"].as_str().unwrap_or("internal")),
        }
    }
}

/// What a product gives the runner: its name and version, its object types and commands, and
/// the clock and edit-lock owner of the process.
pub struct Product {
    /// The product name reported by `arc.initialize`.
    pub name: String,
    /// The product version reported by `arc.initialize`.
    pub version: String,
    /// The object types documents are loaded with.
    pub types: Arc<TypeRegistry>,
    /// The node evaluators of documents.
    pub evaluators: Arc<Evaluators>,
    /// The commands.
    pub registry: Registry,
    /// The clock of command sessions.
    pub clock: Arc<dyn Clock>,
    /// The owner written to the edit lock of a package opened for saving.
    pub owner: LockOwner,
}

impl std::fmt::Debug for Product {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Product")
            .field("name", &self.name)
            .field("version", &self.version)
            .finish_non_exhaustive()
    }
}

/// The usage text written to standard error by `help`.
pub const USAGE: &str = "\
usage: <product>-cli <subcommand> [flags]
  commands                                      list the commands (JSON)
  describe <command>                            describe one command
  validate-params <command> [--params P]        validate parameters without running
  run <file> --command <id> [--params P] [--seed N] [--save]
                                                run one command against a package
  inspect <file> [--check]                      dump a package (and check it)
  validate <file>                               check a package
  serve --stdio [<file>...] [--save]            serve the automation protocol on stdin/stdout
  help                                          this text
flags: --read-root <dir>, --write-root <dir> (repeatable), --allow-edit, --allow-network,
       --allow-process, --json (output is always JSON); P is JSON text or @<file>
";

/// The subcommands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sub {
    Commands,
    Describe,
    ValidateParams,
    Run,
    Inspect,
    Validate,
    Serve,
    Help,
}

/// The parsed arguments.
#[derive(Debug)]
struct Options {
    sub: Sub,
    positional: Vec<OsString>,
    command: Option<String>,
    params: Option<OsString>,
    seed: Option<u64>,
    save: bool,
    check: bool,
    stdio: bool,
    read_roots: Vec<PathBuf>,
    write_roots: Vec<PathBuf>,
    allow: BTreeSet<Permission>,
}

/// A failed invocation: its exit code and the JSON-RPC error object reported on stderr.
#[derive(Debug)]
struct Failure {
    exit: Exit,
    error: Value,
}

impl From<Error> for Failure {
    fn from(error: Error) -> Self {
        let error = errors::error_object(&error, Map::new());
        Self {
            exit: Exit::of_error(&error),
            error,
        }
    }
}

fn usage(reason: impl Into<String>) -> Failure {
    Error::new(codes::CLI_USAGE)
        .with_detail("reason", reason.into())
        .into()
}

/// Runs the command-line program: parses `args` (without the program name), performs the
/// subcommand for `product` with the given streams and returns the exit code. Never panics: a
/// fault is contained and reported as [`Exit::Internal`].
pub fn run<R, W, E>(
    args: impl IntoIterator<Item = OsString>,
    product: &Product,
    stdin: R,
    stdout: W,
    mut stderr: E,
) -> Exit
where
    R: BufRead,
    W: Write + Send,
    E: Write + Send,
{
    let args: Vec<OsString> = args.into_iter().collect();
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        execute(args, product, stdin, stdout, &mut stderr)
    }));
    let failure = match outcome {
        Ok(Ok(exit)) => return exit,
        Ok(Err(failure)) => failure,
        Err(_) => Error::new(codes::FAULT).into(),
    };
    let line = json!({"error": failure.error}).to_string();
    let _ = writeln!(stderr, "{line}");
    failure.exit
}

fn parse(args: Vec<OsString>) -> Result<Options, Failure> {
    let mut args = args.into_iter();
    let sub = match args.next().as_ref().and_then(|a| a.to_str()) {
        Some("commands") => Sub::Commands,
        Some("describe") => Sub::Describe,
        Some("validate-params") => Sub::ValidateParams,
        Some("run") => Sub::Run,
        Some("inspect") => Sub::Inspect,
        Some("validate") => Sub::Validate,
        Some("serve") => Sub::Serve,
        Some("help" | "--help" | "-h") => Sub::Help,
        Some(_) => return Err(usage("unknown_subcommand")),
        None => return Err(usage("no_subcommand")),
    };
    let mut o = Options {
        sub,
        positional: Vec::new(),
        command: None,
        params: None,
        seed: None,
        save: false,
        check: false,
        stdio: false,
        read_roots: Vec::new(),
        write_roots: Vec::new(),
        allow: BTreeSet::new(),
    };
    let mut flags_ended = false;
    while let Some(arg) = args.next() {
        let flag = arg.to_str().filter(|a| !flags_ended && a.starts_with("--"));
        let Some(flag) = flag else {
            o.positional.push(arg);
            continue;
        };
        if flag == "--" {
            flags_ended = true;
            continue;
        }
        let (name, inline) = match flag.split_once('=') {
            Some((name, value)) => (name, Some(OsString::from(value))),
            None => (flag, None),
        };
        flag_into(&mut o, name, inline, &mut args)?;
    }
    check_arity(&o)?;
    Ok(o)
}

/// Applies the flag `name` (with its `=value`, if any) to `o`.
fn flag_into(
    o: &mut Options,
    name: &str,
    inline: Option<OsString>,
    rest: &mut impl Iterator<Item = OsString>,
) -> Result<(), Failure> {
    let sub = o.sub;
    let allowed = |subs: &[Sub]| -> Result<(), Failure> {
        if subs.contains(&sub) || sub == Sub::Help {
            Ok(())
        } else {
            Err(usage(format!("flag_not_allowed:{name}")))
        }
    };
    let has_value = inline.is_some();
    let mut value = || -> Result<OsString, Failure> {
        inline
            .clone()
            .or_else(|| rest.next())
            .ok_or_else(|| usage(format!("missing_value:{name}")))
    };
    let text = |value: OsString| -> Result<String, Failure> {
        value
            .into_string()
            .map_err(|_| usage(format!("not_utf8:{name}")))
    };
    let once = |taken: bool| -> Result<(), Failure> {
        if taken {
            Err(usage(format!("repeated:{name}")))
        } else {
            Ok(())
        }
    };
    match name {
        "--read-root" => o.read_roots.push(PathBuf::from(value()?)),
        "--write-root" => o.write_roots.push(PathBuf::from(value()?)),
        "--command" => {
            allowed(&[Sub::Run])?;
            once(o.command.is_some())?;
            o.command = Some(text(value()?)?);
        }
        "--params" => {
            allowed(&[Sub::Run, Sub::ValidateParams])?;
            once(o.params.is_some())?;
            o.params = Some(value()?);
        }
        "--seed" => {
            allowed(&[Sub::Run])?;
            once(o.seed.is_some())?;
            let seed = text(value()?)?;
            let seed = (!seed.starts_with('+'))
                .then(|| seed.parse::<u64>().ok())
                .flatten()
                .ok_or_else(|| usage("seed"))?;
            o.seed = Some(seed);
        }
        _ => {
            if has_value {
                return Err(usage(format!("unexpected_value:{name}")));
            }
            match name {
                "--allow-edit" => {
                    o.allow.insert(Permission::Edit);
                }
                "--allow-network" => {
                    o.allow.insert(Permission::Network);
                }
                "--allow-process" => {
                    o.allow.insert(Permission::Process);
                }
                "--json" => {}
                "--save" => {
                    allowed(&[Sub::Run, Sub::Serve])?;
                    o.save = true;
                }
                "--check" => {
                    allowed(&[Sub::Inspect])?;
                    o.check = true;
                }
                "--stdio" => {
                    allowed(&[Sub::Serve])?;
                    o.stdio = true;
                }
                "--help" => o.sub = Sub::Help,
                _ => return Err(usage(format!("unknown_flag:{name}"))),
            }
        }
    }
    Ok(())
}

fn check_arity(o: &Options) -> Result<(), Failure> {
    let count = o.positional.len();
    let fits = match o.sub {
        Sub::Commands => count == 0,
        Sub::Describe | Sub::ValidateParams | Sub::Run | Sub::Inspect | Sub::Validate => count == 1,
        Sub::Serve | Sub::Help => true,
    };
    if !fits {
        return Err(usage("arguments"));
    }
    if o.sub == Sub::Run && o.command.is_none() {
        return Err(usage("missing_flag:--command"));
    }
    if o.sub == Sub::Serve && !o.stdio {
        return Err(usage("missing_flag:--stdio"));
    }
    Ok(())
}

/// Writes log records to stderr as JSON lines.
struct StderrLog<E>(Mutex<E>);

impl<E: Write + Send> Log for StderrLog<E> {
    fn log(&self, level: LogLevel, event: &str, detail: &str) {
        let level = match level {
            LogLevel::Debug => "debug",
            LogLevel::Info => "info",
            LogLevel::Warning => "warning",
            LogLevel::Error => "error",
        };
        let line = json!({"level": level, "event": event, "detail": detail}).to_string();
        let mut stderr = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let _ = writeln!(stderr, "{line}");
    }
}

fn execute<R, W, E>(
    args: Vec<OsString>,
    product: &Product,
    stdin: R,
    mut stdout: W,
    stderr: &mut E,
) -> Result<Exit, Failure>
where
    R: BufRead,
    W: Write + Send,
    E: Write + Send,
{
    let o = parse(args)?;
    if o.sub == Sub::Help {
        let _ = stderr.write_all(USAGE.as_bytes());
        return Ok(Exit::Success);
    }
    let limits = Limits::DEFAULT;
    let config = config(&o, product)?;
    let log = StderrLog(Mutex::new(stderr));
    let mut documents = BTreeMap::new();
    let mut packages = Vec::new();
    if matches!(o.sub, Sub::Run | Sub::Serve) {
        for path in &o.positional {
            let path = Path::new(path);
            let key = path
                .file_stem()
                .and_then(|s| s.to_str())
                .ok_or_else(|| usage("document_name"))?
                .to_owned();
            let (package, document) = open(product, path, o.save, &limits)?;
            if documents.insert(key, document).is_some() {
                return Err(usage("duplicate_document_name"));
            }
            packages.push(package);
        }
    }
    let server = Server::new(&product.registry, &documents, config, &log);
    let call = |method: &str, params: Value| server.call(method, params).map_err(failure);
    let name = || -> Result<String, Failure> {
        o.positional
            .first()
            .and_then(|n| n.to_str())
            .map(str::to_owned)
            .ok_or_else(|| usage("not_utf8:command"))
    };
    let file = || o.positional.first().map(Path::new).unwrap_or(Path::new(""));
    let (output, exit) = match o.sub {
        Sub::Commands => (call("command.list", json!({}))?, Exit::Success),
        Sub::Describe => (
            call("command.describe", json!({"command": name()?}))?,
            Exit::Success,
        ),
        Sub::ValidateParams => {
            let params = read_params(o.params.as_ref(), &limits)?;
            let result = call(
                "command.validate",
                json!({"command": name()?, "params": params}),
            )?;
            let exit = if result["valid"] == true {
                Exit::Success
            } else {
                Exit::Invalid
            };
            (result, exit)
        }
        Sub::Run => {
            let params = read_params(o.params.as_ref(), &limits)?;
            let document = documents.keys().next().cloned().unwrap_or_default();
            let result = call(
                "command.run",
                json!({"document": document, "command": o.command, "params": params,
                       "seed": o.seed.unwrap_or(0)}),
            )?;
            if o.save {
                save(&mut packages, &documents)?;
            }
            (result, Exit::Success)
        }
        Sub::Inspect | Sub::Validate => check(o.sub, o.check, file(), &limits)?,
        Sub::Serve | Sub::Help => {
            server.serve(stdin, &mut stdout).map_err(io_failure)?;
            if o.save {
                save(&mut packages, &documents)?;
            }
            return Ok(Exit::Success);
        }
    };
    writeln!(stdout, "{output}")
        .and_then(|()| stdout.flush())
        .map_err(io_failure)?;
    Ok(exit)
}

/// The protocol configuration of the flags: the ceiling is `read`, the classes of the
/// `--allow-*` flags, `file_read` with any root and `file_write` with a write root.
fn config(o: &Options, product: &Product) -> Result<ServerConfig, Failure> {
    let mut ceiling = o.allow.clone();
    ceiling.insert(Permission::Read);
    if !o.read_roots.is_empty() || !o.write_roots.is_empty() {
        ceiling.insert(Permission::FileRead);
    }
    if !o.write_roots.is_empty() {
        ceiling.insert(Permission::FileWrite);
    }
    let mut config = ServerConfig::new(&product.name, &product.version, ceiling);
    config.roots = Roots::new(&o.read_roots, &o.write_roots)?;
    config.clock = Arc::clone(&product.clock);
    Ok(config)
}

fn failure(error: Value) -> Failure {
    Failure {
        exit: Exit::of_error(&error),
        error,
    }
}

fn io_failure(error: io::Error) -> Failure {
    let reason = match error.kind() {
        io::ErrorKind::BrokenPipe => "broken_pipe",
        io::ErrorKind::PermissionDenied => "denied",
        _ => "other",
    };
    Error::new(codes::PATH_IO)
        .with_detail("reason", reason)
        .into()
}

/// Opens the package at `path` (for editing when it is to be saved) as a document.
fn open(
    product: &Product,
    path: &Path,
    edit: bool,
    limits: &Limits,
) -> Result<(Package, Document), Failure> {
    let mode = if edit {
        Mode::Edit(&product.owner)
    } else {
        Mode::ReadOnly
    };
    let package = Package::open(path, mode, limits)?;
    let options = LoadOptions {
        limits: *limits,
        ..LoadOptions::default()
    };
    let document = package.open_document(
        Arc::clone(&product.types),
        Arc::clone(&product.evaluators),
        &options,
        DocumentConfig::default(),
    )?;
    Ok((package, document))
}

/// Saves every document into its package, in the order they were named.
fn save(packages: &mut [Package], documents: &BTreeMap<String, Document>) -> Result<(), Failure> {
    for package in packages {
        let document = documents.iter().find_map(|(key, document)| {
            let stem = package.path().file_stem().and_then(|s| s.to_str());
            (stem == Some(key.as_str())).then_some(document)
        });
        if let Some(document) = document {
            package.save_document(document, &[], None, &mut |_| Ok(()))?;
        }
    }
    Ok(())
}

/// The output and exit code of `inspect` (with `findings` when `check` is set) and `validate`.
fn check(sub: Sub, check: bool, path: &Path, limits: &Limits) -> Result<(Value, Exit), Failure> {
    let mut output = if sub == Sub::Inspect {
        package::inspect(path, limits)?
    } else {
        json!({})
    };
    let mut exit = Exit::Success;
    if check || sub == Sub::Validate {
        let findings = findings(path, limits)?;
        if !findings.is_empty() {
            exit = Exit::Invalid;
        }
        if let Some(map) = output.as_object_mut() {
            map.insert("findings".into(), Value::Array(findings));
        }
    }
    Ok((output, exit))
}

fn findings(path: &Path, limits: &Limits) -> Result<Vec<Value>, Failure> {
    Ok(package::check(path, limits)?
        .iter()
        .map(|finding| errors::error_data(finding, Map::new()))
        .collect())
}

/// The parameters of `--params`: JSON text, or `@<file>` for the JSON text of a file, read
/// with at most `input_bytes` bytes; none is `{}`.
fn read_params(params: Option<&OsString>, limits: &Limits) -> Result<Value, Failure> {
    let Some(params) = params else {
        return Ok(json!({}));
    };
    let text = params.to_str().ok_or_else(|| usage("not_utf8:--params"))?;
    let Some(path) = text.strip_prefix('@') else {
        return Ok(parse_params(text, limits)?);
    };
    let file = std::fs::File::open(path).map_err(|e| {
        if e.kind() == io::ErrorKind::NotFound {
            Error::new(codes::PATH_NOT_FOUND).into()
        } else {
            io_failure(e)
        }
    })?;
    let mut bytes = Vec::new();
    file.take(limits.input_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(io_failure)?;
    let text = String::from_utf8(bytes).map_err(|_| Error::new(codes::PARAMS_MALFORMED))?;
    Ok(parse_params(&text, limits)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    fn reason(result: Result<Options, Failure>) -> String {
        match result {
            Ok(o) => panic!("parsed: {o:?}"),
            Err(f) => {
                assert_eq!(f.exit, Exit::Usage);
                f.error["data"]["details"]["reason"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            }
        }
    }

    #[test]
    fn subcommands_and_flags_parse() {
        let o = parse(args(&[
            "run",
            "a.arc",
            "--command=x.y.z",
            "--params",
            "{}",
            "--seed",
            "7",
            "--save",
            "--allow-edit",
            "--read-root",
            "/r",
            "--json",
        ]))
        .unwrap();
        assert_eq!(o.sub, Sub::Run);
        assert_eq!(o.positional, args(&["a.arc"]));
        assert_eq!(o.command.as_deref(), Some("x.y.z"));
        assert_eq!(o.seed, Some(7));
        assert!(o.save && o.allow.contains(&Permission::Edit));
        assert_eq!(o.read_roots, vec![PathBuf::from("/r")]);
        let o = parse(args(&["serve", "--stdio", "--", "--a.arc", "b.arc"])).unwrap();
        assert_eq!(o.positional, args(&["--a.arc", "b.arc"]));
    }

    #[test]
    fn malformed_arguments_are_usage_errors() {
        assert_eq!(reason(parse(args(&[]))), "no_subcommand");
        assert_eq!(reason(parse(args(&["frobnicate"]))), "unknown_subcommand");
        assert_eq!(reason(parse(args(&["commands", "x"]))), "arguments");
        assert_eq!(reason(parse(args(&["run", "a"]))), "missing_flag:--command");
        assert_eq!(reason(parse(args(&["serve"]))), "missing_flag:--stdio");
        assert_eq!(
            reason(parse(args(&["commands", "--save"]))),
            "flag_not_allowed:--save"
        );
        assert_eq!(
            reason(parse(args(&["run", "a", "--command"]))),
            "missing_value:--command"
        );
        assert_eq!(
            reason(parse(args(&[
                "run",
                "a",
                "--command",
                "x",
                "--command",
                "y"
            ]))),
            "repeated:--command"
        );
        assert_eq!(
            reason(parse(args(&["run", "a", "--command", "x", "--seed", "+1"]))),
            "seed"
        );
        assert_eq!(
            reason(parse(args(&["commands", "--json=1"]))),
            "unexpected_value:--json"
        );
        assert_eq!(
            reason(parse(args(&["commands", "--nope"]))),
            "unknown_flag:--nope"
        );
    }

    #[test]
    fn errors_map_to_exit_codes() {
        let exit = |code| Failure::from(Error::new(code)).exit;
        assert_eq!(exit(codes::CLI_USAGE), Exit::Usage);
        assert_eq!(exit(codes::PARAMS_MALFORMED), Exit::Invalid);
        assert_eq!(exit(codes::PREVIEW_UNSUPPORTED), Exit::Unsupported);
        assert_eq!(exit(codes::PATH_OUTSIDE_ROOTS), Exit::Denied);
        assert_eq!(exit(codes::PARAMS_LIMIT_EXCEEDED), Exit::Limit);
        assert_eq!(exit(codes::FAULT), Exit::Internal);
        assert_eq!(exit(codes::UNKNOWN), Exit::Failed);
        let missing = Error::new(codes::PARAMS_INVALID)
            .with_detail("violations", r#"[{"pointer":"/a","kind":"required"}]"#);
        assert_eq!(Failure::from(missing).exit, Exit::Usage);
        let wrong = Error::new(codes::PARAMS_INVALID)
            .with_detail("violations", r#"[{"pointer":"/a","kind":"type"}]"#);
        assert_eq!(Failure::from(wrong).exit, Exit::Invalid);
        let codes: Vec<u8> = Exit::ALL.iter().map(|e| e.code()).collect();
        assert_eq!(codes, [0, 1, 2, 3, 4, 5, 6, 70]);
    }
}
