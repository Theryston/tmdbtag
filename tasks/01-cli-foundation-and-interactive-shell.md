# Task 01 — CLI Foundation and Interactive Terminal Shell

**Status:** Completed
**Priority:** P0
**Dependencies:** None
**Blocks:** Tasks 02, 03, 04, and 05

## Objective

Turn the initial Rust skeleton into a stable, modern command-line application boundary. This task establishes how the executable starts, how command-line arguments are parsed, how the interactive wizard is rendered, how cancellation is represented, and how application-owned text is kept in English.

This task is about the shell around the product. It must not implement the TMDB client, filesystem discovery, filename generation, or file movement. Those capabilities belong to the later tasks and should be connected through explicit interfaces rather than embedded in `main.rs`.

## Required outcome

Running the binary without arguments must enter the default interactive organization workflow. Running the standard command-line paths must behave predictably:

```text
title-tmdb-file
title-tmdb-file --help
title-tmdb-file --version
```

The `--help` and `--version` paths must be handled by `clap` and must finish without asking for a TMDB API key, making a network request, or scanning the current directory. Invalid command-line input must produce a useful `clap` diagnostic and a non-zero usage exit code.

The normal workflow must expose a polished terminal experience with clear visual hierarchy, keyboard-friendly controls, searchable selection lists where lists are long, progress feedback during work, and an explicit confirmation before mutation. The interface must remain usable in a small terminal and must not depend on mouse input.

## Implementation decision

The foundation uses:

- `clap` for the command-line parser, help, version, and usage diagnostics;
- `dialoguer` for masked secrets, text input, keyboard selection, and multiple selection;
- `crossterm` for polled raw keyboard events used by the live remote-search selector;
- an adapter-level case-insensitive filter for generic searchable long lists before displaying selection controls;
- `indicatif` for spinner/progress contracts;
- `thiserror` for typed configuration, UI, and application errors.

The renderer-neutral interaction contracts live in `src/ui.rs`. The concrete terminal adapter lives in `src/cli.rs`, and the application workflow consumes the contracts without depending on dialoguer or indicatif directly.

## Scope

### 1. Establish the Rust command boundary

- Keep `main.rs` thin. It should initialize the command boundary, invoke the application entry point, and map typed outcomes to exit codes.
- Use `clap` as the only source of truth for command-line parsing, standard help, version output, and argument diagnostics.
- Prefer the derive API when it produces a clear and maintainable command model.
- Define the default invocation before adding any optional command or flag.
- Do not invent non-interactive options such as `--source`, `--destination`, or `--dry-run` unless the root README is updated with their behavior and safety rules first.
- Do not parse `std::env::args` manually or duplicate `clap` validation inside prompt handlers.
- Keep command-line syntax independent from the terminal renderer so future non-interactive commands can reuse the parser boundary.

### 2. Define the interactive UI boundary

Create a dedicated terminal interaction abstraction, adapter, or equivalent boundary. The exact prompt/rendering crate may be selected during implementation, but it must be evaluated for:

- password-style masked input;
- text input and validation feedback;
- searchable single selection;
- searchable multiple selection;
- keyboard navigation;
- cancellation and interruption handling;
- styled panels, tables, spinners, and progress indicators;
- terminal resize behavior;
- color-disabled and non-color fallback behavior;
- Windows, macOS, and Linux support;
- maintenance quality and compatibility with the supported Rust toolchain.

The UI boundary should be able to express the following interactions without exposing terminal-library types to the domain layer:

- ask for a masked TMDB API key;
- ask for the TMDB metadata language;
- ask for a destination path;
- open one unified explorer containing root-level videos and folders with video descendants;
- expand or collapse folders with `Tab` and select one or more recognized video files with `Space`;
- search and select a TMDB result;
- choose movie or series for a manual ID;
- ask for season and episode values;
- go back, retry, or cancel where the flow allows it;
- show a plan preview;
- request a negative-default confirmation;
- show per-file progress and a final report.

The interface should return typed values and typed cancellation instead of making the application inspect raw strings or terminal escape sequences.

### 3. Implement the startup order

The normal interactive order must be represented in the application flow even before all capability tasks are complete:

