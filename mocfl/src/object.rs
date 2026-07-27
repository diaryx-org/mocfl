//! The OCFL object — the unit this crate implements.
//!
//! ## Scope: an object, not a repository
//!
//! OCFL describes two things: an **object** (one versioned thing, self-contained
//! in its own directory) and a **storage root** (a repository of many objects,
//! sharded by one of several layout extensions). This crate implements the
//! object and deliberately stops there.
//!
//! That is the whole reason it is small. Storage-root layouts — hashed n-tuple
//! trees and their variants — exist to spread hundreds of thousands of objects
//! across a filesystem without any one directory growing unmanageable. A caller
//! holding one object per collection has no such problem, and paying for the
//! machinery would buy nothing. A repository layer can be built on top of this
//! without changing anything here.
//!
//! ## The commit order, and what it guarantees
//!
//! A version lands in a fixed order, chosen so an interrupted commit leaves an
//! object that is still valid at its *previous* version rather than a corrupt one:
//!
//! 1. content files into `vN/content/`
//! 2. the object declaration, if this is the first version
//! 3. `vN/inventory.json` and its digest sidecar
//! 4. the root `inventory.json` and its sidecar — **the commit point**
//!
//! The root inventory is what a reader consults, so until step 4 lands the new
//! version is invisible and the object still describes vN-1 correctly. A crash
//! before it leaves orphaned content in `vN/`, which wastes space and is
//! reported by [`crate::validate`] — untidy, never wrong. This crate does not
//! attempt atomicity beyond that; a caller wanting all-or-nothing across the
//! whole tree should journal at its own layer.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::digest::{Digest, DigestAlgorithm};
use crate::error::{Error, Result};
use crate::fs::{Storage, join};
use crate::inventory::{Inventory, User, Version, VersionNum, check_path};

/// The OCFL spec version this crate writes.
pub const OCFL_VERSION: &str = "ocfl_object_1.1";

/// The spec versions this crate can *read*.
///
/// Writing targets [`OCFL_VERSION`] only, but 1.0 objects are everywhere and
/// remain valid, so refusing them would be a bug rather than strictness. An
/// object opened at 1.0 keeps its declared version when new versions are
/// committed to it: silently promoting someone's object to 1.1 is a migration,
/// and a migration is the owner's decision, not a side effect of a write.
pub const SUPPORTED_VERSIONS: [&str; 2] = ["ocfl_object_1.0", "ocfl_object_1.1"];

/// The prefix every OCFL object declaration filename carries. The version
/// follows it, so the filename alone states what the directory is.
pub const DECLARATION_PREFIX: &str = "0=";

/// The name of the NAMASTE declaration file this crate writes.
///
/// A dead-simple, filename-encoded statement of what a directory *is*, readable
/// without opening anything. It is the same instinct as a self-describing
/// workspace: hand the directory to a stranger and it says what it is.
pub const DECLARATION: &str = "0=ocfl_object_1.1";

/// The declaration filename for a given spec version.
pub fn declaration_name(spec_version: &str) -> String {
    format!("{DECLARATION_PREFIX}{spec_version}")
}

/// The inventory filename, in the object root and in each version directory.
pub const INVENTORY: &str = "inventory.json";

/// What a caller records about a version it is committing.
///
/// `created` is supplied rather than read from a clock: this crate has no clock,
/// for the same reason the sister crates don't — determinism in tests, and the
/// caller is the only one that knows whether the right instant is "now", the
/// time of an import, or the timestamp of the source document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionMeta {
    /// RFC 3339 timestamp with a timezone, e.g. `2026-07-27T09:14:00Z`.
    pub created: String,
    /// A free-text note about what this version is.
    pub message: Option<String>,
    /// The agent responsible for the version.
    pub user: User,
}

impl VersionMeta {
    /// A version stamped at `created` with no message and no agent.
    pub fn at(created: impl Into<String>) -> Self {
        Self {
            created: created.into(),
            message: None,
            user: User::default(),
        }
    }

    /// Attach a message.
    pub fn message(mut self, message: impl Into<String>) -> Self {
        self.message = Some(message.into());
        self
    }

    /// Attach the responsible agent.
    pub fn user(mut self, name: impl Into<String>, address: Option<String>) -> Self {
        self.user = User {
            name: Some(name.into()),
            address,
        };
        self
    }
}

