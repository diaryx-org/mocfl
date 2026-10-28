---
title: Commit accepts logical paths that conflict
part_of: '[Tasks](/docs/tasks/tasks.md)'
status: open
description: Object::commit takes one logical path twice, or a path and a path beneath it; the first writes an object its own deep validation calls corrupt, the second fails half-written or records an inventory OCFL forbids
created: 2026-10-28
updated: 2026-10-28
---

# Commit accepts logical paths that conflict

OCFL 1.1 says that within a version, logical paths must be unique and
non-conflicting: one file's logical path cannot be the start of another's
(E095). The same holds for content paths within an inventory (E101).
`Object::commit` checks each logical path on its own with `check_path` and
never checks them against each other. Each of the three cases below gets
through.

## Repro

Each case runs against a fresh root, with `DigestAlgorithm::Sha256` and
`VersionMeta::at("2026-10-28T00:00:00Z")`.

**One path twice, with different bytes.** The commit returns `Ok(v1)`, and
the object is corrupt:

```rust
object.commit([("a".into(), b"x".to_vec()), ("a".into(), b"y".to_vec())], meta)?;
```

Both digests are new, so each gets the content path `v1/content/a`. The
manifest lists that path under both, the state lists `a` under both, and the
second write replaces the first. `validate` finds nothing. `validate_deep`
reports `ContentDigestMismatch` for `v1/content/a`, and the bytes of `x` are
gone.

**A path and a path beneath it, in one commit.** The commit fails with
`Io(AlreadyExists)` after it has written `v1/content/a`:

```rust
object.commit([("a".into(), b"x".to_vec()), ("a/b".into(), b"y".to_vec())], meta)?;
```

`commit` stages in memory so that "a bad logical path fails before it has
written half a version", but this path is only refused by the filesystem, at
`create_dir_all("v1/content/a")`. So `v1/content/a` stays on disk. There is
no declaration and no inventory beside it.

**The same pair across versions.** v1 holds `a`. v2 commits `a` with the
same bytes, plus `a/b`. The commit returns `Ok(v2)`, and `validate` finds
nothing:

```rust
object.commit([("a".into(), b"x".to_vec())], meta)?;
object.commit([("a".into(), b"x".to_vec()), ("a/b".into(), b"y".to_vec())], meta2)?;
```

`a` is already in the manifest, so the new version writes only
`v2/content/a/b`, and the filesystem never sees a conflict. v2's state is
one that E095 forbids, and no filesystem can hold it, since `a` would have
to be both a file and a directory.

`mocfl commit --from <dir>` cannot produce any of these, because a directory
walk yields each path once and a name cannot be both a file and a directory.
A library caller passing its own list can.

## Done when

`commit` refuses a version whose logical paths repeat or where one is a
prefix of another, before anything is written, with an error naming both
paths. `validate` reports such a state, and a manifest whose content paths
conflict, when it reads one from an object some other tool wrote. The three
cases above are tests.
