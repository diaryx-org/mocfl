//! Validation — is what is on disk actually a conformant object?
//!
//! Reading an object answers "what does this say?"; validation answers "should I
//! believe it?" They are separate passes because they have separate audiences: a
//! read failure is a problem for the calling program, while a validation finding
//! is a report about someone's archive — something to list, show, and act on.
//!
//! ## Two depths
//!
//! - **Structural** ([`validate`]) reads only the inventories and the directory
//!   listing: is the declaration right, are versions contiguous, does every state
//!   digest resolve, is every content file where the manifest says, does each
//!   version directory's inventory agree with the root. Cheap enough to run on
//!   every open.
//! - **Deep** ([`validate_deep`]) additionally rehashes every content file. This
//!   is the bit-rot pass — the question no amount of structural checking can
//!   answer — and it costs a full read of the object.
//!
//! ## On error codes
//!
//! The spec publishes a numbered validation-code list (`E001`, `E003`, …) that a
//! mature validator reports against. This crate deliberately does **not** invent
//! or guess those numbers: a wrong code is worse than no code, because it sends a
//! reader to the wrong clause. [`Violation`] names each condition in prose, and
//! mapping the variants onto the published codes is a task to be done against
//! that document rather than from memory.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::Path;

use crate::digest::{Digest, DigestAlgorithm};
use crate::error::Result;
use crate::fs::{Storage, join};
use crate::inventory::{Inventory, Version, VersionNum, conflicting_paths};
use crate::object::{INVENTORY, Object, declaration_name};

/// One way an object fails to conform.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Violation {
    /// The NAMASTE declaration is missing, unreadable, or does not contain the
    /// spec version its filename claims.
    Declaration {
        /// What was wrong with it.
        detail: String,
    },
    /// The inventory digest sidecar is missing.
    SidecarMissing {
        /// The sidecar filename that should have been there.
        name: String,
    },
    /// The sidecar records a digest that is not the inventory's actual digest —
    /// the inventory has been altered since it was written.
    SidecarMismatch {
        /// The digest the sidecar claims.
        recorded: String,
        /// The digest the inventory bytes actually produce.
        actual: String,
    },
    /// Version numbers must run from v1 with no gaps.
    VersionsNotContiguous {
        /// The version absent from the sequence.
        missing: VersionNum,
    },
    /// A version directory exists on disk that the inventory does not know about
    /// — most often an interrupted commit that never reached its commit point.
    UnexpectedVersionDirectory {
        /// The directory name found in the object root.
        name: String,
    },
    /// A version the inventory declares has no directory on disk.
    VersionDirectoryMissing {
        /// The version with nothing on disk.
        version: VersionNum,
    },
    /// A version's state references a digest the manifest does not map.
    StateDigestNotInManifest {
        /// The version whose state is dangling.
        version: VersionNum,
        /// The logical path holding the unmapped digest.
        logical_path: String,
        /// The digest the manifest does not know.
        digest: Digest,
    },
    /// The manifest points at a content file that is not there.
    ContentMissing {
        /// The digest whose bytes are gone.
        digest: Digest,
        /// Where the manifest says they should be.
        content_path: String,
    },
    /// A content file's bytes no longer hash to the digest that addresses them —
    /// corruption, or an edit made behind the object's back. Deep pass only.
    ContentDigestMismatch {
        /// The file whose bytes drifted.
        content_path: String,
        /// The digest addressing it in the manifest.
        recorded: Digest,
        /// What its bytes hash to now.
        actual: Digest,
    },
    /// The object is addressed with an algorithm this crate cannot compute, so
    /// content could not be verified. Reported rather than passed over in
    /// silence: "not checked" must never read as "checked and fine".
    ContentUnverifiable {
        /// The algorithm that could not be computed.
        algorithm: String,
    },
    /// A version's state lists one logical path twice, or a path and another
    /// beneath it, so it describes a tree no filesystem can hold.
    LogicalPathConflict {
        /// The version whose state conflicts.
        version: VersionNum,
        /// The shorter path, or the repeated one.
        path: String,
        /// The path equal to `path` or beneath it.
        other: String,
    },
    /// The manifest lists one content path twice, or a path and another
    /// beneath it, so one of them cannot be on disk as the manifest says.
    ContentPathConflict {
        /// The shorter path, or the repeated one.
        path: String,
        /// The path equal to `path` or beneath it.
        other: String,
    },
    /// A version directory holds an inventory with no sidecar beside it.
    VersionSidecarMissing {
        /// The version whose directory holds the inventory.
        version: VersionNum,
        /// The sidecar filename that should have been there.
        name: String,
    },
    /// A version directory's sidecar records a digest that is not its
    /// inventory's actual digest.
    VersionSidecarMismatch {
        /// The version whose directory holds the inventory.
        version: VersionNum,
        /// The digest the sidecar claims.
        recorded: String,
        /// The digest the inventory bytes actually produce.
        actual: String,
    },
    /// A version directory's inventory cannot be read as an inventory.
    VersionInventoryUnreadable {
        /// The version whose directory holds the inventory.
        version: VersionNum,
        /// Why it could not be read.
        detail: String,
    },
    /// The root inventory is not byte for byte the inventory in the head
    /// version's directory.
    HeadInventoryDiffers {
        /// The head version.
        version: VersionNum,
    },
    /// A version directory's inventory names a different object from the root.
    VersionIdDiffers {
        /// The version whose directory holds the inventory.
        version: VersionNum,
        /// The `id` that inventory records.
        id: String,
        /// The `id` the root inventory records.
        root_id: String,
    },
    /// A version directory's inventory claims an older spec than the version
    /// before it. An object's spec version may only move forward.
    SpecVersionMovedBack {
        /// The version claiming the older spec.
        version: VersionNum,
        /// The `type` it records.
        type_uri: String,
        /// The version before it.
        earlier: VersionNum,
        /// The `type` that version records.
        earlier_type_uri: String,
    },
    /// A version directory's inventory records a version the root does not.
    VersionUnknownToRoot {
        /// The version whose directory holds the inventory.
        inventory: VersionNum,
        /// The version block the root inventory lacks.
        version: VersionNum,
    },
    /// A version directory's inventory and the root disagree about what a
    /// logical path held at a version: one lacks it, or they give it different
    /// content.
    VersionStateDiffers {
        /// The version whose directory holds the inventory.
        inventory: VersionNum,
        /// The version block they disagree about.
        version: VersionNum,
        /// The logical path they disagree about.
        logical_path: String,
    },
}

