//! The escape-test suite (CORE-CMD-033): every way out of the granted roots this platform
//! allows creating is tried against `Roots::open_read` and `Roots::create_write`, the file
//! surfaces of every automation session. A canary file outside the roots must never be read,
//! written, or joined by a new file.

#![allow(clippy::unwrap_used, clippy::panic, reason = "tests")]

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use arc_command::Roots;
use arc_command::roots::MAX_PATH_LEN;

const CANARY: &str = "CANARY";

/// A temporary tree: `base/root` is the granted root, `base/outside` holds the canary.
struct Tree {
    base: PathBuf,
    root: PathBuf,
    outside: PathBuf,
}

impl Tree {
    fn new(name: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let base = std::env::temp_dir().join(format!(
            "arc-escape-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        let (root, outside) = (base.join("root"), base.join("outside"));
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::create_dir_all(base.join("root2")).unwrap();
        fs::write(root.join("inside.txt"), "inside").unwrap();
        fs::write(outside.join("canary.txt"), CANARY).unwrap();
        fs::write(base.join("root2").join("canary.txt"), CANARY).unwrap();
        // Canonical paths, so `..` and links inside the base resolve as the system sees them.
        let base = plain(fs::canonicalize(&base).unwrap());
        Self {
            root: base.join("root"),
            outside: base.join("outside"),
            base,
        }
    }

    fn roots(&self) -> Roots {
        Roots::new([&self.root], [&self.root]).unwrap()
    }

    /// Reads and writes `path`; any file it gets must not be the canary.
    fn probe(&self, roots: &Roots, path: &Path) {
        if let Ok(mut file) = roots.open_read(path) {
            let mut text = String::new();
            let _ = file.read_to_string(&mut text);
            assert_ne!(text, CANARY, "read the canary through {}", path.display());
        }
        if let Ok(mut file) = roots.create_write(path) {
            file.write_all(b"written").unwrap();
        }
    }

    /// The canary is intact and nothing new appeared outside the roots.
    fn assert_no_escape(&self) {
        assert_eq!(
            fs::read_to_string(self.outside.join("canary.txt")).unwrap(),
            CANARY
        );
        let entries: Vec<_> = fs::read_dir(&self.outside)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(entries.len(), 1, "files appeared outside: {entries:?}");
        assert_eq!(
            fs::read_to_string(self.base.join("root2").join("canary.txt")).unwrap(),
            CANARY
        );
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

/// `path` without the Windows verbatim prefix, so `..` in joined paths is resolved.
fn plain(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    if let Some(rest) = path.to_str().and_then(|s| s.strip_prefix(r"\\?\"))
        && !rest.starts_with("UNC")
    {
        return PathBuf::from(rest);
    }
    path
}

#[allow(clippy::print_stderr, reason = "a skipped case says why")]
fn skip(case: &str, why: &str) {
    eprintln!("escape suite: skipped {case}: {why}");
}

/// Creates a symbolic link at `at` to `target`; false when the platform refuses.
fn link(target: &Path, at: &Path) -> bool {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, at).is_ok()
    }
    #[cfg(windows)]
    {
        if target.is_dir() {
            std::os::windows::fs::symlink_dir(target, at).is_ok()
        } else {
            std::os::windows::fs::symlink_file(target, at).is_ok()
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (target, at);
        false
    }
}

fn code(result: std::io::Result<()>, roots_result: arc_base::Result<fs::File>) -> String {
    result.unwrap();
    roots_result.map(drop).unwrap_err().code().to_owned()
}

#[test]
fn paths_inside_work() {
    let t = Tree::new("inside");
    let roots = t.roots();
    let mut text = String::new();
    roots
        .open_read(&t.root.join("sub").join("..").join("inside.txt"))
        .unwrap()
        .read_to_string(&mut text)
        .unwrap();
    assert_eq!(text, "inside");
    roots
        .create_write(&t.root.join("sub").join("new.txt"))
        .unwrap()
        .write_all(b"n")
        .unwrap();
    roots.create_write(&t.root.join("inside.txt")).unwrap();
    assert_eq!(
        fs::read_to_string(t.root.join("inside.txt")).unwrap(),
        "",
        "truncated"
    );
    assert_eq!(
        fs::read_to_string(t.root.join("sub").join("new.txt")).unwrap(),
        "n"
    );
    t.assert_no_escape();
}

#[test]
fn dot_dot_chains_and_prefix_siblings_stay_out() {
    let t = Tree::new("dotdot");
    let roots = t.roots();
    let out = |p: PathBuf| {
        let read = roots.open_read(&p).map(drop).unwrap_err();
        assert_eq!(read.code(), "cmd.path.outside_roots", "{}", p.display());
        let write = roots.create_write(&p).map(drop).unwrap_err();
        assert_eq!(write.code(), "cmd.path.outside_roots", "{}", p.display());
    };
    out(t.root.join("..").join("outside").join("canary.txt"));
    out(t
        .root
        .join("sub")
        .join("..")
        .join("..")
        .join("outside")
        .join("canary.txt"));
    let mut deep = t.root.clone();
    for _ in 0..40 {
        deep.push("sub");
        deep.push("..");
    }
    out(deep.join("..").join("outside").join("canary.txt"));
    out(t.base.join("root2").join("canary.txt"));
    out(t.root.join("..").join("outside").join("new.txt"));
    assert_eq!(
        roots.create_write(&t.root).map(drop).unwrap_err().code(),
        "cmd.path.outside_roots"
    );
    t.assert_no_escape();
}

#[test]
fn malformed_and_long_paths_are_rejected() {
    let t = Tree::new("text");
    let roots = t.roots();
    let rejected = |p: &Path| roots.open_read(p).map(drop).unwrap_err().code().to_owned();
    assert_eq!(rejected(Path::new("root/inside.txt")), "cmd.path.rejected");
    assert_eq!(rejected(&t.root.join("in\0side.txt")), "cmd.path.rejected");
    assert_eq!(rejected(&t.root.join("sub")), "cmd.path.rejected");
    let long = t.root.join("a/".repeat(MAX_PATH_LEN / 2 + 1));
    assert_eq!(rejected(&long), "cmd.path.too_long");
    assert_eq!(
        roots
            .create_write(&long.join("f"))
            .map(drop)
            .unwrap_err()
            .code(),
        "cmd.path.too_long"
    );
    // A single component longer than the system allows.
    let name = t.root.join("n".repeat(300));
    assert!(matches!(
        rejected(&name).as_str(),
        "cmd.path.too_long" | "cmd.path.not_found"
    ));
    assert_eq!(
        roots
            .create_write(&t.root.join("sub").join(".."))
            .map(drop)
            .unwrap_err()
            .code(),
        "cmd.path.rejected"
    );
    #[cfg(windows)]
    {
        for bad in ["inside.txt:stream", "CON", "nul.txt", "COM1.log"] {
            assert_eq!(rejected(&t.root.join(bad)), "cmd.path.rejected", "{bad}");
        }
        assert_eq!(
            rejected(Path::new(r"\\.\PhysicalDrive0")),
            "cmd.path.rejected"
        );
        assert_eq!(
            rejected(Path::new(r"\\server\share\x")),
            "cmd.path.rejected"
        );
    }
    t.assert_no_escape();
}

#[test]
fn symbolic_links_out_are_refused_and_links_in_work() {
    let t = Tree::new("links");
    let roots = t.roots();
    let canary = t.outside.join("canary.txt");
    let cases = [
        (canary.clone(), "abs"),
        (
            PathBuf::from("..").join("outside").join("canary.txt"),
            "rel",
        ),
        (t.outside.clone(), "dir"),
        (t.outside.join("dangling.txt"), "dangling"),
    ];
    for (target, name) in &cases {
        if !link(target, &t.root.join(name)) {
            skip(
                "symbolic links",
                "the system refuses to create them without privileges",
            );
            return;
        }
    }
    assert!(link(&t.root.join("loop_b"), &t.root.join("loop_a")));
    assert!(link(&t.root.join("loop_a"), &t.root.join("loop_b")));
    assert!(link(&t.root.join("inside.txt"), &t.root.join("in_link")));
    for name in ["abs", "rel"] {
        let p = t.root.join(name);
        assert_eq!(code(Ok(()), roots.open_read(&p)), "cmd.path.outside_roots");
        assert_eq!(
            code(Ok(()), roots.create_write(&p)),
            "cmd.path.outside_roots"
        );
    }
    assert_eq!(
        code(
            Ok(()),
            roots.open_read(&t.root.join("dir").join("canary.txt"))
        ),
        "cmd.path.outside_roots"
    );
    assert_eq!(
        code(
            Ok(()),
            roots.create_write(&t.root.join("dir").join("new.txt"))
        ),
        "cmd.path.outside_roots"
    );
    assert!(roots.create_write(&t.root.join("dangling")).is_err());
    assert!(roots.open_read(&t.root.join("loop_a")).is_err());
    assert!(roots.create_write(&t.root.join("loop_a")).is_err());
    let mut text = String::new();
    roots
        .open_read(&t.root.join("in_link"))
        .unwrap()
        .read_to_string(&mut text)
        .unwrap();
    assert_eq!(text, "inside");
    roots
        .create_write(&t.root.join("in_link"))
        .unwrap()
        .write_all(b"via link")
        .unwrap();
    assert_eq!(
        fs::read_to_string(t.root.join("inside.txt")).unwrap(),
        "via link"
    );
    // A root reached through a link is canonical; a link to the root from outside is a way in.
    assert!(link(&t.root, &t.base.join("alias")));
    roots
        .open_read(&t.base.join("alias").join("inside.txt"))
        .unwrap();
    t.assert_no_escape();
}

#[test]
fn normalization_and_case_variants_resolve_to_what_the_system_opens() {
    let t = Tree::new("unicode");
    let nfc = "caf\u{e9}";
    let nfd = "cafe\u{301}";
    fs::create_dir_all(t.root.join(nfc)).unwrap();
    fs::write(t.root.join(nfc).join("menu.txt"), "inside").unwrap();
    // The decomposed name outside holds a canary; on systems that treat both forms as one name
    // it is the same directory as the root's, which is fine as long as the canary stays unread.
    let outside_variant = t.base.join(nfd);
    if fs::create_dir_all(&outside_variant).is_ok() {
        let _ = fs::write(outside_variant.join("menu.txt"), CANARY);
    }
    let roots = t.roots();
    for name in [nfc, nfd, "CAF\u{c9}", "CAFE\u{301}"] {
        t.probe(&roots, &t.root.join("..").join(name).join("menu.txt"));
        t.probe(&roots, &t.root.join(name).join("menu.txt"));
    }
    for variant in ["ROOT", "Root", "root\u{200b}"] {
        t.probe(&roots, &t.base.join(variant).join("inside.txt"));
        t.probe(
            &roots,
            &t.base
                .join(variant)
                .join("..")
                .join("outside")
                .join("canary.txt"),
        );
    }
    if let Ok(text) = fs::read_to_string(outside_variant.join("menu.txt")) {
        assert_eq!(text, CANARY, "the outside variant was written");
    }
    t.assert_no_escape();
}

#[cfg(windows)]
#[test]
fn junctions_out_are_refused() {
    let t = Tree::new("junction");
    let at = t.root.join("junction");
    let made = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&at)
        .arg(&t.outside)
        .output()
        .is_ok_and(|o| o.status.success());
    if !made {
        skip("junctions", "mklink /J failed");
        return;
    }
    let roots = t.roots();
    t.probe(&roots, &at.join("canary.txt"));
    t.probe(&roots, &at.join("new.txt"));
    assert!(roots.open_read(&at.join("canary.txt")).is_err());
    t.assert_no_escape();
}

#[cfg(unix)]
/// Swaps `at` between a regular file inside the root and a link to `target` while `probe` runs.
fn race(
    t: &Tree,
    at: &Path,
    target: &Path,
    make_inside: impl Fn(&Path) + Sync,
    probe: impl Fn() + Sync,
) {
    let stop = std::sync::atomic::AtomicBool::new(false);
    let staged_link = t.root.join("staged_link");
    let staged = t.root.join("staged");
    std::thread::scope(|scope| {
        scope.spawn(|| {
            // A link cannot replace a directory in one rename, so a directory goes first.
            let clear = || {
                if at.symlink_metadata().is_ok_and(|m| m.is_dir()) {
                    let _ = fs::remove_dir_all(at);
                }
            };
            for _ in 0..500 {
                let _ = fs::remove_file(&staged_link);
                if link(target, &staged_link) {
                    clear();
                    let _ = fs::rename(&staged_link, at);
                }
                make_inside(&staged);
                clear();
                let _ = fs::rename(&staged, at);
            }
            stop.store(true, Ordering::SeqCst);
        });
        while !stop.load(Ordering::SeqCst) {
            probe();
        }
    });
}

#[cfg(unix)]
#[test]
fn a_file_swapped_for_a_link_never_exposes_the_canary() {
    let t = Tree::new("race-file");
    let roots = t.roots();
    let at = t.root.join("swap");
    fs::write(&at, "inside").unwrap();
    let canary = t.outside.join("canary.txt");
    race(
        &t,
        &at,
        &canary,
        |staged| {
            let _ = fs::remove_dir_all(staged);
            let _ = fs::write(staged, "inside");
        },
        || t.probe(&roots, &at),
    );
    t.assert_no_escape();
}

#[cfg(any(target_os = "linux", target_os = "android"))]
#[test]
fn a_directory_swapped_for_a_link_never_receives_new_files() {
    let t = Tree::new("race-dir");
    let roots = t.roots();
    let at = t.root.join("d");
    fs::create_dir(&at).unwrap();
    let n = AtomicUsize::new(0);
    race(
        &t,
        &at,
        &t.outside,
        |staged| {
            let _ = fs::remove_file(staged);
            let _ = fs::create_dir(staged);
        },
        || {
            let i = n.fetch_add(1, Ordering::SeqCst);
            t.probe(&roots, &at.join(format!("new{i}.txt")));
            t.probe(&roots, &at.join("canary.txt"));
        },
    );
    t.assert_no_escape();
}
