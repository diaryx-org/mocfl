//! Validation — is what is on disk actually a conformant object?
//!
//! Reading an object answers "what does this say?"; validation answers "should I
//! believe it?" They are separate passes because they have separate audiences: a
//! read failure is a problem for the calling program, while a validation finding
//! is a report about someone's archive — something to list, show, and act on.
//!
//! ## Two depths
//!
//! - **Structural** ([`validate`]) reads only the inventory and the directory
//!   listing: is the declaration right, are versions contiguous, does every state
//!   digest resolve, is every content file where the manifest says. Cheap enough
//!   to run on every open.
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

use crate::digest::Digest;
use crate::error::Result;
use crate::fs::{Storage, join};
use crate::inventory::VersionNum;
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
    let sidecar_name = format!("{INVENTORY}.{algorithm}");
    match fs.read(&root.join(&sidecar_name)) {
        Err(_) => found.push(Violation::SidecarMissing { name: sidecar_name }),
        Ok(bytes) => {
            let text = String::from_utf8_lossy(&bytes);
            let recorded = text.split_whitespace().next().unwrap_or("").to_string();
            if algorithm.is_computable() {
                let actual = algorithm.digest(&fs.read(&root.join(INVENTORY))?)?;
                if !recorded.eq_ignore_ascii_case(actual.as_str()) {
                    found.push(Violation::SidecarMismatch {
                        recorded,
                        actual: actual.to_string(),
                    });
                }
            }
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
        };
        *counts.entry(key).or_insert(0) += 1;
    }
    counts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::digest::DigestAlgorithm;
    use crate::fs::StdFs;
    use crate::object::VersionMeta;
    use std::path::PathBuf;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ocfl-validate-{name}-{}", std::process::id()));
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

    #[test]
    fn summarize_counts_by_kind() {
        let counts = summarize(&[
            Violation::SidecarMissing { name: "a".into() },
            Violation::SidecarMissing { name: "b".into() },
        ]);
        assert_eq!(counts["sidecar-missing"], 2);
    }
}