/// Say how `other` conflicts with `path`, for a violation's message.
fn describe_conflict(path: &str, other: &str) -> String {
    if path == other {
        format!("{path} is listed twice")
    } else {
        format!("{other} lies beneath {path}, which is a file")
    }
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Declaration { detail } => write!(f, "object declaration: {detail}"),
            Self::SidecarMissing { name } => write!(f, "inventory sidecar {name} is missing"),
            Self::SidecarMismatch { recorded, actual } => write!(
                f,
                "inventory sidecar records {recorded} but the inventory hashes to {actual}"
            ),
            Self::VersionsNotContiguous { missing } => {
                write!(f, "version {missing} is missing from the sequence")
            }
            Self::UnexpectedVersionDirectory { name } => write!(
                f,
                "directory {name} is not a version the inventory declares (an interrupted commit?)"
            ),
            Self::VersionDirectoryMissing { version } => {
                write!(f, "version {version} has no directory on disk")
            }
            Self::StateDigestNotInManifest {
                version,
                logical_path,
                digest,
            } => write!(
                f,
                "{logical_path} at {version} has digest {digest}, which the manifest does not map"
            ),
            Self::ContentMissing {
                digest,
                content_path,
            } => write!(f, "content for {digest} is missing at {content_path}"),
            Self::ContentDigestMismatch {
                content_path,
                recorded,
                actual,
            } => write!(
                f,
                "{content_path} hashes to {actual} but is addressed as {recorded}"
            ),
            Self::ContentUnverifiable { algorithm } => write!(
                f,
                "content is addressed with {algorithm}, which this crate cannot compute — not verified"
            ),
            Self::LogicalPathConflict {
                version,
                path,
                other,
            } => write!(
                f,
                "logical paths at {version} conflict: {}",
                describe_conflict(path, other)
            ),
            Self::ContentPathConflict { path, other } => write!(
                f,
                "manifest content paths conflict: {}",
                describe_conflict(path, other)
            ),
            Self::VersionSidecarMissing { version, name } => {
                write!(f, "inventory sidecar {version}/{name} is missing")
            }
            Self::VersionSidecarMismatch {
                version,
                recorded,
                actual,
            } => write!(
                f,
                "{version}'s inventory sidecar records {recorded} but its inventory hashes to {actual}"
            ),
            Self::VersionInventoryUnreadable { version, detail } => {
                write!(f, "{version}'s inventory could not be read: {detail}")
            }
            Self::HeadInventoryDiffers { version } => {
                write!(f, "the root inventory is not identical to {version}'s")
            }
            Self::VersionIdDiffers {
                version,
                id,
                root_id,
            } => write!(
                f,
                "{version}'s inventory has id {id}, but the root's is {root_id}"
            ),
            Self::SpecVersionMovedBack {
                version,
                type_uri,
                earlier,
                earlier_type_uri,
            } => write!(
                f,
                "{version}'s inventory has type {type_uri}, older than {earlier}'s {earlier_type_uri}"
            ),
            Self::VersionUnknownToRoot { inventory, version } => write!(
                f,
                "{inventory}'s inventory records {version}, which the root does not"
            ),
            Self::VersionStateDiffers {
                inventory,
                version,
                logical_path,
            } => write!(
                f,
                "{inventory}'s inventory and the root disagree about {logical_path} at {version}"
            ),
        }
    }
}

/// Structural validation: everything but rehashing content.
pub fn validate<S: Storage>(object: &Object<S>) -> Result<Vec<Violation>> {
    check(object, false)
}

/// Structural validation plus a rehash of every content file — the bit-rot pass.
pub fn validate_deep<S: Storage>(object: &Object<S>) -> Result<Vec<Violation>> {
    check(object, true)
}

