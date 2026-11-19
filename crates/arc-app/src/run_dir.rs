//! The run directory: the private folder that holds the single-instance channel and the
//! automation socket. [`choose`] picks it from candidates the caller has read; it reads no
//! environment variable itself.
//!
//! On Unix the candidates are tried in this order:
//!
//! 1. `<runtime>/<app>`, when the runtime directory is set and absolute and the folder is owned by
//!    the current user with mode 0700. It is created with that mode when missing. A folder that
//!    fails the check is not used, and the next candidate is tried.
//! 2. The data directory, when every socket path below it fits [`SOCKET_PATH_LIMIT`].
//! 3. `<temporary>/<app>-<uid>`, created with mode 0700 when missing. An existing folder must be a
//!    directory that is not a symbolic link, is owned by the current user and has mode 0700;
//!    otherwise the call fails and nothing is used.
//!
//! The chosen folder is the parent of the socket folders (`instance`, `automation`). Those are
//! created by their own users with the private-directory checks of the socket code, so those checks
//! apply to whatever is chosen. An environment variable never bypasses them. Windows has no socket
//! path limit here and uses the data directory.

use std::path::{Path, PathBuf};

#[cfg(unix)]
use arc_base::Error;
use arc_base::Result;
#[cfg(unix)]
use arc_command::protocol::SOCKET_PATH_LIMIT;

#[cfg(unix)]
use crate::codes;

/// The candidates [`choose`] picks from. The caller reads the environment and creates the data
/// directory before the call.
#[derive(Debug, Clone, Copy)]
pub struct RunInputs<'a> {
    /// The application's name: the folder name under the runtime and temporary directories.
    pub app: &'a str,
    /// The runtime directory (`XDG_RUNTIME_DIR`), when it is set and not empty.
    pub runtime_dir: Option<&'a Path>,
    /// The per-user data directory.
    pub data_dir: &'a Path,
    /// The system temporary directory.
    pub temp_dir: &'a Path,
    /// Socket paths relative to the run directory. Each one must fit the platform limit.
    pub sockets: &'a [&'a str],
}

/// Refuses a socket path longer than [`SOCKET_PATH_LIMIT`] bytes. The path is not reported: it
/// names the user's folders, and the lengths say what is wrong.
#[cfg(unix)]
pub(crate) fn check_socket_path(path: &Path) -> Result<()> {
    let length = path.as_os_str().len();
    if length <= SOCKET_PATH_LIMIT {
        return Ok(());
    }
    Err(Error::new(codes::INSTANCE_PATH_TOO_LONG)
        .with_arg("length", i64::try_from(length).unwrap_or(i64::MAX))
        .with_arg(
            "limit",
            i64::try_from(SOCKET_PATH_LIMIT).unwrap_or(i64::MAX),
        ))
}

/// Picks the run directory; the order is in the module documentation.
///
/// # Errors
/// `app.instance.path_too_long` when the temporary folder would not hold the socket paths;
/// `app.instance.directory` (detail `reason`: `not_directory`, `owner`, `not_private`) when the
/// temporary folder exists but is not a private folder of the user; `app.instance.io` when the
/// temporary folder cannot be created or the user cannot be identified.
#[cfg(unix)]
pub fn choose(inputs: &RunInputs<'_>) -> Result<PathBuf> {
    if let Some(runtime) = inputs.runtime_dir.filter(|dir| dir.is_absolute()) {
        let dir = runtime.join(inputs.app);
        if fits(&dir, inputs.sockets)
            && own_uid(runtime, inputs.app).is_ok_and(|uid| private_folder(&dir, uid, true).is_ok())
        {
            return Ok(dir);
        }
    }
    if fits(inputs.data_dir, inputs.sockets) {
        return Ok(inputs.data_dir.to_path_buf());
    }
    let uid = own_uid(inputs.temp_dir, inputs.app)
        .map_err(|e| Error::new(codes::INSTANCE_IO).with_source(e))?;
    let dir = inputs.temp_dir.join(format!("{}-{uid}", inputs.app));
    if let Some(error) = socket_too_long(&dir, inputs.sockets) {
        return Err(error);
    }
    private_folder(&dir, uid, true).map_err(|reason| {
        let code = if reason == "create" {
            codes::INSTANCE_IO
        } else {
            codes::INSTANCE_DIRECTORY
        };
        Error::new(code).with_detail("reason", reason)
    })?;
    Ok(dir)
}

/// The start of the run directory on platforms without the socket limit: the data directory.
#[cfg(not(unix))]
pub fn choose(inputs: &RunInputs<'_>) -> Result<PathBuf> {
    Ok(inputs.data_dir.to_path_buf())
}

/// The error for the first socket path below `dir` that is too long, if any.
#[cfg(unix)]
fn socket_too_long(dir: &Path, sockets: &[&str]) -> Option<Error> {
    sockets
        .iter()
        .find_map(|socket| check_socket_path(&dir.join(socket)).err())
}

#[cfg(unix)]
fn fits(dir: &Path, sockets: &[&str]) -> bool {
    socket_too_long(dir, sockets).is_none()
}

/// The mode of every run folder: only the user may enter it.
#[cfg(unix)]
const RUN_MODE: u32 = 0o700;

