---
title: Validate calls an object valid without reading its version inventories
part_of: '[Tasks](/docs/tasks/tasks.md)'
status: done
description: validate compares nothing with a version directory's own inventory, so a corrupt or disagreeing one passes as valid, content verified, where rocfl refuses it
created: 2026-11-05
updated: 2026-11-05
---

# Validate calls an object valid without reading its version inventories

**Status: done**, by `fix(validate): read each version directory's inventory`.
`validate` now reads the inventory in every version directory that holds one.
It reports a sidecar that is missing or disagrees, an inventory that does not
parse, a head inventory not byte for byte the root's, an `id` that differs, a
`type` older than the version before it, and a version block whose logical
state differs from the root's, compared through content paths where the digest
algorithm changed. The repro exits 1, and the eleven fixtures are caught. So
are `E011_E013_invalid_padded_head_version`, `E040_head_not_most_recent` and
`E060_E064_root_inventory_digest_mismatch`, two of each, which all disagree
with the root too, so the fixtures test reports 68 of 107.

`check` in `mocfl/src/validate.rs` reads the root inventory and its sidecar,
and never opens `vN/inventory.json` or its sidecar. An object whose version
inventories are corrupt, or say something other than the root does, gets the
verdict a sound object gets: `valid`, and with `--deep`, `content verified`.

The README knows part of this: `validate` is "a floor", and "cross-inventory
consistency between versions" is among the checks it does not yet make. What
it does not say is that the verdict line reads as though the check was made,
which `Violation::ContentUnverifiable` rules out for content: "not checked"
must never read as "checked and fine". Nothing tracked the work until this
task.

## Repro

```console
$ mkdir w && echo one > w/a.txt
$ mocfl commit o --from w --id urn:x -m v1 --user u
$ echo two > w/b.txt
$ mocfl commit o --from w -m v2 --user u
$ echo junk >> o/v2/inventory.json
$ mocfl validate o --deep
valid	urn:x	2 version(s), content verified
$ rocfl -r . validate --paths o
Object urn:x is invalid
  Errors:
    1. [E064] (v2) Inventory file must be identical to the root inventory
    2. [E060] (v2) Inventory does not match expected digest. …
```

rocfl 1.7.0, the validator CI uses, exits 2 there, and mocfl exits 0. The same
split, each on an otherwise valid object with the sidecar resealed where the
edit touched an inventory:

| What was done to it | rocfl | mocfl |
|---|---|---|
| `v2/inventory.json`'s `message` changed | E064 | valid |
| `v1/inventory.json`'s state renamed a path | E066 | valid |
| `v1/inventory.json`'s `id` changed | E110 | valid |
| `v2/inventory.json.sha256` deleted | E058 | valid |

The codes are rocfl's output. E058, E064 and E066 say the same in the
validation codes for OCFL 1.1, and spec section 3.7 holds both the root's
sameness to the most recent version's inventory and each prior `version`
block's logical state as MUST. Section 3.7 has a version directory's inventory
as SHOULD (W010), so one that is absent is not a violation; these rules bind
where it exists.

## What the fixtures say

With `OCFL_FIXTURES` set, `objects_the_spec_calls_bad_are_not_reported_clean`
reports 51 of 107 caught, as the README says. Eleven of the 56 it misses are
this task's, counting both spec versions: `E037_inconsistent_id` (2),
`E064_different_root_and_latest_inventories` (2),
`E066_algorithm_change_state_mismatch` (2),
`E066_E092_old_manifest_digest_incorrect` (2),
`E066_inconsistent_version_state` (2) and `E103_older_spec_v2` (1).

The four `E060_*` fixtures are sha512, which the crate cannot compute yet, and
the two `E061_invalid_sidecar` are the sidecar's format, so they wait on other
work.

## Beside this

The same pass misses files nothing should hold, which is a walk of the
directory and not a read of inventories, so it is not this task:
a stray file in the object root (E001), in a version directory (E015), in
`content/` unlisted by the manifest (E023) or in `extensions/` (E067). rocfl
refuses the first three in the same way. Only "content outside the content
directory" of those is named in the README's list.

## Done when

Where a version directory holds an inventory, `validate` checks its sidecar
is present and agrees with it, and reports a head inventory that differs from
the root, a prior `version` block whose logical state differs from the root's
(through content paths, where the digest algorithm changed between them), an
`id` that differs, and a version whose inventory `type` is an older spec than
the version before it. Each has a test in `mocfl/src/validate.rs`, the repro
above exits 1, the eleven fixtures are caught, and the README's count moves
with them. `validate` still passes an object with no inventory in a version
directory.