/// One point in a file's life, as reported by [`Object::history`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryEntry<'a> {
    /// The version at which this state began.
    pub version: VersionNum,
    /// The content at that version.
    pub digest: &'a Digest,
    /// The name the file had *then*, which is not necessarily the name asked
    /// about — that difference is how a rename shows up.
    pub logical_path: &'a str,
}

/// An OCFL object rooted at a directory.
#[derive(Debug, Clone)]
pub struct Object<S: Storage> {
    fs: S,
    root: PathBuf,
    inventory: Inventory,
    /// The spec version this object declares — what its `0=…` file says, which
    /// is not necessarily what this crate writes for a new object.
    spec_version: String,
    /// Whether anything has been written to disk yet. False for an object built
    /// by [`Object::create`] whose first version has not been committed.
    on_disk: bool,
    write_version_inventories: bool,
}

impl<S: Storage> Object<S> {
    /// Prepare a new object. **Nothing is written** until the first
    /// [`commit`](Object::commit).
    ///
    /// The spec requires an object to have at least one version, so there is no
    /// such thing as a valid empty object on disk. Rather than create an invalid
    /// directory and hope the caller finishes, this stays in memory until there
    /// is a real version to write.
    pub fn create(
        fs: S,
        root: impl Into<PathBuf>,
        id: impl Into<String>,
        digest_algorithm: DigestAlgorithm,
    ) -> Self {
        Self {
            fs,
            root: root.into(),
            inventory: Inventory::new(id, digest_algorithm),
            spec_version: OCFL_VERSION.to_string(),
            on_disk: false,
            write_version_inventories: true,
        }
    }

    /// Open the object at `root`, reading its declaration and root inventory.
    ///
    /// The declaration is *discovered* rather than assumed: the spec version
    /// lives in the filename, so the root is scanned for a `0=ocfl_object_…`
    /// entry and whatever version it names is adopted. Looking only for the
    /// version this crate happens to write would reject every conformant 1.0
    /// object in existence.
    pub fn open(fs: S, root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();

        let declared: Vec<String> = fs
            .read_dir(&root)?
            .into_iter()
            .filter(|(name, is_dir)| !*is_dir && name.starts_with("0=ocfl_object_"))
            .map(|(name, _)| name)
            .collect();

        let spec_version = match declared.as_slice() {
            [] => {
                return Err(Error::NotFound(format!(
                    "no OCFL object declaration (0=ocfl_object_…) in {}; is it an object root?",
                    root.display()
                )));
            }
            [one] => one[DECLARATION_PREFIX.len()..].to_string(),
            many => {
                // Two declarations make the directory's own claim about itself
                // ambiguous, which is the one thing a self-describing layout
                // must never be.
                return Err(Error::Conflict(format!(
                    "{} declares {} OCFL versions at once ({})",
                    root.display(),
                    many.len(),
                    many.join(", ")
                )));
            }
        };

        if !SUPPORTED_VERSIONS.contains(&spec_version.as_str()) {
            return Err(Error::Conflict(format!(
                "{} declares {spec_version}, which this crate does not read (supported: {})",
                root.display(),
                SUPPORTED_VERSIONS.join(", ")
            )));
        }

        let inventory = Inventory::parse(&fs.read(&root.join(INVENTORY))?)?;
        Ok(Self {
            fs,
            root,
            inventory,
            spec_version,
            on_disk: true,
            write_version_inventories: true,
        })
    }

    /// The spec version this object declares.
    pub fn spec_version(&self) -> &str {
        &self.spec_version
    }

    /// Whether to also write a copy of the inventory into each version directory.
    ///
    /// The spec makes this a SHOULD, not a MUST, and it is on by default here.
    /// The copies are what let an object be reconstructed after the root
    /// inventory is lost or truncated — real robustness rather than ceremony,
    /// and the kind of redundancy the whole layout exists to provide. Turn it off
    /// only with a measurement in hand: each copy carries every version's state,
    /// so total inventory bytes grow with the square of the version count.
    pub fn with_version_inventories(mut self, enabled: bool) -> Self {
        self.write_version_inventories = enabled;
        self
    }

    /// The object root directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The parsed inventory.
    pub fn inventory(&self) -> &Inventory {
        &self.inventory
    }

    /// The object's identifier.
    pub fn id(&self) -> &str {
        &self.inventory.id
    }

