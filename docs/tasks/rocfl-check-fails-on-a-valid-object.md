---
title: CI's rocfl check fails on a valid object
part_of: '[Tasks](/docs/tasks/tasks.md)'
status: open
description: rocfl validates the interop directory as a storage root, finds no root declaration, and exits 2, so the `rocfl cross-validation` job fails on main although rocfl finds the object valid
created: 2026-10-27
updated: 2026-10-27
---

# CI's rocfl check fails on a valid object

The `rocfl cross-validation` job has failed on every push to `main`. It
failed in `Emit an object` until `build(deps)!: move to fig 7, whose JSON is
Rust`. Since then it gets as far as `Validate it with rocfl` and fails there
(run 40249398822, on e9960ea). The other two jobs pass.
[CI cannot build fig 4's JSON-only core](/docs/tasks/ci-cannot-build-fig-json-only.md)
says all three jobs pass, but this one has never got through its last step.

## Why

The step validates the whole directory:

```sh
set -o pipefail
rocfl -r "$RUNNER_TEMP/interop" validate 2>&1 | tee report.txt
if grep -q "Invalid objects:  *[1-9]" report.txt; then ... exit 1; fi
```

The comment beside it says the storage-root declaration is expected to be
absent, and the `grep` is meant to tell an invalid object from that. But
rocfl 1.7.0 exits 2 whenever the storage root has an issue, and the job's
shell runs with `-e`. Under `pipefail` the pipeline's status is rocfl's, so
the step stops before the `grep` runs:

```
Storage root is invalid
  Errors:
    1. [E069] Root version declaration does not exist

Object urn:example:interop is valid with warnings
...
Summary:
  Total objects:   1
  Invalid objects: 0
  Storage issues:  1
##[error]Process completed with exit code 2.
```

`--suppress-error E069` does not help: the exit status is still 2.

## Repro

At e9960ea, with `cargo install rocfl --no-default-features --locked` (1.7.0):

```sh
MOCFL_INTEROP_OUT=/tmp/interop cargo test --test interop -- --nocapture
rocfl -r /tmp/interop validate; echo $?          # 2, "Invalid objects: 0"
rocfl -r /tmp/interop validate --paths interop-object; echo $?   # 0
```

Validating the object by its path checks the object alone. It exits 0 for
the object the test writes and 2 once a content file is altered (E092, a
fixity failure), which is what the step's comment asks for.

## Done when

The `rocfl cross-validation` job passes on `main` for the object the interop
test writes, and still fails when rocfl finds that object invalid. One way is
to validate with `--paths interop-object` and let rocfl's exit status decide,
dropping the `grep`.