1. `clap` parses the command line.
2. The application loads the per-user TMDB configuration.
3. The UI asks for the TMDB API key using masked input only when the saved key is missing or invalid.
4. The UI asks for the TMDB metadata language only when the saved language is missing or invalid.
5. The application validates and persists a complete startup configuration.
6. The application obtains the current working directory.
7. The UI asks for the destination folder.
8. The remaining source, media, identification, planning, and execution steps follow.

Task 01 owns the flow boundary and interaction contracts. Task 02 owns the real credential and language validation. Task 03 owns filesystem discovery. Do not bypass the order merely because a later subsystem is not implemented yet; use typed stubs or test doubles during incremental development.

### 4. Keep application-owned text in English

All of the following must be written in English:

- `clap` command names, descriptions, help text, and version text;
- prompt labels and instructions;
- validation errors;
- cancellation messages;
- progress messages;
- preview headings;
- final summaries;
- logs and diagnostics intended for users;
- source comments and test descriptions;
- documentation and task files.

TMDB metadata is external data and may be displayed in the language selected by the user. That does not authorize translating the application interface or mixing localized metadata into fixed UI labels.

### 5. Establish visual and interaction quality

The terminal should communicate state clearly. At minimum:

- show the current stage and, where practical, progress through the wizard;
- use consistent headings and spacing for each stage;
- distinguish editable input, selected items, warnings, errors, and successful results;
- show the selected destination throughout the operation;
- make destructive or irreversible actions visually distinct;
- use a confirmation default of no;
- never use color as the only carrier of safety-critical meaning;
- avoid dumping raw JSON, unbounded search results, or noisy debug output into the normal UI;
- keep long paths readable through wrapping, truncation with a visible indication, or a detail view;
- show a single per-file context line and preserve completed file logs as readable execution history;
- preserve a usable plain-text fallback when styling is unavailable or disabled;
- make cancellation obvious and safe.

The interface may be beautiful, but visual polish must not conceal what will happen to a file. A user must be able to inspect source paths, destination paths, IDs, titles, and episode values before confirming.

## Explicit non-goals

Do not implement in this task:

- TMDB HTTP requests or API response mapping;
- filesystem traversal or video-extension filtering;
- filename normalization or parsing;
- moving, copying, renaming, deleting, or creating media files/directories;
- persistent TMDB metadata caching or a local database;
- a non-interactive automation mode;
- mouse-only controls;
- Portuguese application text;
- arbitrary collision suffixes or automatic overwrite behavior.

## Suggested code boundaries

The exact module names may change, but the responsibilities should remain recognizable:

```text
src/main.rs       process entry point and exit-code mapping
src/cli.rs        clap parser plus interactive UI boundary
src/app.rs        application workflow orchestration
src/config.rs     startup configuration value types
src/domain.rs     typed workflow values and outcomes
src/error.rs      typed application errors
```

`cli.rs` may know about `clap` and the chosen terminal UI library. Domain and use-case modules must not depend on either. The UI must not construct final filenames, parse TMDB JSON, decide whether a path is safe, or perform file mutation directly.

## Tests and verification

Add tests for:

- default invocation parsing;
- `--help` parsing and successful help rendering;
- `--version` parsing and successful version rendering;
- invalid argument diagnostics and usage exit behavior;
- a fake UI returning values in the required startup order;
- cancellation at each pre-confirmation interaction;
- negative-default confirmation behavior;
- English application-owned labels and errors where practical;
- behavior when the terminal does not support color or interactive features;
- behavior when the terminal is too narrow for the preferred layout.

Use a fake or scripted UI in application tests. Do not require a human, a real terminal session, a real API key, or a live TMDB request for unit tests.

## Acceptance checklist

- [x] `clap` owns command-line parsing, help, version, and usage diagnostics.
- [x] `title-tmdb-file --help` exits without credentials, network access, or filesystem discovery.
- [x] `title-tmdb-file --version` exits without credentials, network access, or filesystem discovery.
- [x] The default invocation enters the interactive workflow.
- [x] The UI boundary supports masked input, searchable selection, multiple selection, confirmation, cancellation, and progress contracts.
- [x] When configuration questions are needed, the API-key question precedes the language question, and both precede filesystem discovery.
- [x] All application-owned text is English.
- [x] The UI remains understandable without color and in a narrow terminal.
- [x] `main.rs` contains no business rules or file operations.
- [x] Automated tests cover parser paths, cancellation, and the UI/application boundary.
- [x] Formatting, build, tests, and applicable lint checks pass.
