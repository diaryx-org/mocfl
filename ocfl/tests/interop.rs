//! Generate an object on disk for an external validator to inspect.
//!
//! `rocfl` is an independent OCFL implementation. Pointing it at an object this
//! crate wrote is the strongest conformance evidence available short of the
//! spec's own fixtures: two codebases that share no lineage agreeing on the
//! same bytes. Set `OCFL_INTEROP_OUT` to a directory to emit one.

use ocfl::{DigestAlgorithm, Object, StdFs, VersionMeta};

#[test]
fn emit_an_object_for_an_external_validator() {
    let Some(out) = std::env::var_os("OCFL_INTEROP_OUT") else {
        eprintln!("skipping: set OCFL_INTEROP_OUT to emit an object");
        return;
    };
    let root = std::path::PathBuf::from(out).join("interop-object");
    let _ = std::fs::remove_dir_all(&root);

    let mut object = Object::create(StdFs, &root, "urn:example:interop", DigestAlgorithm::Sha256);
    object
        .commit(
            [
                ("letter.md".to_string(), b"Dear Mother,\n".to_vec()),
                ("photos/stamp.png".to_string(), vec![0x89, 0x50, 0x4e, 0x47]),
            ],
            VersionMeta::at("2026-07-27T09:14:00Z")
                .message("first capture")
                .user("Adam", Some("mailto:adam@example.org".into())),
        )
        .unwrap();
    object
        .commit(
            [
                (
                    "letter.md".to_string(),
                    b"Dear Mother, we are well.\n".to_vec(),
                ),
                ("photos/stamp.png".to_string(), vec![0x89, 0x50, 0x4e, 0x47]),
            ],
            VersionMeta::at("2026-07-27T10:00:00Z").message("edited"),
        )
        .unwrap();
    // A rename plus a deletion, the two cases that move no bytes.
    object
        .commit(
            [(
                "letters/1943.md".to_string(),
                b"Dear Mother, we are well.\n".to_vec(),
            )],
            VersionMeta::at("2026-07-27T11:00:00Z").message("filed and pruned"),
        )
        .unwrap();

    eprintln!("wrote {}", root.display());
}
