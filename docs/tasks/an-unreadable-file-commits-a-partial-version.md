---
title: An unreadable file commits a partial version
part_of: '[Tasks](/docs/tasks/tasks.md)'
status: open
description: mocfl commit stops reading at the first file it cannot read and still lands a version, so every path after it is deleted from the head while the command reports failure
created: 2026-11-06
---

# An unreadable file commits a partial version

`commit` in `mocfl-cli/src/main.rs` reads the working tree lazily, through
`map_while`, and hands the iterator to `Object::commit`. A file that cannot be
read ends the iterator early, but nothing stops the commit: `Object::commit`
takes what it was given as the version's complete state, and "a path omitted
here is deleted as of this version". The CLI only looks at the failure after
the version has landed.

So a failed `mocfl commit` is not a commit that did not happen. The new head
holds the files sorted before the unreadable one and none from it onward, and
the error says only that a file could not be read.

## Repro

```console
$ mkdir w && for f in a b c d; do echo $f > w/$f.txt; done
$ mocfl commit o --from w --id urn:x -m v1 --user u
$ chmod 000 w/b.txt
$ mocfl commit o --from w -m v2 --user u      # exit 1
mocfl: could not read w/b.txt: Permission denied (os error 13)
$ mocfl ls o
a.txt
$ mocfl diff o --from v1 --to v2
D	b.txt
D	c.txt
D	d.txt
$ mocfl validate o --deep                     # exit 0
valid	urn:x	2 version(s), content verified
```

The earlier bytes are still in `v1`, so nothing is destroyed, but the head is
wrong and nothing says so. Three things make it worse:

- **Running it again adds another version.** With the file still unreadable,
  the same command fails the same way and lands `v3`.
- **It gets past the `--allow-empty` guard.** The guard looks at the walk's
  entries, not at what was read. If the unreadable file sorts first, the
  version is empty, and every file leaves the head with no `--allow-empty`
  given.
- **A first commit makes a partial object.** With no object at the path, the
  same failure writes one whose `v1` holds only the files before the
  unreadable one.

## Done when

A `mocfl commit` that cannot read a file exits non-zero and leaves the object
as it was: no new version, no new version directory, and for a first commit
no object. A test in `mocfl-cli` covers an object that already has a version
and one that does not. A commit of a large tree still holds no more in memory
than it does today.
