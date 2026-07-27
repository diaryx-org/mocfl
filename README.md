---
title: ocfl
author: adammharris
created: 2026-07-27
---

# ocfl

A dependency-thin Rust implementation of the [Oxford Common File Layout](https://ocfl.io/1.1.0/spec/) **object**.

OCFL is a specification for storing versioned digital objects on an ordinary filesystem, so that a repository can be rebuilt from the files alone — no database, no application, no proprietary index. An object is a directory that declares what it is, carries a human-readable JSON inventory of every version it has ever had, and stores content addressed by digest.

```
object-root/
├── 0=ocfl_object_1.1        what this directory is
├── inventory.json           every version, every digest
├── inventory.json.sha256    fixity over the inventory itself
├── v1/
│   ├── inventory.json
│   ├── inventory.json.sha256
│   └── content/…            the bytes first seen at v1
└── v2/…
```

## Why this exists alongside `rocfl`

[`rocfl`](https://github.com/pwinckles/rocfl) is the established Rust OCFL tool and is more complete than this crate: storage roots, layout extensions, S3, staging, locks. It is also repository-scale, carrying around forty dependencies including a deprecated AWS SDK, and its last release was 2022.

This crate serves the other end: **one object, embedded in an application, on a device.** It has exactly one dependency ([`fig`](https://github.com/diaryx-org/fig), for JSON), computes its own SHA-256, and needs no toolchain beyond Rust.

Where the two overlap they should agree exactly — which is why `rocfl` is used as this crate's independent validator in CI rather than treated as a competitor.

## Scope

**In:** the OCFL *object* — declaration, inventory read and write, versions, content addressing and deduplication, logical-path history, structural and deep validation.

**Out, deliberately:** storage roots and their layout extensions (sharding exists to spread hundreds of thousands of objects across a filesystem; a caller with one object per collection has no such problem), S3 and other remote backends, staging, locking, and concurrent-writer protection. A repository layer can be built on top without changing anything here.

**Not yet:** SHA-512 computation. The algorithm is *recognized*, so an object addressed with it opens and validates structurally, but its content cannot be verified and `validate_deep` reports `ContentUnverifiable` rather than passing it in silence.

## Use

```rust
use ocfl::{DigestAlgorithm, Object, StdFs, VersionMeta};

let mut object = Object::create(
    StdFs,
    "/tmp/letters",
    "urn:example:letters",
    DigestAlgorithm::Sha256,
);

// A version's file list is its *complete* state, not a delta: a path omitted
// here is deleted as of this version.
let v1 = object.commit(
    [("letter.md".to_string(), b"Dear Mother,".to_vec())],
    VersionMeta::at("2026-07-27T09:14:00Z")
        .message("first capture")
        .user("Adam", Some("mailto:adam@example.org".into())),
)?;

assert_eq!(object.read(v1, "letter.md")?, b"Dear Mother,");

// Rename-robust history, answered from the inventory alone — no content read,
// no similarity heuristics.
for (version, digest) in object.history("letter.md") {
    println!("{version}  {digest}");
}
# Ok::<(), ocfl::Error>(())
```

Bytes already present under any earlier version are never rewritten, so an unchanged photo across a hundred versions is stored once, and a rename moves nothing at all.

## No clock, no agent of its own

A caller supplies `created` on every version and fills in `user` itself. The crate has no clock — determinism under test, and only the caller knows whether the right instant is "now" or the time of the event being recorded. `user` is what turns a version list into a provenance record rather than a byte log.

## Conformance

Reimplementing a specification is only responsible with an independent oracle. There are two, and both run in CI:

1. **The spec's own [test fixtures](https://github.com/OCFL/fixtures)** — objects written by other implementations. Every 1.0 and 1.1 good object must open, read end to end, validate clean, and survive a rewrite unchanged.

   ```sh
   git clone --depth 1 https://github.com/OCFL/fixtures /tmp/ocfl-fixtures
   OCFL_FIXTURES=/tmp/ocfl-fixtures cargo test
   ```

2. **`rocfl validate`** over objects this crate writes — two codebases sharing no lineage agreeing on the same bytes.

   ```sh
   OCFL_INTEROP_OUT=/tmp/interop cargo test --test interop
   rocfl -r /tmp/interop validate
   ```

SHA-256 is pinned to the FIPS 180-4 known-answer vectors, so the hash is *checked*, not trusted.

### Known coverage gaps

The structural validator currently detects 45 of the 107 `bad-objects` fixtures. The rest need checks this crate does not yet make — duplicate digests within a manifest, non-unique logical paths, content outside the content directory, timestamp format rules, cross-inventory consistency between versions. `validate` is honest about being a floor rather than a complete validator, and the fixtures test reports exactly which fixtures go undetected rather than hiding the number.

Violations are named in prose rather than by the spec's `E###` codes. The codes are [published](https://github.com/OCFL/spec/blob/main/1.1/spec/validation-codes.md) and mapping onto them is worth doing — but a *wrong* code is worse than no code, since it sends a reader to the wrong clause, so the mapping will be done against that document rather than from memory.

## Status

Early. The object model, read/write, and both validation depths work and are tested against the spec's fixtures and an independent implementation. Unpublished; the API will move.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or <http://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or <http://opensource.org/licenses/MIT>)

at your option.
