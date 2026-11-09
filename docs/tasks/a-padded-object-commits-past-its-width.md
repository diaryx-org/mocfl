---
title: A padded object commits a version past its width
part_of: '[Tasks](/docs/tasks/tasks.md)'
status: open
description: A zero-padded object's next version is allowed one digit wider than the spec permits, so v09 commits v10 and v0999 commits v1000, and validate calls the mixed object clean
created: 2026-11-07
updated: 2026-11-07
---

# A padded object commits a version past its width

OCFL 1.1 lets an object pad its version numbers, on one condition: "If
zero-padded version directory numbers are used then they MUST start with the
prefix v and then a zero. For example, in an implementation that uses five
digits for version directory names then v00001 to v09999 are allowed, v10000
is not allowed." And "all version directories of an object MUST use the same
naming convention" (E011, E012, E013).

`VersionNum::next` in `mocfl/src/inventory.rs` allows one digit too many. It
refuses a successor only when its digits outnumber the padded width
(`number.to_string().len() > self.padding - 1`), so a number that exactly
fills the width, with no zero left in front, gets through. Its doc comment
has the same arithmetic: "`v0001` cannot represent 10000", where the spec's
ceiling for four digits is `v0999`.

## Repro

```rust
use mocfl::VersionNum;

let next = VersionNum::parse("v0999").unwrap().next().unwrap();
assert_eq!(next.to_string(), "v1000");                // the spec forbids it
assert_eq!(VersionNum::parse("v1000").unwrap().padding(), 0); // and it reads back unpadded
assert_eq!(VersionNum::parse("v09").unwrap().next().unwrap().to_string(), "v10");
```

The same through `Object`. mocfl only pads what it opens, so take an object
it wrote, rename `v1` to `v01` and rewrite both inventories and sidecars to
match (`validate` then reports nothing), and commit nine more versions. The
ninth returns `Ok(v10)`, and the object holds `v01` … `v09` beside `v10`:
two naming conventions, which E012 forbids. `validate` and `validate_deep`
both return no violations for it.

Nothing mocfl creates is padded, so this needs an object some other tool
wrote with a padded convention, at its last version: nine versions for
`v01`, 999 for `v0001`.

## Done when

`VersionNum::next` refuses the successor of `v09`, `v0999` and every other
padded number whose successor would fill the width, with `Error::Conflict`
as it refuses an overflow today, and its doc comment names the right
ceiling. `validate` reports an object whose version directories mix
conventions, or whose padded names have no leading zero. The repro above is
a test of each.
