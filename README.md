---
title: mocfl
author: adammharris
created: 2026-07-27
---

# mocfl

*minimal OCFL* — a dependency-thin Rust implementation of the [Oxford Common File Layout](https://ocfl.io/1.1.0/spec/) **object**, named in the spirit of [`moid`](https://github.com/diaryx-org/moid) (*minimal opaque ID*). The `m` is a promise about scope: this implements one part of the standard, deliberately and completely, and says plainly what it leaves out.

OCFL is a specification for storing versioned digital objects on an ordinary filesystem, so that a repository can be rebuilt from the files alone — no database, no application, no proprietary index. An object is a directory that declares what it is, carries a human-readable JSON inventory of every version it has ever had, and stores content addressed by digest.

## Layout

- **`mocfl/`** — the library.
- **`mocfl-cli/`** — a thin command-line companion (the installed binary is `mocfl`).

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
use mocfl::{DigestAlgorithm, Object, StdFs, VersionMeta};

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

// History that follows renames, answered from the inventory alone — no content
// read, no similarity heuristics.
for entry in object.history("letter.md") {
    println!("{}  {}  {}", entry.version, entry.digest, entry.logical_path);
}
# Ok::<(), mocfl::Error>(())
```

Bytes already present under any earlier version are never rewritten, so an unchanged photo across a hundred versions is stored once, and a rename moves nothing at all.

## The CLI

```sh
cargo install --path mocfl-cli    # installs `mocfl`
```

The library is the product; the CLI is the way to *see* it. It adds nothing the library cannot do, with one deliberate exception — the **clock**. `mocfl` takes every timestamp from its caller so it stays deterministic and dependency-free; a command-line run is the caller that actually reads the wall clock.

```sh
mocfl init archive --id "ark:/99999/dxg6h4ncm" --user Adam   # or skip it — see below
mocfl commit archive --from vault -m "first capture" --user Adam
mocfl status archive --from vault      # what would a commit record right now?
mocfl log archive                      # versions, newest first
mocfl log archive --path letters/1943-05-scan.jpg   # one file's life, across renames
mocfl diff archive --from v1 --to v3
mocfl restore archive --to ./v1-copy --version v1
mocfl validate archive --deep          # exits 1 on findings
mocfl show archive                     # identity, versions, what dedup saved
```

### Creating an object

Two ways, because there are two moments people mean by "create":

- **`mocfl commit <object> --from <dir> --id <identifier>`** — create and fill in one step. The `--id` is only consulted when there is no object there yet.
- **`mocfl init <object> --id <identifier>`** — establish an empty archive now, fill it later.

`init` does not write an empty *directory*. An OCFL object must have at least one version, so it writes a real `v1` whose state is empty — valid OCFL, and byte-for-byte the shape of the spec's own `minimal_no_content` fixture, right down to having no `content/` directory:

```
archive/0=ocfl_object_1.1
archive/inventory.json
archive/inventory.json.sha256
archive/v1/inventory.json
archive/v1/inventory.json.sha256
```

The object is valid and checksummed the moment `init` returns. Its value is that "the archive was established" becomes its own dated, attributed event rather than being folded into whatever happened to be captured first.

### Committing

`commit` captures a directory whole, so a file it no longer contains is deleted as of that version — that is what makes a version's state complete rather than a delta. `.git`, `.DS_Store` and friends are skipped unless you pass `--include-all`.

Committing an *empty* directory over an object that holds files removes every one of them. That is a legitimate thing to want and also exactly what a mistyped `--from` looks like, so it requires `--allow-empty`. The guard is about destruction, not emptiness: an empty first version needs no flag, because it deletes nothing.

Two commands are worth running just for the intuition:

**`log --path`** follows renames *exactly*. Version control that stores by path has to infer moves after the fact by scoring content similarity — a flag with a threshold that still guesses wrong. OCFL keys state by digest, so a move is not an inference: the same digest is simply listed under a different name. The `logical_path` column shows the name the file had at each point.

```
v1  6cb38dd2…  letters/scan.jpg
v3  6cb38dd2…  letters/1943-05-scan.jpg
```

**`show`** prints stored bytes against logical bytes. The gap between them is everything deduplication saved, and it is the number that makes the model click.

## No clock, no agent of its own

A caller supplies `created` on every version and fills in `user` itself. The crate has no clock — determinism under test, and only the caller knows whether the right instant is "now" or the time of the event being recorded. `user` is what turns a version list into a provenance record rather than a byte log.

## Conformance

Reimplementing a specification is only responsible with an independent oracle. There are two, and both run in CI:

1. **The spec's own [test fixtures](https://github.com/OCFL/fixtures)** — objects written by other implementations. Every 1.0 and 1.1 good object must open, read end to end, validate clean, and survive a rewrite unchanged.

   ```sh
   git clone --depth 1 https://github.com/OCFL/fixtures /tmp/mocfl-fixtures
   OCFL_FIXTURES=/tmp/mocfl-fixtures cargo test
   ```

2. **`rocfl validate`** over objects this crate writes — two codebases sharing no lineage agreeing on the same bytes.

   ```sh
   MOCFL_INTEROP_OUT=/tmp/interop cargo test --test interop
   rocfl -r /tmp/interop validate
   ```

SHA-256 is pinned to the FIPS 180-4 known-answer vectors, so the hash is *checked*, not trusted.

### Known coverage gaps

The structural validator currently detects 45 of the 107 `bad-objects` fixtures. The rest need checks this crate does not yet make — duplicate digests within a manifest, non-unique logical paths, content outside the content directory, timestamp format rules, cross-inventory consistency between versions. `validate` is honest about being a floor rather than a complete validator, and the fixtures test reports exactly which fixtures go undetected rather than hiding the number.

Violations are named in prose rather than by the spec's `E###` codes. The codes are [published](https://github.com/OCFL/spec/blob/main/1.1/spec/validation-codes.md) and mapping onto them is worth doing — but a *wrong* code is worse than no code, since it sends a reader to the wrong clause, so the mapping will be done against that document rather than from memory.

## Status

Early. The object model, read/write, both validation depths, and the CLI work and are tested against the spec fixtures and an independent implementation. Unpublished; the API will move.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or <http://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or <http://opensource.org/licenses/MIT>)

at your option.
