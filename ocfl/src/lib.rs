//! A dependency-thin implementation of the **Oxford Common File Layout**
//! ([OCFL](https://ocfl.io/1.1.0/spec/)) object.
//!
//! OCFL is a specification for storing versioned digital objects on an ordinary
//! filesystem so that a repository can be rebuilt from the files alone — no
//! database, no application, no proprietary index. An object is a directory
//! containing a declaration of what it is, a JSON inventory of every version it
//! has ever had, and content files addressed by digest.
//!
//! ```text
//! object-root/
//! ├── 0=ocfl_object_1.1        what this directory is
//! ├── inventory.json           every version, every digest
//! ├── inventory.json.sha256    fixity over the inventory itself
//! ├── v1/
//! │   ├── inventory.json
//! │   ├── inventory.json.sha256
//! │   └── content/…            the bytes first seen at v1
//! └── v2/…
//! ```
//!
//! ## Why this exists alongside `rocfl`
//!
//! [`rocfl`](https://github.com/pwinckles/rocfl) is the established Rust OCFL
//! tool and is more complete than this crate: storage roots, layout extensions,
//! S3, staging, locks. It is also a repository-scale application, carrying around
//! forty dependencies including a deprecated AWS SDK.
//!
//! This crate serves the other end: **one object, embedded in an application, on
//! a device**. It has exactly one dependency ([`fig`], for JSON), computes its own
//! SHA-256, and compiles without a toolchain beyond Rust. Where the two overlap
//! they should agree exactly — which is why `rocfl` is the intended independent
//! validator for objects this crate writes, rather than a competitor to route
//! around.
//!
//! ## Scope
//!
//! **In:** the OCFL *object* — declaration, inventory read and write, versions,
//! content addressing and deduplication, logical-path history, structural and
//! deep validation.
//!
//! **Out, deliberately:** storage roots and their layout extensions (sharding
//! exists for repositories of many objects), S3 and other remote backends,
//! staging, locking, and concurrent-writer protection. Say what a library does
//! not do, and it stays small enough to trust.
//!
//! **Not yet:** SHA-512 computation. The algorithm is *recognized*, so an object
//! addressed with it parses and validates structurally, but its content cannot be
//! verified here and [`validate_deep`] says so rather than passing it silently.
//!
//! ## Example
//!
//! ```no_run
//! use ocfl::{DigestAlgorithm, Object, StdFs, VersionMeta};
//!
//! let mut object = Object::create(
//!     StdFs,
//!     "/tmp/my-object",
//!     "urn:example:letters",
//!     DigestAlgorithm::Sha256,
//! );
//!
//! // A version's file list is its *complete* state, not a delta.
//! let v1 = object.commit(
//!     [("letter.md".to_string(), b"Dear Mother,".to_vec())],
//!     VersionMeta::at("2026-07-27T09:14:00Z")
//!         .message("first capture")
//!         .user("Adam", Some("mailto:adam@example.org".into())),
//! )?;
//!
//! assert_eq!(object.read(v1, "letter.md")?, b"Dear Mother,");
//!
//! // Rename-robust history, answered from the inventory alone.
//! for (version, digest) in object.history("letter.md") {
//!     println!("{version}  {digest}");
//! }
//! # Ok::<(), ocfl::Error>(())
//! ```
//!
//! ## Timestamps and agents
//!
//! This crate has no clock. A caller supplies `created` on every version, for the
//! same reason its sister crates do: determinism under test, and only the caller
//! knows whether the right instant is "now" or the time of the event being
//! recorded. [`User`] is likewise the caller's to fill — it is the field that
//! turns a version list into a provenance record rather than a byte log.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod digest;
mod error;
mod fs;
mod inventory;
mod object;
mod validate;

pub use digest::{Digest, DigestAlgorithm};
pub use error::{Error, Result};
pub use fs::{StdFs, Storage};
pub use inventory::{
    DEFAULT_CONTENT_DIRECTORY, INVENTORY_TYPE, Inventory, User, Version, VersionNum, check_path,
};
pub use object::{
    DECLARATION, DECLARATION_PREFIX, INVENTORY, OCFL_VERSION, Object, SUPPORTED_VERSIONS,
    VersionMeta, declaration_name,
};
pub use validate::{Violation, summarize, validate, validate_deep};
