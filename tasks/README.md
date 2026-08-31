# Implementation Tasks

This directory breaks the product specification into five cohesive implementation tasks. The task breakdown is intentionally small: each file represents a meaningful capability, and testing, documentation, and integration work remain part of the task that owns the capability instead of becoming separate administrative tasks.

The product contract is defined by the root [README.md](../README.md). Engineering conventions, safety rules, and implementation boundaries are defined by [AGENTS.md](../AGENTS.md). If a task appears to conflict with either document, stop and resolve the specification mismatch before writing code.

## Task map

| Task | Capability | Depends on | Blocks | Status |
| --- | --- | --- | --- | --- |
| [01 — CLI foundation and interactive shell](01-cli-foundation-and-interactive-shell.md) | Rust binary setup, `clap`, terminal UI boundary, English interaction contract, and application entry flow | None | 02, 03, 04, 05 | Completed |
| [02 — TMDB configuration and identification](02-tmdb-configuration-and-identification.md) | Per-user API-key configuration, metadata language, searches, manual IDs, details, and episode validation | 01 | 05 | Completed |
| [03 — Filesystem discovery and media selection](03-filesystem-discovery-and-media-selection.md) | Current-directory discovery, destination selection, source-folder selection, and `.mkv` selection | 01 | 05 | Not started |
| [04 — Naming normalization and metadata recovery](04-naming-normalization-and-metadata-recovery.md) | Deterministic movie/series filenames, invalid-character handling, and parsing generated names | 01 | 05 | Not started |
| [05 — Plan, preview, and safe movement](05-plan-preview-and-safe-movement.md) | End-to-end planning, conflict detection, confirmation, same-volume moves, cross-volume safety, and reporting | 01, 02, 03, 04 | None | Not started |

## Recommended sequence

1. Complete Task 01 so the project has a stable command-line and terminal-interaction boundary.
2. Complete Task 02, Task 03, and Task 04 in any order. They are separate capability slices and can be developed in parallel once their shared domain contracts are agreed.
3. Complete Task 05 after the three capability slices are available. It is the only task allowed to connect the validated metadata, selected files, generated names, and filesystem mutation into the complete workflow.
4. Run the full verification checklist from Task 05 and update the root documentation if implementation details intentionally diverge from the specification.

## Task conventions

Every task must:

- keep application-owned code, prompts, help, diagnostics, tests, and documentation in English;
- define typed boundaries between the UI, domain, TMDB, naming, and filesystem layers;
- include automated tests for the behavior it owns;
- preserve the one-TMDB-item-per-source-folder rule;
- avoid implementing features listed as initially out of scope;
- avoid logging credentials, authorization headers, or sensitive request data;
- keep file mutation behind a validated and confirmed operation plan;
- report limitations honestly instead of marking a checklist item complete prematurely.

The task files describe outcomes and constraints, not a mandate to copy a particular implementation literally. Rust types, function names, and internal module details may improve during implementation as long as the observable behavior and safety guarantees remain intact.

## Definition of done for a task

A task is complete only when its implementation, tests, and integration points satisfy its acceptance checklist. Before marking it complete:

- run the relevant formatting, build, test, and lint checks;
- test failure and cancellation paths, not only the happy path;
- inspect user-facing text for English, clarity, and secret leakage;
- verify that the task did not introduce undocumented CLI behavior;
- update the root README when the product contract changed;
- inspect `git status` and the final diff for unrelated changes.
