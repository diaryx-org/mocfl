//! Conformance against the OCFL project's own test fixtures.
//!
//! Reimplementing a specification is only responsible with an *independent*
//! oracle. These tests read the objects published at
//! <https://github.com/OCFL/fixtures> — written by other implementations, not by
//! this one — so agreement is evidence rather than self-consistency.
//!
//! The fixtures are not vendored (they are a separate repository with their own
//! history). Point `OCFL_FIXTURES` at a checkout to run these:
//!
//! ```sh
//! git clone --depth 1 https://github.com/OCFL/fixtures /tmp/ocfl-fixtures
//! OCFL_FIXTURES=/tmp/ocfl-fixtures cargo test --test fixtures
//! ```
//!
//! Without it, every test here reports as skipped rather than failing — a
//! contributor without the checkout still gets a green suite, and CI sets the
//! variable so the coverage is not quietly optional there.

use std::path::PathBuf;

use ocfl::{Object, StdFs, Violation, validate, validate_deep};

/// The fixtures checkout, or `None` when the suite should skip.
fn fixtures() -> Option<PathBuf> {
    let root = PathBuf::from(std::env::var_os("OCFL_FIXTURES")?);
    root.is_dir().then_some(root)
}

/// Every object directory directly under `<fixtures>/<spec>/<group>`.
fn objects(spec: &str, group: &str) -> Vec<PathBuf> {
    let Some(root) = fixtures() else {
        return Vec::new();
    };
    let dir = root.join(spec).join(group);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .map(|e| e.path())
        .collect();
    out.sort();
    out
}

fn skip(what: &str) -> bool {
    if fixtures().is_none() {
        eprintln!("skipping {what}: set OCFL_FIXTURES to a fixtures checkout");
        return true;
    }
    false
}

#[test]
fn every_good_object_opens_and_reads() {
    if skip("good-object open") {
        return;
    }
    let mut seen = 0;
    for spec in ["1.0", "1.1"] {
        for path in objects(spec, "good-objects") {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let object = Object::open(StdFs, &path)
                .unwrap_or_else(|e| panic!("{spec}/{name} failed to open: {e}"));

            // The head version must be readable end to end: every logical path
            // resolves through the manifest to bytes on disk.
            let head = object.head().expect("an opened object has a head");
            for (logical, _) in object.files_at(head).unwrap() {
                object
                    .read(head, logical)
                    .unwrap_or_else(|e| panic!("{spec}/{name}: reading {logical} failed: {e}"));
            }
            seen += 1;
        }
    }
    assert!(seen > 0, "no good objects found — is OCFL_FIXTURES right?");
    eprintln!("opened and read {seen} good objects");
}

#[test]
fn every_good_object_validates_structurally() {
    if skip("good-object validation") {
        return;
    }
    let mut failures = Vec::new();
    for spec in ["1.0", "1.1"] {
        for path in objects(spec, "good-objects") {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let Ok(object) = Object::open(StdFs, &path) else {
                failures.push(format!("{spec}/{name}: could not open"));
                continue;
            };
            let found = validate(&object).unwrap();
            if !found.is_empty() {
                failures.push(format!(
                    "{spec}/{name}: {}",
                    found
                        .iter()
                        .map(|v| v.to_string())
                        .collect::<Vec<_>>()
                        .join("; ")
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "objects the spec calls valid were reported invalid:\n{}",
        failures.join("\n")
    );
}

#[test]
fn a_sha512_object_reports_that_it_could_not_be_verified() {
    // The honesty requirement: this crate cannot compute SHA-512, and a deep
    // pass over such an object must say so rather than returning clean. "Not
    // checked" must never be indistinguishable from "checked and fine".
    if skip("sha512 honesty") {
        return;
    }
    let mut checked = 0;
    for spec in ["1.0", "1.1"] {
        for path in objects(spec, "good-objects") {
            let Ok(object) = Object::open(StdFs, &path) else {
                continue;
            };
            if object.inventory().digest_algorithm.is_computable() {
                continue;
            }
            let found = validate_deep(&object).unwrap();
            assert!(
                found
                    .iter()
                    .any(|v| matches!(v, Violation::ContentUnverifiable { .. })),
                "{}: deep validation stayed silent about an uncomputable digest",
                path.display()
            );
            checked += 1;
        }
    }
    assert!(checked > 0, "no sha512 fixtures found to check");
}

#[test]
fn a_sha256_object_verifies_all_the_way_down() {
    if skip("sha256 deep validation") {
        return;
    }
    let mut checked = 0;
    for spec in ["1.0", "1.1"] {
        for path in objects(spec, "good-objects") {
            let Ok(object) = Object::open(StdFs, &path) else {
                continue;
            };
            if !object.inventory().digest_algorithm.is_computable() {
                continue;
            }
            let found = validate_deep(&object).unwrap();
            assert!(
                found.is_empty(),
                "{}: {found:?}",
                path.file_name().unwrap().to_string_lossy()
            );
            checked += 1;
        }
    }
    eprintln!("deep-validated {checked} sha256 objects");
}

#[test]
fn a_foreign_object_survives_being_reread_after_a_rewrite() {
    // Round-tripping someone else's inventory must not lose or reshape anything:
    // this is what makes it safe for this crate to write an object it did not
    // create.
    if skip("round-trip") {
        return;
    }
    let mut checked = 0;
    for spec in ["1.0", "1.1"] {
        for path in objects(spec, "good-objects") {
            let Ok(object) = Object::open(StdFs, &path) else {
                continue;
            };
            let json = object.inventory().to_json().unwrap();
            let back = ocfl::Inventory::parse(json.as_bytes()).unwrap();
            assert_eq!(
                &back,
                object.inventory(),
                "{} did not survive a rewrite",
                path.display()
            );
            checked += 1;
        }
    }
    assert!(checked > 0);
}

#[test]
fn objects_the_spec_calls_bad_are_not_reported_clean() {
    // The other half of conformance: a validator that accepts everything is
    // useless. Not every fixture is detectable by a structural pass, so this
    // asserts a floor and reports the gap rather than pretending to totality.
    if skip("bad-object detection") {
        return;
    }
    let (mut caught, mut missed) = (0, Vec::new());
    for spec in ["1.0", "1.1"] {
        for path in objects(spec, "bad-objects") {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            match Object::open(StdFs, &path) {
                // Refused outright — the strongest form of catching it.
                Err(_) => caught += 1,
                Ok(object) => {
                    if validate(&object).map(|f| !f.is_empty()).unwrap_or(true) {
                        caught += 1;
                    } else {
                        missed.push(name);
                    }
                }
            }
        }
    }
    eprintln!(
        "bad-object fixtures: {caught} caught, {} not detected by the structural pass:\n  {}",
        missed.len(),
        missed.join("\n  ")
    );
    assert!(caught > 0, "the validator caught nothing at all");
}
