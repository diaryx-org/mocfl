---
title: Tasks
part_of: '[mocfl](/README.md)'
description: Deferred work in mocfl — one file each, every one with a done state
contents:
- '[CI cannot build fig 4''s JSON-only core](/docs/tasks/ci-cannot-build-fig-json-only.md)'
- '[CI''s rocfl check fails on a valid object](/docs/tasks/rocfl-check-fails-on-a-valid-object.md)'
created: 2026-10-22
updated: 2026-10-27
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