/// Makes sure `dir` is a folder of the user `uid` with mode 0700 that is not a symbolic link,
/// creating it with that mode when `create` is set and it is missing. The reason names the first
/// check that fails.
#[cfg(unix)]
fn private_folder(dir: &Path, uid: u32, create: bool) -> std::result::Result<(), &'static str> {
    use std::fs::{self, DirBuilder};
    use std::io;
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};

    let missing = fs::symlink_metadata(dir).is_err_and(|e| e.kind() == io::ErrorKind::NotFound);
    if create && missing {
        DirBuilder::new()
            .mode(RUN_MODE)
            .create(dir)
            .map_err(|_| "create")?;
    }
    let meta = fs::symlink_metadata(dir).map_err(|_| "create")?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err("not_directory");
    }
    if meta.uid() != uid {
        return Err("owner");
    }
    if meta.permissions().mode() & 0o777 != RUN_MODE {
        return Err("not_private");
    }
    Ok(())
}

/// The numeric id of the user running the process: the owner of a file this call creates in
/// `parent` and removes again. Used instead of a libc call.
#[cfg(unix)]
fn own_uid(parent: &Path, app: &str) -> std::io::Result<u32> {
    use std::fs::{self, OpenOptions};
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

    let probe = parent.join(format!(".{app}-{}.probe", std::process::id()));
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&probe)?;
    let uid = file.metadata()?.uid();
    drop(file);
    fs::remove_file(&probe)?;
    Ok(uid)
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use std::fs::{self, DirBuilder};
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::testutil::padded;

    /// The socket paths of the application, relative to the run directory.
    const SOCKETS: [&str; 2] = ["instance/instance.sock", "automation/automation.sock"];

    /// A fresh folder with a short name, so that the socket paths below it fit on every platform.
    fn base(tag: char) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rd{}{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)
            .unwrap();
        dir
    }

    /// Creates `path` (and its parents) with `mode`.
    fn folder(path: &Path, mode: u32) -> PathBuf {
        DirBuilder::new()
            .recursive(true)
            .mode(mode)
            .create(path)
            .unwrap();
        path.to_path_buf()
    }

    fn inputs<'a>(runtime: Option<&'a Path>, data: &'a Path, temp: &'a Path) -> RunInputs<'a> {
        RunInputs {
            app: "ap",
            runtime_dir: runtime,
            data_dir: data,
            temp_dir: temp,
            sockets: &SOCKETS,
        }
    }

    #[test]
    fn a_valid_runtime_folder_is_chosen_and_created_private() {
        let root = base('a');
        let runtime = folder(&root.join("rt"), 0o700);
        let data = folder(&root.join("d"), 0o755);
        let temp = folder(&root.join("t"), 0o700);
        let chosen = choose(&inputs(Some(&runtime), &data, &temp)).unwrap();
        assert_eq!(chosen, runtime.join("ap"));
        assert_eq!(
            fs::metadata(&chosen).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }

    #[test]
    fn a_runtime_folder_with_the_wrong_mode_or_a_relative_path_is_not_used() {
        let root = base('b');
        let runtime = folder(&root.join("rt"), 0o700);
        folder(&runtime.join("ap"), 0o755);
        let data = folder(&root.join("d"), 0o755);
        let temp = folder(&root.join("t"), 0o700);
        let chosen = choose(&inputs(Some(&runtime), &data, &temp)).unwrap();
        assert_eq!(chosen, data);
        let relative = choose(&inputs(Some(Path::new("rel")), &data, &temp)).unwrap();
        assert_eq!(relative, data);
    }

    #[test]
    fn a_data_folder_that_fits_is_chosen_without_a_runtime_folder() {
        let root = base('c');
        let data = folder(&root.join("d"), 0o755);
        let temp = folder(&root.join("t"), 0o700);
        let chosen = choose(&inputs(None, &data, &temp)).unwrap();
        assert_eq!(chosen, data);
    }

    #[test]
    fn a_data_folder_too_long_for_the_sockets_falls_to_the_temporary_folder() {
        let root = base('e');
        // The automation socket path is 27 bytes longer than this folder.
        let data = folder(&padded(&root, 80).unwrap(), 0o700);
        let temp = folder(&root.join("t"), 0o700);
        let uid = fs::metadata(&temp).unwrap().uid();
        let chosen = choose(&inputs(None, &data, &temp)).unwrap();
        assert_eq!(chosen, temp.join(format!("ap-{uid}")));
        assert_eq!(
            fs::metadata(&chosen).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }

    #[test]
    fn a_temporary_folder_of_the_wrong_mode_is_refused() {
        let root = base('f');
        let data = folder(&padded(&root, 80).unwrap(), 0o700);
        let temp = folder(&root.join("t"), 0o700);
        let uid = fs::metadata(&temp).unwrap().uid();
        folder(&temp.join(format!("ap-{uid}")), 0o755);
        let error = choose(&inputs(None, &data, &temp)).unwrap_err();
        assert_eq!(error.code(), "app.instance.directory");
        assert_eq!(
            error.details().get("reason"),
            Some(&arc_base::error::Value::Text("not_private".into()))
        );
    }

    #[test]
    fn no_folder_that_holds_the_sockets_is_reported_as_too_long() {
        let root = base('g');
        let data = folder(&padded(&root, 80).unwrap(), 0o700);
        let temp = folder(&padded(&root.join("q"), 81).unwrap(), 0o700);
        let error = choose(&inputs(None, &data, &temp)).unwrap_err();
        assert_eq!(error.code(), "app.instance.path_too_long");
        let limit = i64::try_from(SOCKET_PATH_LIMIT).unwrap_or(i64::MAX);
        assert_eq!(
            error.arguments().get("limit"),
            Some(&arc_base::error::Value::Int(limit))
        );
    }
}
