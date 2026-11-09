---
title: A version inventory can claim a later head
part_of: '[Tasks](/docs/tasks/tasks.md)'
status: open
description: validate never asks whether the inventory in a version directory is that version's, so `v1/inventory.json` holding the head's inventory, `head` v2 and all, passes as valid
created: 2026-11-09
updated: 2026-11-09
---

# A version inventory can claim a later head

`check_version_inventories` in `mocfl/src/validate.rs` reads the inventory in
each version directory and holds it to the root: its sidecar, its `id`, its
`type` against the version before it, and every `version` block it records
against the root's. It never compares the inventory's own `head` with the
directory it sits in. An inventory in `v1/` that says `head: v2`, and records
a `v2` block that matches the root's, passes every one of those checks.

OCFL 1.1 has a version directory's inventory describe "all content for
versions up to and including that particular version" (§3.7, W010), and an
inventory's `head` is "the version directory name with the highest version
number" (§3.5.1, E040). An inventory in `v1/` with a `v2` block covers a
version after its own, and its `head` names a version directory other than
the one it is in. The spec's fixtures call this object bad:
`E040_wrong_version_in_version_dir`, in 1.0 and 1.1, where `v2/inventory.json`
is the head's, `head: v3` and all. The fixtures test lists both among the
bad objects the structural pass does not detect.

## Repro

```console
$ printf 'one\n' > work/a.txt && mocfl commit obj --from work --id urn:example:a -m one
$ printf 'two\n' > work/a.txt && mocfl commit obj --from work -m two
$ cp obj/inventory.json obj/inventory.json.sha256 obj/v1/
$ grep '"head"' obj/v1/inventory.json
  "head": "v2",
$ mocfl validate obj
valid	urn:example:a	2 version(s)
$ echo $?
0
```

`validate_deep` returns nothing for it either.

## Done when

`validate` reports a version directory whose inventory's `head` is not that
directory's version, as a new `Violation` with a test in
`mocfl/src/validate.rs` built as the repro is. `E040_wrong_version_in_version_dir`
joins the list in `objects_whose_version_inventories_disagree_are_caught` in
`mocfl/tests/fixtures.rs`, and the README's count of detected `bad-objects`
moves with it.