fn check<S: Storage>(object: &Object<S>, deep: bool) -> Result<Vec<Violation>> {
    let fs = object.storage();
    let root = object.root();
    let inventory = object.inventory();
    let mut found = Vec::new();

    // ---- the declaration ----
    //
    // The filename states a version and the file's contents must repeat it. A
    // directory whose name and contents disagree is making two different claims
    // about what it is, which defeats the point of declaring at all.
    let spec_version = object.spec_version();
    let name = declaration_name(spec_version);
    match fs.read(&root.join(&name)) {
        Ok(bytes) => {
            let text = String::from_utf8_lossy(&bytes);
            if text.trim_end_matches(['\r', '\n']) != spec_version {
                found.push(Violation::Declaration {
                    detail: format!(
                        "{name} should contain {spec_version:?}, found {:?}",
                        text.trim()
                    ),
                });
            }
        }
        Err(e) => found.push(Violation::Declaration {
            detail: format!("{name} could not be read: {e}"),
        }),
    }

    // ---- the sidecar over the inventory ----
    let algorithm = inventory.digest_algorithm;
    let root_bytes = fs.read(&root.join(INVENTORY))?;
    match sidecar_fault(fs, root, &root_bytes, algorithm)? {
        None => {}
        Some(SidecarFault::Missing { name }) => found.push(Violation::SidecarMissing { name }),
        Some(SidecarFault::Mismatch { recorded, actual }) => {
            found.push(Violation::SidecarMismatch { recorded, actual });
        }
    }

    // ---- versions run 1..=head with no gaps ----
    let declared: BTreeSet<VersionNum> = inventory.versions.keys().copied().collect();
    for n in 1..=inventory.head.number() {
        let expected = VersionNum::new(n);
        if !declared.iter().any(|v| v.number() == n) {
            found.push(Violation::VersionsNotContiguous { missing: expected });
        }
    }

    // ---- version directories on disk agree with the inventory ----
    let on_disk: BTreeSet<String> = fs
        .read_dir(root)?
        .into_iter()
        .filter(|(name, is_dir)| *is_dir && name.starts_with('v'))
        .map(|(name, _)| name)
        .collect();
    for version in &declared {
        if !on_disk.contains(&version.to_string()) {
            found.push(Violation::VersionDirectoryMissing { version: *version });
        }
    }
    for name in &on_disk {
        let known = VersionNum::parse(name)
            .map(|v| declared.contains(&v) && v.to_string() == *name)
            .unwrap_or(false);
        if !known {
            found.push(Violation::UnexpectedVersionDirectory { name: name.clone() });
        }
    }

    // ---- each version directory's own inventory agrees with the root ----
    check_version_inventories(fs, root, inventory, &root_bytes, &mut found)?;

    // ---- every state digest resolves through the manifest ----
    for (version, record) in &inventory.versions {
        for (logical_path, digest) in record.files() {
            if !inventory.manifest.contains_key(digest) {
                found.push(Violation::StateDigestNotInManifest {
                    version: *version,
                    logical_path: logical_path.to_string(),
                    digest: digest.clone(),
                });
            }
        }
    }

    // ---- each version's logical paths, and the manifest's content paths, make
    // a tree: none repeats and none lies beneath another ----
    for (version, record) in &inventory.versions {
        let paths = record.state.values().flatten().map(String::as_str);
        for (path, other) in conflicting_paths(paths) {
            found.push(Violation::LogicalPathConflict {
                version: *version,
                path: path.to_string(),
                other: other.to_string(),
            });
        }
    }
    let content_paths = inventory.manifest.values().flatten().map(String::as_str);
    for (path, other) in conflicting_paths(content_paths) {
        found.push(Violation::ContentPathConflict {
            path: path.to_string(),
            other: other.to_string(),
        });
    }

    // ---- content is where the manifest says, and (deep) still itself ----
    if deep && !algorithm.is_computable() {
        found.push(Violation::ContentUnverifiable {
            algorithm: algorithm.to_string(),
        });
    }
    for (digest, paths) in &inventory.manifest {
        for content_path in paths {
            let full = join(root, content_path);
            match fs.read(&full) {
                Err(_) => found.push(Violation::ContentMissing {
                    digest: digest.clone(),
                    content_path: content_path.clone(),
                }),
                Ok(bytes) if deep && algorithm.is_computable() => {
                    let actual = algorithm.digest(&bytes)?;
                    if actual != *digest {
                        found.push(Violation::ContentDigestMismatch {
                            content_path: content_path.clone(),
                            recorded: digest.clone(),
                            actual,
                        });
                    }
                }
                Ok(_) => {}
            }
        }
    }

    Ok(found)
}

/// What is wrong with the sidecar beside an inventory.
enum SidecarFault {
    Missing { name: String },
    Mismatch { recorded: String, actual: String },
}

