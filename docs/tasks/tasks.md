---
title: Tasks
part_of: '[mocfl](/README.md)'
description: Deferred work in mocfl — one file each, every one with a done state
contents:
- '[CI cannot build fig 4''s JSON-only core](/docs/tasks/ci-cannot-build-fig-json-only.md)'
- '[CI''s rocfl check fails on a valid object](/docs/tasks/rocfl-check-fails-on-a-valid-object.md)'
- '[Commit accepts logical paths that conflict](/docs/tasks/commit-accepts-conflicting-logical-paths.md)'
- '[Validate calls an object valid without reading its version inventories](/docs/tasks/validate-never-reads-a-version-inventory.md)'
- '[An unreadable file commits a partial version](/docs/tasks/an-unreadable-file-commits-a-partial-version.md)'
- '[A version inventory can claim a later head](/docs/tasks/a-version-inventory-claims-a-later-head.md)'
created: 2026-10-22
updated: 2026-11-09
---

# Tasks

Work this crate has committed to and has not done yet, one document each. A
bug is a task with a repro; anything else is a task with a done state written
down, so that finishing it is a fact rather than an opinion.

`contents` above lists every task, open and closed. Closing a task is an edit,
not a delete: its `status` becomes `done` or `dropped`, it names the commit
that resolved it, and it keeps its place above.

`status` takes `open`, `in-progress`, `done`, or `dropped`, and nothing else,
so that a tool can read it across every repository in the org.
