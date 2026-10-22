---
title: CI cannot build fig 4's JSON-only core
part_of: '[Tasks](/docs/tasks/tasks.md)'
status: open
description: mocfl asks fig 4 for JSON alone, which fig-sys compiles from Zig source; CI installs no Zig, so every job on main has failed since the move to fig 4
created: 2026-10-22
updated: 2026-10-22
---

# CI cannot build fig 4's JSON-only core

Every CI job on `main` has failed since `build(deps)!: move to fig 4`
(runs 36016983521 and 38636310973): `test / clippy / fmt`, `rocfl
cross-validation` and `msrv` all stop in the build script of `fig-sys 4.0.0`.

## Why

`mocfl/Cargo.toml` asks for `fig = { version = "4", default-features = false,
features = ["json"] }`. fig-sys links its prebuilt `libfig.a` only for fig's
default language set; any other set compiles the core from Zig source
(`features_match_prebuilt` in fig-sys's `build.rs`). The workflow installs a
Rust toolchain and nothing else, so the build script finds no `zig`:

```
fig-sys: this build needs to compile the fig core from source (target
`x86_64-unknown-linux-gnu` has no prebuilt library, a non-default language
feature set is active, or FIG_SYS_FORCE_SOURCE is set), but running `zig`
failed: No such file or directory (os error 2).
```

Installing a Zig is not enough on its own either. With Zig 0.17.0 on PATH,
fig-sys 4.0.0's `build.zig` does not compile:

```
build.zig:66:91: error: no field named 'ReleaseSmall' in enum 'lang.Optimize'
```

## Repro

```sh
git clone https://github.com/diaryx-org/mocfl && cd mocfl
cargo build    # with no zig on PATH, or with Zig 0.17
```

## The ways out

- **Move to fig 7.** fig 7 is Rust throughout: `fig` is the core with no
  language features, and each language is its own crate (`fig-json`), so no
  Zig is involved. fig-schema builds on it today. It is a dependency change
  with an API to follow, not a version bump: `fig = "7"` with `features =
  ["json"]` is refused, as `fig` 7 has no `json` feature.
- **Keep fig 4 and install a Zig fig-sys 4 accepts** (0.16) in each job. This
  keeps a C toolchain in a crate that calls itself dependency-thin, and pins
  CI to a Zig release fig no longer needs.

## Done when

`cargo build`, the three CI jobs and the doc tests pass on a machine with no
Zig, and CI on `main` is green.
