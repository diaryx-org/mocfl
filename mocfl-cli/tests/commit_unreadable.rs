//! A `mocfl commit` that cannot read a file leaves the object as it was.
//!
//! A version's state is complete, so a path missing from it is a deletion. A
//! commit that stopped at the file it could not read would record every path
//! from there on as deleted, which is why these drive the real binary and
//! compare the object's bytes rather than ask the library what it thinks.

#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A working directory of `a.txt` to `d.txt` beside where an object will go,
/// removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "mocfl-cli-unreadable-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("work")).unwrap();
        for name in ["a", "b", "c", "d"] {
            fs::write(dir.join("work").join(format!("{name}.txt")), name).unwrap();
        }
        Self(dir)
    }

    fn object(&self) -> PathBuf {
        self.0.join("object")
    }

    /// Run `mocfl commit` from the working directory.
    fn commit(&self, extra: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_mocfl"))
            .arg("commit")
            .arg(self.object())
            .arg("--from")
            .arg(self.0.join("work"))
            .args(["--user", "u", "--at", "2026-11-06T09:00:00Z"])
            .args(extra)
            .output()
            .unwrap()
    }

    /// Make one file unreadable, or say why it cannot be: permissions do not
    /// stop root, and a test that read the file anyway would prove nothing.
    fn lock(&self, name: &str) -> bool {
        let path = self.0.join("work").join(name);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
        let locked = fs::read(&path).is_err();
        if !locked {
            eprintln!("skipped: this user reads a file with no permissions");
        }
        locked
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Every file under `root` and its bytes.
fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(dir: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                visit(&path, out);
            } else {
                let bytes = fs::read(&path).unwrap();
                out.insert(path, bytes);
            }
        }
    }
    let mut out = BTreeMap::new();
    visit(root, &mut out);
    out
}

fn assert_refused(output: &Output, name: &str) {
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "the commit reported success");
    assert!(
        stderr.contains("could not read") && stderr.contains(name),
        "the error does not name {name}: {stderr}"
    );
}

#[test]
fn an_object_with_a_version_keeps_its_head() {
    let scratch = Scratch::new("existing");
    assert!(
        scratch
            .commit(&["--id", "urn:x", "-m", "v1"])
            .status
            .success()
    );
    let before = snapshot(&scratch.object());
    if !scratch.lock("b.txt") {
        return;
    }

    assert_refused(&scratch.commit(&["-m", "v2"]), "b.txt");
    assert!(snapshot(&scratch.object()) == before, "the object changed");

    // Running it again is no different: a failure never adds up to a version.
    assert_refused(&scratch.commit(&["-m", "v2"]), "b.txt");
    assert!(snapshot(&scratch.object()) == before, "the object changed");
}

#[test]
fn no_object_is_made_by_a_first_commit() {
    let scratch = Scratch::new("first");
    if !scratch.lock("b.txt") {
        return;
    }

    assert_refused(&scratch.commit(&["--id", "urn:x", "-m", "v1"]), "b.txt");
    assert!(!scratch.object().exists(), "a partial object was written");
}

#[test]
fn an_unreadable_first_file_empties_nothing() {
    // The file sorts first, so nothing was read before the failure. The walk
    // found four entries, so the `--allow-empty` guard has nothing to say: only
    // the read failing can keep the head's files in it.
    let scratch = Scratch::new("sorts-first");
    assert!(
        scratch
            .commit(&["--id", "urn:x", "-m", "v1"])
            .status
            .success()
    );
    let before = snapshot(&scratch.object());
    if !scratch.lock("a.txt") {
        return;
    }

    assert_refused(&scratch.commit(&["-m", "v2"]), "a.txt");
    assert!(snapshot(&scratch.object()) == before, "the object changed");
}

#[test]
fn the_commit_goes_through_once_the_file_can_be_read() {
    let scratch = Scratch::new("recovers");
    assert!(
        scratch
            .commit(&["--id", "urn:x", "-m", "v1"])
            .status
            .success()
    );
    fs::write(scratch.0.join("work/d.txt"), "changed").unwrap();
    if !scratch.lock("b.txt") {
        return;
    }
    assert_refused(&scratch.commit(&["-m", "v2"]), "b.txt");

    let path = scratch.0.join("work/b.txt");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    let output = scratch.commit(&["-m", "v2"]);
    assert!(output.status.success());
    // The refused attempts took no number: this is v2, and it changes one file.
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("v2\t4 file(s), 1 change(s)"), "{stdout}");
}