    /// Every version, oldest first.
    pub fn versions(&self) -> Vec<VersionNum> {
        self.inventory.version_numbers()
    }

    /// The most recent version number, or `None` before the first commit.
    pub fn head(&self) -> Option<VersionNum> {
        self.on_disk.then_some(self.inventory.head)
    }

    /// One version's record.
    pub fn version(&self, version: VersionNum) -> Result<&Version> {
        self.inventory
            .versions
            .get(&version)
            .ok_or_else(|| Error::NotFound(format!("version {version}")))
    }

    /// The logical paths present at `version`, with their digests.
    pub fn files_at(&self, version: VersionNum) -> Result<BTreeMap<&str, &Digest>> {
        Ok(self.version(version)?.files())
    }

    /// Read one file's bytes as of `version`.
    pub fn read(&self, version: VersionNum, logical_path: &str) -> Result<Vec<u8>> {
        let digest = self
            .version(version)?
            .files()
            .get(logical_path)
            .copied()
            .ok_or_else(|| Error::NotFound(format!("{logical_path} at {version}")))?
            .clone();

        let content_path = self.inventory.content_path(&digest).ok_or_else(|| {
            // A state digest with no manifest entry is a corrupt inventory, not a
            // missing file — worth saying differently so the report is actionable.
            Error::Inventory(format!(
                "{logical_path} at {version} has digest {digest}, which the manifest does not map to any content path"
            ))
        })?;

        Ok(self.fs.read(&join(&self.root, content_path))?)
    }

