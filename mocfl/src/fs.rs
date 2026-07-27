//! The storage seam.
//!
//! Every byte this crate reads or writes goes through [`Storage`], so an object
//! can live somewhere that is not `std::fs` — a coordinated iCloud container, a
//! sandboxed app directory, a test harness — without the object logic knowing.
//!
//! ## Why this is synchronous
//!
//! The sister crates take an `async` storage trait because they must reach OPFS
//! and IndexedDB in a browser, where there is no blocking read. This crate is
//! deliberately sync, and the reason is the shape of the work rather than
//! taste: OCFL operations are batch operations — commit a version, validate an
//! object, export a copy — not interactive ones. Nothing here interleaves with a
//! UI. A caller that needs to stay responsive runs a whole operation off the
//! main thread, which is one `spawn_blocking` at the boundary instead of an
//! async colouring of every function beneath it.
//!
//! If a browser backend is ever wanted, this is the trait to duplicate — not the
//! reason to make the object model async today.

use std::io;
use std::path::{Path, PathBuf};

/// What this crate needs from a filesystem.
pub trait Storage {
    /// Read a file's entire contents.
    fn read(&self, path: &Path) -> io::Result<Vec<u8>>;

    /// Write a file, replacing it if it exists. Parent directories are the
    /// caller's responsibility — see [`Storage::create_dir_all`].
    fn write(&self, path: &Path, contents: &[u8]) -> io::Result<()>;

    /// Create a directory and every missing parent.
    fn create_dir_all(&self, path: &Path) -> io::Result<()>;

    /// The names of the entries directly inside `path`, in unspecified order,
    /// paired with whether each is a directory.
    fn read_dir(&self, path: &Path) -> io::Result<Vec<(String, bool)>>;

    /// Whether something exists at `path`.
    fn exists(&self, path: &Path) -> io::Result<bool>;

    /// Remove a regular file.
    fn remove_file(&self, path: &Path) -> io::Result<()>;
}

/// [`Storage`] over the real filesystem.
#[derive(Debug, Clone, Copy, Default)]
pub struct StdFs;

impl Storage for StdFs {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        std::fs::read(path)
    }

    fn write(&self, path: &Path, contents: &[u8]) -> io::Result<()> {
        std::fs::write(path, contents)
    }

    fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        std::fs::create_dir_all(path)
    }

    fn read_dir(&self, path: &Path) -> io::Result<Vec<(String, bool)>> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let is_dir = entry.file_type()?.is_dir();
            out.push((name, is_dir));
        }
        Ok(out)
    }

    fn exists(&self, path: &Path) -> io::Result<bool> {
        match std::fs::metadata(path) {
            Ok(_) => Ok(true),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e),
        }
    }

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        std::fs::remove_file(path)
    }
}

/// Join a root with a `/`-separated OCFL path.
///
/// OCFL paths are always `/`-separated regardless of platform, so they are split
/// and rejoined rather than handed to `Path::join` whole — otherwise a
/// `v1/content/a.txt` would become one literal filename on a platform whose
/// separator is `\`.
pub(crate) fn join(root: &Path, ocfl_path: &str) -> PathBuf {
    let mut out = root.to_path_buf();
    for segment in ocfl_path.split('/') {
        out.push(segment);
    }
    out
}