/// Check the sidecar in `dir` against the inventory `bytes` found there.
///
/// An algorithm this crate cannot compute leaves only the sidecar's presence
/// to check.
fn sidecar_fault<S: Storage>(
    fs: &S,
    dir: &Path,
    bytes: &[u8],
    algorithm: DigestAlgorithm,
) -> Result<Option<SidecarFault>> {
    let name = format!("{INVENTORY}.{algorithm}");
    let Ok(sidecar) = fs.read(&dir.join(&name)) else {
        return Ok(Some(SidecarFault::Missing { name }));
    };
    if !algorithm.is_computable() {
        return Ok(None);
    }
    let text = String::from_utf8_lossy(&sidecar);
    let recorded = text.split_whitespace().next().unwrap_or("").to_string();
    let actual = algorithm.digest(bytes)?;
    if recorded.eq_ignore_ascii_case(actual.as_str()) {
        return Ok(None);
    }
    Ok(Some(SidecarFault::Mismatch {
        recorded,
        actual: actual.to_string(),
    }))
}

/// Hold the inventory in each version directory to the root's.
///
/// A version directory's inventory is a SHOULD, so one that is absent is no
/// violation. These rules bind where one exists.
fn check_version_inventories<S: Storage>(
    fs: &S,
    root: &Path,
    inventory: &Inventory,
    root_bytes: &[u8],
    found: &mut Vec<Violation>,
) -> Result<()> {
    // The newest version whose `type` has been read, for the next to follow.
    let mut last_type: Option<(VersionNum, String)> = None;

    for version in inventory.versions.keys().copied() {
        let dir = root.join(version.to_string());
        let path = dir.join(INVENTORY);
        if !fs.exists(&path)? {
            continue;
        }
        let bytes = match fs.read(&path) {
            Ok(bytes) => bytes,
            Err(e) => {
                found.push(Violation::VersionInventoryUnreadable {
                    version,
                    detail: e.to_string(),
                });
                continue;
            }
        };
        if version == inventory.head && bytes != root_bytes {
            found.push(Violation::HeadInventoryDiffers { version });
        }

        // An inventory that does not parse names no algorithm of its own, so
        // the sidecar to look for is the one the root's would have.
        let parsed = Inventory::parse(&bytes);
        let algorithm = parsed
            .as_ref()
            .map_or(inventory.digest_algorithm, |held| held.digest_algorithm);
        match sidecar_fault(fs, &dir, &bytes, algorithm)? {
            None => {}
            Some(SidecarFault::Missing { name }) => {
                found.push(Violation::VersionSidecarMissing { version, name });
            }
            Some(SidecarFault::Mismatch { recorded, actual }) => {
                found.push(Violation::VersionSidecarMismatch {
                    version,
                    recorded,
                    actual,
                });
            }
        }

        let held = match parsed {
            Ok(held) => held,
            Err(e) => {
                found.push(Violation::VersionInventoryUnreadable {
                    version,
                    detail: e.to_string(),
                });
                continue;
            }
        };
        if held.id != inventory.id {
            found.push(Violation::VersionIdDiffers {
                version,
                id: held.id.clone(),
                root_id: inventory.id.clone(),
            });
        }
        check_spec_order(last_type.as_ref(), version, &held.type_uri, found);
        check_prior_states(inventory, &held, version, found);
        last_type = Some((version, held.type_uri));
    }

    // The root speaks for the head, whether or not its directory holds a copy.
    check_spec_order(
        last_type.as_ref(),
        inventory.head,
        &inventory.type_uri,
        found,
    );
    Ok(())
}

/// Report `type_uri`, at `version`, if it claims an older spec than `last`.
fn check_spec_order(
    last: Option<&(VersionNum, String)>,
    version: VersionNum,
    type_uri: &str,
    found: &mut Vec<Violation>,
) {
    let Some((earlier, earlier_type_uri)) = last else {
        return;
    };
    if let (Some(spec), Some(earlier_spec)) = (spec_of(type_uri), spec_of(earlier_type_uri))
        && spec < earlier_spec
    {
        found.push(Violation::SpecVersionMovedBack {
            version,
            type_uri: type_uri.to_string(),
            earlier: *earlier,
            earlier_type_uri: earlier_type_uri.clone(),
        });
    }
}

/// The spec version an inventory's `type` names, as `(major, minor)`, or `None`
/// for a URI that does not have the spec's shape.
fn spec_of(type_uri: &str) -> Option<(u32, u32)> {
    let rest = type_uri.strip_prefix("https://ocfl.io/")?;
    let (spec, _) = rest.split_once('/')?;
    let (major, minor) = spec.split_once('.')?;
    Some((major.parse().ok()?, minor.parse().ok()?))
}

/// Hold every version block in `held`, the inventory in `directory`, to the
/// same block in the root.
///
/// Digests can be compared only under one algorithm. Under two, the content
/// paths they lead to are what can agree, so those are held to it either way:
/// the older manifest's paths must all still be listed for that logical path.
fn check_prior_states(
    inventory: &Inventory,
    held: &Inventory,
    directory: VersionNum,
    found: &mut Vec<Violation>,
) {
    let same_algorithm = held.digest_algorithm == inventory.digest_algorithm;
    for (version, block) in &held.versions {
        let Some(root_block) = inventory.versions.get(version) else {
            found.push(Violation::VersionUnknownToRoot {
                inventory: directory,
                version: *version,
            });
            continue;
        };
        let held_paths = logical_paths(held, block);
        let root_paths = logical_paths(inventory, root_block);
        let every_path: BTreeSet<&str> = held_paths
            .keys()
            .chain(root_paths.keys())
            .copied()
            .collect();
        for path in every_path {
            let agrees = match (held_paths.get(path), root_paths.get(path)) {
                (Some((digest, content)), Some((root_digest, root_content))) => {
                    (!same_algorithm || digest == root_digest) && content.is_subset(root_content)
                }
                _ => false,
            };
            if !agrees {
                found.push(Violation::VersionStateDiffers {
                    inventory: directory,
                    version: *version,
                    logical_path: path.to_string(),
                });
            }
        }
    }
}

