//! Working-directory ↔ object-version conversion.
//!
//! An OCFL version's state is a flat map of logical paths to digests; a working
//! directory is a tree. This module is the translation in both directions, and
//! it is the only place in the CLI that knows what a directory looks like.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Names skipped by default when reading a directory into a version.
///
/// A version records what a person means to keep. Version-control metadata and
/// filesystem debris are neither, and capturing them would make every commit
/// noisy and every restore hazardous — `.git` in particular must never be
/// written back out by a restore. Override with `--include-all`.
pub const DEFAULT_EXCLUDES: [&str; 4] = [".git", ".DS_Store", ".svn", "Thumbs.db"];

/// One file found in a working directory.
pub struct Entry {
    /// `/`-separated path relative to the scanned root — the logical path.
    pub logical: String,
    /// Where it actually is, for reading.
    pub source: PathBuf,
}

/// Collect every file under `root`, depth-first, sorted by logical path.
///
/// Sorted output makes a commit's reported changes stable between runs, which
/// matters more than it sounds: an unsorted walk turns every diff of a report
/// into noise.
pub fn walk(root: &Path, excludes: &[String]) -> std::io::Result<Vec<Entry>> {
    let mut out = Vec::new();
    collect(root, root, excludes, &mut out)?;
    out.sort_by(|a, b| a.logical.cmp(&b.logical));
    Ok(out)
}

fn collect(
    root: &Path,
    dir: &Path,
    excludes: &[String],
    out: &mut Vec<Entry>,
) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if excludes.contains(&name) {
            continue;
        }
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect(root, &path, excludes, out)?;
        } else if file_type.is_file() {
            // Symlinks are deliberately skipped rather than followed: OCFL
            // stores bytes, and a followed link either duplicates content that
            // is already captured or reaches outside the tree entirely.
            let Ok(relative) = path.strip_prefix(root) else {
                continue;
            };
            let logical = relative
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            out.push(Entry {
                logical,
                source: path,
            });
        }
    }
    Ok(())
}

/// What changed between two logical states.
///
/// Renames are detected the way OCFL itself sees them — a digest that left one
/// path and arrived at another — rather than by guessing at content similarity.
/// That is exact, not heuristic, which is the whole reason the layout keys on
/// digests.
#[derive(Debug, Default)]
pub struct Changes {
    /// Paths present only in the newer state.
    pub added: Vec<String>,
    /// Paths present in both, with different bytes.
    pub modified: Vec<String>,
    /// Paths present only in the older state.
    pub removed: Vec<String>,
    /// `(from, to)` pairs holding identical bytes — a move, not a rewrite.
    pub renamed: Vec<(String, String)>,
}

impl Changes {
    /// Whether anything at all differs.
    pub fn is_empty(&self) -> bool {
        self.added.is_empty()
            && self.modified.is_empty()
            && self.removed.is_empty()
            && self.renamed.is_empty()
    }

    /// The count of changed paths, for a one-line summary.
    pub fn total(&self) -> usize {
        self.added.len() + self.modified.len() + self.removed.len() + self.renamed.len()
    }
}

/// Compare two `logical path -> digest` maps.
pub fn diff(from: &BTreeMap<String, String>, to: &BTreeMap<String, String>) -> Changes {
    let mut changes = Changes::default();

    let mut appeared: Vec<&String> = to.keys().filter(|p| !from.contains_key(*p)).collect();
    let mut vanished: Vec<&String> = from.keys().filter(|p| !to.contains_key(*p)).collect();

    // Pair a vanished path with an appeared one holding the same bytes. Both
    // lists are sorted, so pairing is deterministic when several files share a
    // digest.
    let mut paired_from: Vec<&String> = Vec::new();
    let mut paired_to: Vec<&String> = Vec::new();
    for old in &vanished {
        let digest = &from[*old];
        if let Some(new) = appeared
            .iter()
            .find(|p| &to[**p] == digest && !paired_to.contains(p))
        {
            changes.renamed.push(((*old).clone(), (*new).clone()));
            paired_from.push(old);
            paired_to.push(new);
        }
    }
    appeared.retain(|p| !paired_to.contains(p));
    vanished.retain(|p| !paired_from.contains(p));

    changes.added = appeared.into_iter().cloned().collect();
    changes.removed = vanished.into_iter().cloned().collect();
    for (path, digest) in to {
        if let Some(before) = from.get(path)
            && before != digest
        {
            changes.modified.push(path.clone());
        }
    }
    changes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(p, d)| ((*p).to_string(), (*d).to_string()))
            .collect()
    }

    #[test]
    fn a_move_reads_as_a_rename_not_an_add_and_a_delete() {
        // The property OCFL's digest keying buys: exact rename detection, with
        // no similarity threshold to tune and no false pairing.
        let changes = diff(&state(&[("old.md", "aaa")]), &state(&[("new.md", "aaa")]));
        assert_eq!(
            changes.renamed,
            [("old.md".to_string(), "new.md".to_string())]
        );
        assert!(changes.added.is_empty() && changes.removed.is_empty());
    }

    #[test]
    fn adds_modifications_and_deletions_are_told_apart() {
        let changes = diff(
            &state(&[("keep.md", "a"), ("edit.md", "b"), ("drop.md", "c")]),
            &state(&[("keep.md", "a"), ("edit.md", "B"), ("new.md", "d")]),
        );
        assert_eq!(changes.added, ["new.md"]);
        assert_eq!(changes.modified, ["edit.md"]);
        assert_eq!(changes.removed, ["drop.md"]);
        assert!(changes.renamed.is_empty());
    }

    #[test]
    fn an_unchanged_state_reports_nothing() {
        let s = state(&[("a.md", "x")]);
        assert!(diff(&s, &s).is_empty());
    }

    #[test]
    fn a_copy_is_an_add_not_a_rename_because_the_original_stayed() {
        // Only a path that *vanished* can be the source of a rename.
        let changes = diff(
            &state(&[("a.md", "same")]),
            &state(&[("a.md", "same"), ("b.md", "same")]),
        );
        assert_eq!(changes.added, ["b.md"]);
        assert!(changes.renamed.is_empty());
    }

    #[test]
    fn walking_a_tree_yields_sorted_slash_separated_paths() {
        let dir = std::env::temp_dir().join(format!("mocfl-walk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub/deep")).unwrap();
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::write(dir.join("b.md"), b"b").unwrap();
        std::fs::write(dir.join("a.md"), b"a").unwrap();
        std::fs::write(dir.join("sub/deep/c.md"), b"c").unwrap();
        std::fs::write(dir.join(".git/config"), b"nope").unwrap();

        let excludes: Vec<String> = DEFAULT_EXCLUDES.iter().map(|s| s.to_string()).collect();
        let found: Vec<String> = walk(&dir, &excludes)
            .unwrap()
            .into_iter()
            .map(|e| e.logical)
            .collect();

        assert_eq!(found, ["a.md", "b.md", "sub/deep/c.md"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
