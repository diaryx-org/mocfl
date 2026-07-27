//! Errors.
//!
//! Two things are deliberately kept apart here:
//!
//! - **[`Error`]** — this crate could not *do* what was asked: the bytes would
//!   not parse, the filesystem refused, an inventory contradicts itself so badly
//!   that no object can be constructed from it.
//! - **[`crate::Violation`]** — the object was read fine and *is not conformant*.
//!
//! That seam matters because they have different audiences. An `Error` is a bug
//! or an environment problem for the calling program to handle; a `Violation` is
//! a finding about someone's archive, to be reported, listed, and acted on — the
//! same distinction the sister crates draw between an error and a `Finding`.

use std::fmt;

use crate::digest::DigestAlgorithm;

/// The result type this crate returns.
pub type Result<T> = std::result::Result<T, Error>;

/// Something this crate could not do.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// The filesystem said no.
    Io(std::io::Error),
    /// `inventory.json` is not parseable as JSON at all.
    Json(String),
    /// The inventory parsed as JSON but is not an inventory: a required field is
    /// missing, or one holds a value of the wrong shape. Carries a human-readable
    /// path into the document (`versions.v1.state`) so the report names the spot.
    Inventory(String),
    /// A digest string that is not bare hex.
    MalformedDigest(String),
    /// A version directory name that is not `v` followed by a positive integer.
    MalformedVersion(String),
    /// The algorithm is one the spec permits, but this crate cannot compute it.
    /// Recognized, not supported — see [`DigestAlgorithm::is_computable`].
    UnsupportedDigest(DigestAlgorithm),
    /// A logical or content path the spec forbids (`.`, `..`, an empty segment,
    /// a leading or trailing slash, or a backslash).
    IllegalPath(String),
    /// The caller asked for a version, or a file within one, that is not there.
    NotFound(String),
    /// The caller asked to create an object where one already exists, or to
    /// commit a version that would contradict what is on disk.
    Conflict(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "{e}"),
            Self::Json(detail) => write!(f, "inventory.json is not valid JSON: {detail}"),
            Self::Inventory(detail) => write!(f, "inventory is malformed: {detail}"),
            Self::MalformedDigest(got) => {
                write!(f, "not a bare-hex digest: {got:?}")
            }
            Self::MalformedVersion(got) => {
                write!(f, "not an OCFL version name: {got:?}")
            }
            Self::UnsupportedDigest(alg) => write!(
                f,
                "digest algorithm `{alg}` is recognized but cannot be computed by this crate"
            ),
            Self::IllegalPath(path) => write!(f, "path is not allowed by the spec: {path:?}"),
            Self::NotFound(what) => write!(f, "not found: {what}"),
            Self::Conflict(detail) => write!(f, "{detail}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<fig::Error> for Error {
    fn from(e: fig::Error) -> Self {
        Self::Json(e.to_string())
    }
}