/// Each logical path in `block`, with its digest and the content paths that
/// digest leads to in `inventory`'s manifest.
fn logical_paths<'a>(
    inventory: &'a Inventory,
    block: &'a Version,
) -> BTreeMap<&'a str, (&'a Digest, BTreeSet<&'a str>)> {
    block
        .files()
        .into_iter()
        .map(|(path, digest)| {
            let content = inventory
                .manifest
                .get(digest)
                .into_iter()
                .flatten()
                .map(String::as_str)
                .collect();
            (path, (digest, content))
        })
        .collect()
}

/// A count of each violation kind, for a one-line summary.
pub fn summarize(violations: &[Violation]) -> BTreeMap<&'static str, usize> {
    let mut counts = BTreeMap::new();
    for violation in violations {
        let key = match violation {
            Violation::Declaration { .. } => "declaration",
            Violation::SidecarMissing { .. } => "sidecar-missing",
            Violation::SidecarMismatch { .. } => "sidecar-mismatch",
            Violation::VersionsNotContiguous { .. } => "versions-not-contiguous",
            Violation::UnexpectedVersionDirectory { .. } => "unexpected-version-directory",
            Violation::VersionDirectoryMissing { .. } => "version-directory-missing",
            Violation::StateDigestNotInManifest { .. } => "state-digest-not-in-manifest",
            Violation::ContentMissing { .. } => "content-missing",
            Violation::ContentDigestMismatch { .. } => "content-digest-mismatch",
            Violation::ContentUnverifiable { .. } => "content-unverifiable",
            Violation::LogicalPathConflict { .. } => "logical-path-conflict",
            Violation::ContentPathConflict { .. } => "content-path-conflict",
            Violation::VersionSidecarMissing { .. } => "version-sidecar-missing",
            Violation::VersionSidecarMismatch { .. } => "version-sidecar-mismatch",
            Violation::VersionInventoryUnreadable { .. } => "version-inventory-unreadable",
            Violation::HeadInventoryDiffers { .. } => "head-inventory-differs",
            Violation::VersionIdDiffers { .. } => "version-id-differs",
            Violation::SpecVersionMovedBack { .. } => "spec-version-moved-back",
            Violation::VersionUnknownToRoot { .. } => "version-unknown-to-root",
            Violation::VersionStateDiffers { .. } => "version-state-differs",
        };
        *counts.entry(key).or_insert(0) += 1;
    }
    counts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::digest::{Digest, DigestAlgorithm};
    use crate::fs::StdFs;
    use crate::inventory::Inventory;
    use crate::object::VersionMeta;
    use std::path::PathBuf;

    fn tmp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("mocfl-validate-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("object")
    }

    fn committed(root: &std::path::Path) -> Object<StdFs> {
        let mut obj = Object::create(StdFs, root, "urn:example:v", DigestAlgorithm::Sha256);
        obj.commit(
            [("a.md".to_string(), b"one".to_vec())],
            VersionMeta::at("2026-07-27T09:00:00Z"),
        )
        .unwrap();
        obj.commit(
            [("a.md".to_string(), b"two".to_vec())],
            VersionMeta::at("2026-07-27T10:00:00Z"),
        )
        .unwrap();
        obj
    }

    #[test]
    fn an_object_this_crate_wrote_validates_clean() {
        let root = tmp("clean");
        let obj = committed(&root);
        assert_eq!(validate(&obj).unwrap(), vec![]);
        assert_eq!(validate_deep(&obj).unwrap(), vec![]);
    }

    #[test]
    fn a_corrupted_content_file_is_caught_only_by_the_deep_pass() {
        // The distinction the two depths exist for: structure is intact, bytes
        // are not. Nothing short of rehashing can see this.
        let root = tmp("bit-rot");
        let obj = committed(&root);
        let content = obj
            .inventory()
            .content_path(obj.inventory().manifest.keys().next().unwrap())
            .unwrap()
            .to_string();
        std::fs::write(join(&root, &content), b"tampered").unwrap();

        assert_eq!(validate(&obj).unwrap(), vec![], "structure is untouched");
        let deep = validate_deep(&obj).unwrap();
        assert!(
            matches!(deep.as_slice(), [Violation::ContentDigestMismatch { .. }]),
            "{deep:?}"
        );
    }

    #[test]
    fn a_missing_content_file_is_caught_by_the_cheap_pass() {
        let root = tmp("content-missing");
        let obj = committed(&root);
        let content = obj
            .inventory()
            .content_path(obj.inventory().manifest.keys().next().unwrap())
            .unwrap()
            .to_string();
        std::fs::remove_file(join(&root, &content)).unwrap();

        let found = validate(&obj).unwrap();
        assert!(
            found
                .iter()
                .any(|v| matches!(v, Violation::ContentMissing { .. })),
            "{found:?}"
        );
    }

    #[test]
    fn an_altered_inventory_is_caught_by_its_sidecar() {
        let root = tmp("sidecar-mismatch");
        let obj = committed(&root);
        let mut json = std::fs::read_to_string(root.join(INVENTORY)).unwrap();
        json.push(' '); // one byte, no semantic change
        std::fs::write(root.join(INVENTORY), json).unwrap();

        let found = validate(&obj).unwrap();
        assert!(
            found
                .iter()
                .any(|v| matches!(v, Violation::SidecarMismatch { .. })),
            "{found:?}"
        );
    }

    #[test]
    fn a_wrong_declaration_is_caught() {
        let root = tmp("bad-declaration");
        let obj = committed(&root);
        std::fs::write(root.join(crate::object::DECLARATION), b"ocfl_object_9.9\n").unwrap();
        let found = validate(&obj).unwrap();
        assert!(
            found
                .iter()
                .any(|v| matches!(v, Violation::Declaration { .. })),
            "{found:?}"
        );
    }

    #[test]
    fn an_orphaned_version_directory_from_an_interrupted_commit_is_reported() {
        // The exact residue the documented commit order can leave: content for a
        // version whose root inventory never landed. Untidy, not wrong — and it
        // must be visible rather than silently accumulating.
        let root = tmp("orphan-version");
        let obj = committed(&root);
        std::fs::create_dir_all(root.join("v3/content")).unwrap();
        std::fs::write(root.join("v3/content/ghost.md"), b"never committed").unwrap();

        let found = validate(&obj).unwrap();
        assert!(
            found.iter().any(
                |v| matches!(v, Violation::UnexpectedVersionDirectory { name } if name == "v3")
            ),
            "{found:?}"
        );
    }

    /// Rewrite the object's inventory (and its sidecar) with `edit` applied,
    /// as another tool might have written it, and open it again.
    fn rewritten(root: &std::path::Path, edit: impl FnOnce(&mut Inventory)) -> Object<StdFs> {
        let mut inventory =
            Inventory::parse(&std::fs::read(root.join(INVENTORY)).unwrap()).unwrap();
        edit(&mut inventory);
        let json = inventory.to_json().unwrap();
        let digest = DigestAlgorithm::Sha256.digest(json.as_bytes()).unwrap();
        std::fs::write(root.join(INVENTORY), &json).unwrap();
        std::fs::write(
            root.join("inventory.json.sha256"),
            format!("{digest}  {INVENTORY}\n"),
        )
        .unwrap();
        Object::open(StdFs, root).unwrap()
    }

    #[test]
    fn a_state_holding_a_path_and_a_path_beneath_it_is_reported() {
        let root = tmp("logical-conflict");
        let obj = committed(&root);
        let v2 = VersionNum::new(2);
        let digest = obj.inventory().versions[&v2].files()["a.md"].clone();
        let obj = rewritten(&root, |inventory| {
            let state = &mut inventory.versions.get_mut(&v2).unwrap().state;
            state.get_mut(&digest).unwrap().push("a.md/b".into());
        });

        let found = validate(&obj).unwrap();
        assert!(
            found.contains(&Violation::LogicalPathConflict {
                version: v2,
                path: "a.md".into(),
                other: "a.md/b".into(),
            }),
            "{found:?}"
        );
    }

    #[test]
    fn a_state_listing_one_path_under_two_digests_is_reported() {
        // What a commit of `a` twice used to write: each digest names `a`.
        let root = tmp("logical-twice");
        let obj = committed(&root);
        let v2 = VersionNum::new(2);
        let v1_digest = obj.inventory().versions[&VersionNum::FIRST].files()["a.md"].clone();
        let obj = rewritten(&root, |inventory| {
            let state = &mut inventory.versions.get_mut(&v2).unwrap().state;
            state.insert(v1_digest, vec!["a.md".into()]);
        });

        let found = validate(&obj).unwrap();
        assert!(
            found.contains(&Violation::LogicalPathConflict {
                version: v2,
                path: "a.md".into(),
                other: "a.md".into(),
            }),
            "{found:?}"
        );
    }

    #[test]
    fn a_manifest_whose_content_paths_conflict_is_reported() {
        let root = tmp("content-conflict");
        let obj = committed(&root);
        let digest = obj.inventory().manifest.keys().next().unwrap().clone();
        let obj = rewritten(&root, |inventory| {
            let paths = inventory.manifest.get_mut(&digest).unwrap();
            let beneath = format!("{}/b", paths[0]);
            paths.push(beneath);
        });

        let found = validate(&obj).unwrap();
        assert!(
            found.iter().any(
                |v| matches!(v, Violation::ContentPathConflict { path, other }
                    if *other == format!("{path}/b"))
            ),
            "{found:?}"
        );
    }

    /// Rewrite one version directory's inventory with `edit` applied and its
    /// sidecar resealed, so that only what the edit says is wrong with it.
    fn resealed(root: &std::path::Path, version: &str, edit: impl FnOnce(&mut Inventory)) {
        let dir = root.join(version);
        let mut inventory = Inventory::parse(&std::fs::read(dir.join(INVENTORY)).unwrap()).unwrap();
        edit(&mut inventory);
        let json = inventory.to_json().unwrap();
        let digest = DigestAlgorithm::Sha256.digest(json.as_bytes()).unwrap();
        std::fs::write(dir.join(INVENTORY), &json).unwrap();
        std::fs::write(
            dir.join("inventory.json.sha256"),
            format!("{digest}  {INVENTORY}\n"),
        )
        .unwrap();
    }

    #[test]
    fn a_corrupt_head_inventory_is_reported_three_ways() {
        // The task's repro: junk after the JSON, the sidecar left alone.
        let root = tmp("junk-head");
        let obj = committed(&root);
        let v2 = VersionNum::new(2);
        let mut json = std::fs::read(root.join("v2").join(INVENTORY)).unwrap();
        json.extend_from_slice(b"junk\n");
        std::fs::write(root.join("v2").join(INVENTORY), json).unwrap();

        let found = validate(&obj).unwrap();
        assert!(
            found.contains(&Violation::HeadInventoryDiffers { version: v2 }),
            "{found:?}"
        );
        assert!(
            found.iter().any(
                |v| matches!(v, Violation::VersionSidecarMismatch { version, .. } if *version == v2)
            ),
            "{found:?}"
        );
        assert!(
            found.iter().any(
                |v| matches!(v, Violation::VersionInventoryUnreadable { version, .. } if *version == v2)
            ),
            "{found:?}"
        );
    }

    #[test]
    fn a_head_inventory_other_than_the_root_is_reported() {
        let root = tmp("head-differs");
        let obj = committed(&root);
        let v2 = VersionNum::new(2);
        resealed(&root, "v2", |inventory| {
            inventory.versions.get_mut(&v2).unwrap().message = Some("changed".into());
        });

        assert_eq!(
            validate(&obj).unwrap(),
            vec![Violation::HeadInventoryDiffers { version: v2 }]
        );
    }

    #[test]
    fn a_prior_inventory_whose_state_renamed_a_path_is_reported() {
        let root = tmp("prior-state");
        let obj = committed(&root);
        let v1 = VersionNum::FIRST;
        resealed(&root, "v1", |inventory| {
            let state = &mut inventory.versions.get_mut(&v1).unwrap().state;
            for paths in state.values_mut() {
                *paths = vec!["renamed.md".to_string()];
            }
        });

        let differs = |logical_path: &str| Violation::VersionStateDiffers {
            inventory: v1,
            version: v1,
            logical_path: logical_path.into(),
        };
        assert_eq!(
            validate(&obj).unwrap(),
            vec![differs("a.md"), differs("renamed.md")]
        );
    }

    #[test]
    fn a_prior_inventory_giving_a_path_other_bytes_is_reported() {
        let root = tmp("prior-digest");
        let obj = committed(&root);
        let v1 = VersionNum::FIRST;
        let other = DigestAlgorithm::Sha256.digest(b"other").unwrap();
        resealed(&root, "v1", |inventory| {
            let old = inventory.versions[&v1].files()["a.md"].clone();
            let paths = inventory.manifest.remove(&old).unwrap();
            inventory.manifest.insert(other.clone(), paths);
            let state = &mut inventory.versions.get_mut(&v1).unwrap().state;
            let paths = state.remove(&old).unwrap();
            state.insert(other.clone(), paths);
        });

        assert_eq!(
            validate(&obj).unwrap(),
            vec![Violation::VersionStateDiffers {
                inventory: v1,
                version: v1,
                logical_path: "a.md".into(),
            }]
        );
    }

    #[test]
    fn a_prior_inventory_under_another_algorithm_is_compared_through_content_paths() {
        // Its digests mean nothing beside the root's, so the content paths they
        // lead to are what must agree.
        let root = tmp("algorithm-change");
        let obj = committed(&root);
        let v1 = VersionNum::FIRST;
        let sha512 = Digest::parse(&"a".repeat(128)).unwrap();
        let rekeyed = |inventory: &mut Inventory| {
            inventory.digest_algorithm = DigestAlgorithm::Sha512;
            let old = inventory.versions[&v1].files()["a.md"].clone();
            let paths = inventory.manifest.remove(&old).unwrap();
            inventory.manifest.insert(sha512.clone(), paths);
            let state = &mut inventory.versions.get_mut(&v1).unwrap().state;
            let paths = state.remove(&old).unwrap();
            state.insert(sha512.clone(), paths);
        };
        let write = |inventory: &Inventory| {
            let dir = root.join("v1");
            std::fs::write(dir.join(INVENTORY), inventory.to_json().unwrap()).unwrap();
            let _ = std::fs::remove_file(dir.join("inventory.json.sha256"));
            std::fs::write(dir.join("inventory.json.sha512"), "unchecked\n").unwrap();
        };
        let mut prior =
            Inventory::parse(&std::fs::read(root.join("v1/inventory.json")).unwrap()).unwrap();
        rekeyed(&mut prior);
        write(&prior);
        assert_eq!(
            validate(&obj).unwrap(),
            vec![],
            "same content, other digest"
        );

        let v2_content = obj
            .inventory()
            .content_path(obj.inventory().versions[&VersionNum::new(2)].files()["a.md"]);
        let elsewhere = format!("{}-not", v2_content.unwrap());
        prior.manifest.insert(sha512.clone(), vec![elsewhere]);
        write(&prior);
        assert_eq!(
            validate(&obj).unwrap(),
            vec![Violation::VersionStateDiffers {
                inventory: v1,
                version: v1,
                logical_path: "a.md".into(),
            }]
        );
    }

    #[test]
    fn a_prior_inventory_recording_a_version_the_root_lacks_is_reported() {
        let root = tmp("prior-extra");
        let obj = committed(&root);
        let v9 = VersionNum::new(9);
        resealed(&root, "v1", |inventory| {
            let block = inventory.versions[&VersionNum::FIRST].clone();
            inventory.versions.insert(v9, block);
        });

        assert_eq!(
            validate(&obj).unwrap(),
            vec![Violation::VersionUnknownToRoot {
                inventory: VersionNum::FIRST,
                version: v9,
            }]
        );
    }

    #[test]
    fn a_prior_inventory_with_another_id_is_reported() {
        let root = tmp("prior-id");
        let obj = committed(&root);
        resealed(&root, "v1", |inventory| {
            inventory.id = "urn:example:other".into();
        });

        assert_eq!(
            validate(&obj).unwrap(),
            vec![Violation::VersionIdDiffers {
                version: VersionNum::FIRST,
                id: "urn:example:other".into(),
                root_id: "urn:example:v".into(),
            }]
        );
    }

    #[test]
    fn a_version_inventory_with_no_sidecar_is_reported() {
        let root = tmp("version-sidecar");
        let obj = committed(&root);
        let v2 = VersionNum::new(2);
        std::fs::remove_file(root.join("v2/inventory.json.sha256")).unwrap();

        assert_eq!(
            validate(&obj).unwrap(),
            vec![Violation::VersionSidecarMissing {
                version: v2,
                name: "inventory.json.sha256".into(),
            }]
        );
    }

    #[test]
    fn a_spec_version_moving_back_is_reported_and_moving_forward_is_not() {
        const OLD: &str = "https://ocfl.io/1.0/spec/#inventory";
        let root = tmp("spec-back");
        let mut obj = committed(&root);
        obj.commit(
            [("a.md".to_string(), b"three".to_vec())],
            VersionMeta::at("2026-07-27T11:00:00Z"),
        )
        .unwrap();

        resealed(&root, "v1", |inventory| inventory.type_uri = OLD.into());
        assert_eq!(
            validate(&obj).unwrap(),
            vec![],
            "1.0 then 1.1 moves forward"
        );

        resealed(&root, "v2", |inventory| inventory.type_uri = OLD.into());
        resealed(&root, "v1", |inventory| {
            inventory.type_uri = crate::inventory::INVENTORY_TYPE.into();
        });
        assert_eq!(
            validate(&obj).unwrap(),
            vec![Violation::SpecVersionMovedBack {
                version: VersionNum::new(2),
                type_uri: OLD.into(),
                earlier: VersionNum::FIRST,
                earlier_type_uri: crate::inventory::INVENTORY_TYPE.into(),
            }]
        );
    }

    #[test]
    fn a_root_older_than_the_last_version_inventory_is_reported() {
        // With no inventory in the head's directory, the root speaks for it.
        const NEWER: &str = "https://ocfl.io/1.2/spec/#inventory";
        let root = tmp("spec-root");
        let obj = committed(&root);
        std::fs::remove_file(root.join("v2").join(INVENTORY)).unwrap();
        std::fs::remove_file(root.join("v2/inventory.json.sha256")).unwrap();
        resealed(&root, "v1", |inventory| inventory.type_uri = NEWER.into());

        assert_eq!(
            validate(&obj).unwrap(),
            vec![Violation::SpecVersionMovedBack {
                version: VersionNum::new(2),
                type_uri: crate::inventory::INVENTORY_TYPE.into(),
                earlier: VersionNum::FIRST,
                earlier_type_uri: NEWER.into(),
            }]
        );
    }

    #[test]
    fn an_object_with_no_inventory_in_a_version_directory_validates_clean() {
        // The spec has a version directory's inventory as SHOULD, not MUST.
        let root = tmp("no-version-inventories");
        let mut obj = Object::create(StdFs, &root, "urn:example:v", DigestAlgorithm::Sha256)
            .with_version_inventories(false);
        obj.commit(
            [("a.md".to_string(), b"one".to_vec())],
            VersionMeta::at("2026-07-27T09:00:00Z"),
        )
        .unwrap();
        assert!(!root.join("v1").join(INVENTORY).exists());

        assert_eq!(validate(&obj).unwrap(), vec![]);
        assert_eq!(validate_deep(&obj).unwrap(), vec![]);
    }

    #[test]
    fn summarize_counts_by_kind() {
        let counts = summarize(&[
            Violation::SidecarMissing { name: "a".into() },
            Violation::SidecarMissing { name: "b".into() },
        ]);
        assert_eq!(counts["sidecar-missing"], 2);
    }
}