    /// The history of one file, oldest first — **following renames**.
    ///
    /// This is the query the whole layout makes cheap, and the one a
    /// path-keyed store cannot answer: a file that moved is one life, not two
    /// unrelated ones. It is answered entirely from the inventory, with no
    /// content read.
    ///
    /// ## Why this is exact rather than a guess
    ///
    /// Version control that stores by path has to *infer* renames after the
    /// fact, usually by scoring content similarity, which is why such tools
    /// offer rename detection as a flag with a threshold and still get it wrong.
    /// OCFL keys state by digest, so a move is not an inference at all: the same
    /// digest is simply listed under a different logical path. Walking backwards
    /// and following that digest is a lookup.
    ///
    /// The walk starts at the newest version containing `logical_path` — so a
    /// deleted file still has a history — and steps back one version at a time.
    /// When the path is absent from an earlier version, the digest last seen is
    /// looked for elsewhere in that version; finding it means a rename, and the
    /// walk continues under the older name. Finding nothing means the lineage
    /// began here.
    ///
    /// Consecutive versions where neither the bytes nor the name changed collapse
    /// into one entry, so the result reads as "what actually happened, and when"
    /// rather than one row per capture. A rename *is* something that happened, so
    /// it is never collapsed away even though the bytes are identical.
    pub fn history<'a>(&'a self, logical_path: &str) -> Vec<HistoryEntry<'a>> {
        // Snapshot each version's inverted state once; the walk consults them
        // repeatedly and rebuilding per step would be quadratic for no reason.
        let states: Vec<(VersionNum, BTreeMap<&'a str, &'a Digest>)> = self
            .inventory
            .versions
            .iter()
            .map(|(num, version)| (*num, version.files()))
            .collect();

        // Start at the newest version holding this path — so a *deleted* file
        // still has a history — and adopt the key from the inventory rather than
        // the caller's string, so every name reported afterwards is borrowed
        // from the object itself.
        let Some((start, mut current)) =
            states
                .iter()
                .enumerate()
                .rev()
                .find_map(|(index, (_, files))| {
                    files
                        .get_key_value(logical_path)
                        .map(|(name, _)| (index, *name))
                })
        else {
            return Vec::new();
        };

        let mut newest_first: Vec<HistoryEntry<'a>> = Vec::new();
        for index in (0..=start).rev() {
            let (version, files) = &states[index];

            let entry = match files.get(current).copied() {
                Some(digest) => HistoryEntry {
                    version: *version,
                    digest,
                    logical_path: current,
                },
                None => {
                    // The name is gone from this older version. If the bytes are
                    // here under another name, the file was renamed *into* the
                    // name we were following; keep walking under the old one.
                    let looking_for = match newest_first.last() {
                        Some(previous) => previous.digest,
                        None => break,
                    };
                    // A rename source must have *vanished*. A name still present
                    // in the newer version was copied from, not moved — following
                    // it would graft someone else's past onto this file. (Index
                    // `index + 1` always exists: the path is present at `start`,
                    // so this branch only runs below it.)
                    let newer = &states[index + 1].1;
                    let Some((older_name, digest)) = files
                        .iter()
                        .find(|(name, d)| **d == looking_for && !newer.contains_key(*name))
                        .map(|(p, d)| (*p, *d))
                    else {
                        break;
                    };
                    current = older_name;
                    HistoryEntry {
                        version: *version,
                        digest,
                        logical_path: older_name,
                    }
                }
            };

            // Collapse only when nothing observable changed. A rename carries the
            // same digest and must still surface as its own event.
            let unchanged = newest_first.last().is_some_and(|newer| {
                newer.digest == entry.digest && newer.logical_path == entry.logical_path
            });
            if unchanged {
                newest_first.pop();
            }
            newest_first.push(entry);
        }

        newest_first.reverse();
        newest_first
    }

    /// Write a new version whose logical state is exactly `files`.
    ///
    /// `files` is the **complete** state of the version, not a delta: a path
    /// omitted here is deleted as of this version, and a path whose bytes are
    /// unchanged costs nothing but a manifest reference. That mirrors how OCFL
    /// models a version and removes any need to diff against the previous one.
    ///
    /// Bytes already present under any earlier version are never rewritten — the
    /// manifest is consulted first, so an unchanged photo across a hundred
    /// versions is stored once.
    ///
    /// The iterator is consumed lazily and one entry at a time, so a caller
    /// streaming a large tree can read each file only as it is reached rather
    /// than materializing the whole vault in memory.
    pub fn commit(
        &mut self,
        files: impl IntoIterator<Item = (String, Vec<u8>)>,
        meta: VersionMeta,
    ) -> Result<VersionNum> {
        if !self.inventory.digest_algorithm.is_computable() {
            return Err(Error::UnsupportedDigest(self.inventory.digest_algorithm));
        }

        let version = match self.on_disk {
            false => VersionNum::FIRST,
            true => self.inventory.head.next()?,
        };

        // Stage in memory first: nothing touches the disk until the whole version
        // is known to be well-formed, so a bad logical path fails before it has
        // written half a version.
        let mut state: BTreeMap<Digest, Vec<String>> = BTreeMap::new();
        let mut to_write: Vec<(String, Vec<u8>)> = Vec::new();
        let mut new_manifest: BTreeMap<Digest, Vec<String>> = BTreeMap::new();

        for (logical_path, bytes) in files {
            check_path(&logical_path)?;
            let digest = self.inventory.digest_algorithm.digest(&bytes)?;

            let known =
                self.inventory.manifest.contains_key(&digest) || new_manifest.contains_key(&digest);
            if !known {
                let content_path = format!(
                    "{version}/{}/{logical_path}",
                    self.inventory.content_directory
                );
                check_path(&content_path)?;
                new_manifest.insert(digest.clone(), vec![content_path.clone()]);
                to_write.push((content_path, bytes));
            }

            let paths = state.entry(digest).or_default();
            if !paths.contains(&logical_path) {
                paths.push(logical_path);
            }
        }
        for paths in state.values_mut() {
            paths.sort();
        }

        if self.on_disk && self.inventory.versions.contains_key(&version) {
            return Err(Error::Conflict(format!(
                "version {version} already exists in {}",
                self.root.display()
            )));
        }

        // Build the inventory this commit will publish, without disturbing the
        // live one — if a write fails, `self` still describes what is on disk.
        let mut next = self.inventory.clone();
        next.manifest.extend(new_manifest);
        next.head = version;
        next.versions.insert(
            version,
            Version {
                created: meta.created,
                message: meta.message,
                user: meta.user,
                state,
            },
        );
        let inventory_json = next.to_json()?;
        let sidecar_name = format!("{INVENTORY}.{}", next.digest_algorithm);
        let sidecar_body = format!(
            "{}  {INVENTORY}\n",
            next.digest_algorithm.digest(inventory_json.as_bytes())?
        );

        // ---- 1. content ----
        for (content_path, bytes) in to_write {
            let full = join(&self.root, &content_path);
            if let Some(parent) = full.parent() {
                self.fs.create_dir_all(parent)?;
            }
            self.fs.write(&full, &bytes)?;
        }

        // ---- 2. the declaration, on first commit ----
        if !self.on_disk {
            self.fs.create_dir_all(&self.root)?;
            // The declaration's *content* must be the version string plus a
            // newline; the filename alone is not the whole statement.
            self.fs.write(
                &self.root.join(declaration_name(&self.spec_version)),
                format!("{}\n", self.spec_version).as_bytes(),
            )?;
        }

        // ---- 3. the version inventory ----
        let version_dir = self.root.join(version.to_string());
        self.fs.create_dir_all(&version_dir)?;
        if self.write_version_inventories {
            self.fs
                .write(&version_dir.join(INVENTORY), inventory_json.as_bytes())?;
            self.fs
                .write(&version_dir.join(&sidecar_name), sidecar_body.as_bytes())?;
        }

        // ---- 4. the root inventory — the commit point ----
        self.fs
            .write(&self.root.join(INVENTORY), inventory_json.as_bytes())?;
        self.fs
            .write(&self.root.join(&sidecar_name), sidecar_body.as_bytes())?;

        self.inventory = next;
        self.on_disk = true;
        Ok(version)
    }

    /// Borrow the storage backend.
    pub fn storage(&self) -> &S {
        &self.fs
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::StdFs;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mocfl-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("object")
    }

    fn file(path: &str, body: &str) -> (String, Vec<u8>) {
        (path.to_string(), body.as_bytes().to_vec())
    }

    fn object(root: &Path) -> Object<StdFs> {
        Object::create(StdFs, root, "urn:example:vault", DigestAlgorithm::Sha256)
    }

    #[test]
    fn the_first_commit_creates_a_readable_object() {
        let root = tmp("first-commit");
        let mut obj = object(&root);
        let v1 = obj
            .commit(
                [file("letter.md", "Dear Mother,")],
                VersionMeta::at("2026-07-27T09:14:00Z").message("first"),
            )
            .unwrap();

        assert_eq!(v1, VersionNum::FIRST);
        assert_eq!(
            std::fs::read_to_string(root.join(DECLARATION)).unwrap(),
            "ocfl_object_1.1\n"
        );
        assert!(root.join("inventory.json").exists());
        assert!(root.join("inventory.json.sha256").exists());
        assert!(root.join("v1/inventory.json").exists());
        assert_eq!(obj.read(v1, "letter.md").unwrap(), b"Dear Mother,");
    }

    #[test]
    fn nothing_is_written_before_the_first_commit() {
        // An object with no versions is invalid per the spec, so `create` must
        // not leave a half-formed directory behind.
        let root = tmp("no-premature-write");
        let obj = object(&root);
        assert!(!root.exists());
        assert_eq!(obj.head(), None);
    }

    #[test]
    fn unchanged_bytes_are_stored_once_across_versions() {
        // The dedup property the manifest exists for: a photo that never changes
        // does not accumulate a copy per capture.
        let root = tmp("dedup");
        let mut obj = object(&root);
        obj.commit(
            [file("a.md", "same"), file("b.md", "same")],
            VersionMeta::at("2026-07-27T09:00:00Z"),
        )
        .unwrap();
        obj.commit(
            [file("a.md", "same"), file("b.md", "same")],
            VersionMeta::at("2026-07-27T10:00:00Z"),
        )
        .unwrap();

        assert_eq!(obj.inventory().manifest.len(), 1, "one digest, one blob");
        assert!(!root.join("v2/content").exists(), "v2 stored no new bytes");
        // Both logical paths still resolve at both versions.
        for v in [VersionNum::new(1), VersionNum::new(2)] {
            assert_eq!(obj.read(v, "a.md").unwrap(), b"same");
            assert_eq!(obj.read(v, "b.md").unwrap(), b"same");
        }
    }

    #[test]
    fn a_version_state_is_complete_so_an_omitted_path_is_deleted() {
        let root = tmp("delete-by-omission");
        let mut obj = object(&root);
        obj.commit(
            [file("keep.md", "k"), file("drop.md", "d")],
            VersionMeta::at("2026-07-27T09:00:00Z"),
        )
        .unwrap();
        let v2 = obj
            .commit(
                [file("keep.md", "k")],
                VersionMeta::at("2026-07-27T10:00:00Z"),
            )
            .unwrap();

        assert!(obj.read(v2, "drop.md").is_err(), "gone at v2");
        // ...but still readable at v1. That is the whole point.
        assert_eq!(obj.read(VersionNum::FIRST, "drop.md").unwrap(), b"d");
    }

    #[test]
    fn a_rename_moves_no_bytes_and_keeps_one_lineage() {
        let root = tmp("rename");
        let mut obj = object(&root);
        obj.commit(
            [file("old.md", "body")],
            VersionMeta::at("2026-07-27T09:00:00Z"),
        )
        .unwrap();
        obj.commit(
            [file("new.md", "body")],
            VersionMeta::at("2026-07-27T10:00:00Z"),
        )
        .unwrap();

        assert_eq!(obj.inventory().manifest.len(), 1, "no new content written");
        assert_eq!(obj.read(VersionNum::new(2), "new.md").unwrap(), b"body");
    }

    #[test]
    fn history_collapses_versions_where_the_bytes_did_not_change() {
        let root = tmp("history");
        let mut obj = object(&root);
        obj.commit(
            [file("a.md", "one")],
            VersionMeta::at("2026-07-27T09:00:00Z"),
        )
        .unwrap();
        obj.commit(
            [file("a.md", "one")],
            VersionMeta::at("2026-07-27T10:00:00Z"),
        )
        .unwrap();
        obj.commit(
            [file("a.md", "two")],
            VersionMeta::at("2026-07-27T11:00:00Z"),
        )
        .unwrap();

        let history = obj.history("a.md");
        assert_eq!(
            history
                .iter()
                .map(|e| e.version.number())
                .collect::<Vec<_>>(),
            [1, 3],
            "v2 changed nothing, so it is not a point in this file's history"
        );
        assert!(obj.history("never-existed.md").is_empty());
    }

    #[test]
    fn history_follows_a_rename_back_to_the_original_name() {
        // The query a path-keyed store cannot answer. Asking about the *new*
        // name must reach back past the rename to where the bytes first
        // appeared — and must report the name the file had at each point.
        let root = tmp("history-rename");
        let mut obj = object(&root);
        obj.commit(
            [file("scan.jpg", "bytes")],
            VersionMeta::at("2026-07-27T09:00:00Z"),
        )
        .unwrap();
        obj.commit(
            [file("letters/1943-scan.jpg", "bytes")],
            VersionMeta::at("2026-07-27T10:00:00Z"),
        )
        .unwrap();
        obj.commit(
            [file("letters/1943-scan.jpg", "rescanned")],
            VersionMeta::at("2026-07-27T11:00:00Z"),
        )
        .unwrap();

        let history = obj.history("letters/1943-scan.jpg");
        assert_eq!(
            history
                .iter()
                .map(|e| (e.version.number(), e.logical_path))
                .collect::<Vec<_>>(),
            [
                (1, "scan.jpg"),
                (2, "letters/1943-scan.jpg"),
                (3, "letters/1943-scan.jpg")
            ],
            "one life, under two names"
        );
    }

    #[test]
    fn a_rename_is_an_event_even_though_the_bytes_are_identical() {
        // Collapsing is about "nothing observable changed". A move changed the
        // name, so it must survive the collapse that identical bytes would
        // otherwise trigger.
        let root = tmp("history-rename-event");
        let mut obj = object(&root);
        obj.commit(
            [file("old.md", "same")],
            VersionMeta::at("2026-07-27T09:00:00Z"),
        )
        .unwrap();
        obj.commit(
            [file("new.md", "same")],
            VersionMeta::at("2026-07-27T10:00:00Z"),
        )
        .unwrap();

        let history = obj.history("new.md");
        assert_eq!(history.len(), 2, "{history:?}");
        assert_eq!(history[0].digest, history[1].digest, "bytes never changed");
        assert_eq!(history[0].logical_path, "old.md");
        assert_eq!(history[1].logical_path, "new.md");
    }

    #[test]
    fn a_deleted_file_still_has_a_history() {
        // The walk starts at the newest version that *has* the path, not at the
        // head — otherwise deleting something would erase its past.
        let root = tmp("history-deleted");
        let mut obj = object(&root);
        obj.commit(
            [file("keep.md", "k"), file("gone.md", "g")],
            VersionMeta::at("2026-07-27T09:00:00Z"),
        )
        .unwrap();
        obj.commit(
            [file("keep.md", "k")],
            VersionMeta::at("2026-07-27T10:00:00Z"),
        )
        .unwrap();

        let history = obj.history("gone.md");
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].version, VersionNum::FIRST);
    }

    #[test]
    fn history_does_not_follow_a_digest_that_merely_appears_elsewhere() {
        // `copy.md` is created at v2 holding bytes that already existed under
        // `original.md`. That is a copy, not a rename: `original.md` never went
        // away, so `copy.md`'s life starts at v2.
        let root = tmp("history-copy");
        let mut obj = object(&root);
        obj.commit(
            [file("original.md", "shared")],
            VersionMeta::at("2026-07-27T09:00:00Z"),
        )
        .unwrap();
        obj.commit(
            [file("original.md", "shared"), file("copy.md", "shared")],
            VersionMeta::at("2026-07-27T10:00:00Z"),
        )
        .unwrap();

        let history = obj.history("copy.md");
        assert_eq!(
            history
                .iter()
                .map(|e| e.version.number())
                .collect::<Vec<_>>(),
            [2],
            "a copy is not a rename"
        );
    }

    #[test]
    fn an_object_reopens_to_the_same_state() {
        let root = tmp("reopen");
        let mut obj = object(&root);
        obj.commit(
            [file("a.md", "one")],
            VersionMeta::at("2026-07-27T09:00:00Z")
                .message("m")
                .user("Adam", Some("mailto:adam@example.org".into())),
        )
        .unwrap();
        obj.commit(
            [file("a.md", "two")],
            VersionMeta::at("2026-07-27T10:00:00Z"),
        )
        .unwrap();

        let reopened = Object::open(StdFs, &root).unwrap();
        assert_eq!(reopened.inventory(), obj.inventory());
        assert_eq!(reopened.id(), "urn:example:vault");
        assert_eq!(reopened.read(VersionNum::FIRST, "a.md").unwrap(), b"one");
        assert_eq!(
            reopened
                .version(VersionNum::FIRST)
                .unwrap()
                .user
                .name
                .as_deref(),
            Some("Adam")
        );
    }

    #[test]
    fn committing_onto_a_reopened_object_continues_the_history() {
        let root = tmp("reopen-commit");
        let mut obj = object(&root);
        obj.commit(
            [file("a.md", "one")],
            VersionMeta::at("2026-07-27T09:00:00Z"),
        )
        .unwrap();

        let mut reopened = Object::open(StdFs, &root).unwrap();
        let v2 = reopened
            .commit(
                [file("a.md", "two")],
                VersionMeta::at("2026-07-27T10:00:00Z"),
            )
            .unwrap();

        assert_eq!(v2, VersionNum::new(2));
        assert_eq!(reopened.versions().len(), 2);
        assert_eq!(reopened.read(VersionNum::FIRST, "a.md").unwrap(), b"one");
    }

    #[test]
    fn the_sidecar_records_the_digest_of_the_inventory_actually_written() {
        // The sidecar is fixity over the inventory itself; if it disagreed with
        // the bytes on disk, every validator would reject the object.
        let root = tmp("sidecar");
        let mut obj = object(&root);
        obj.commit([file("a.md", "x")], VersionMeta::at("2026-07-27T09:00:00Z"))
            .unwrap();

        let json = std::fs::read(root.join("inventory.json")).unwrap();
        let sidecar = std::fs::read_to_string(root.join("inventory.json.sha256")).unwrap();
        let expected = DigestAlgorithm::Sha256.digest(&json).unwrap();
        assert_eq!(sidecar, format!("{expected}  inventory.json\n"));
    }

    #[test]
    fn a_path_that_would_escape_the_object_is_refused_before_anything_is_written() {
        let root = tmp("bad-path");
        let mut obj = object(&root);
        let err = obj.commit(
            [file("../escape.md", "x")],
            VersionMeta::at("2026-07-27T09:00:00Z"),
        );
        assert!(err.is_err());
        assert!(!root.exists(), "a rejected commit leaves nothing behind");
    }

    #[test]
    fn opening_a_directory_that_is_not_an_object_says_so() {
        let root = tmp("not-an-object");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("inventory.json"), b"{}").unwrap();
        let err = Object::open(StdFs, &root).unwrap_err();
        assert!(err.to_string().contains("declaration"), "{err}");
    }
}
