# AGENTS.md

## Purpose

This file defines the engineering rules for agents and contributors working on the title-tmdb-file repository.

The project is a small Rust CLI that interactively selects video files, identifies a movie or TV series through The Movie Database (TMDB), and moves the files to a destination with deterministic, metadata-bearing names.

This file is an implementation guide. The product behavior is defined by README.md. Agents must read both files before making a code change.

The most important rule in this repository is:

> Never sacrifice filesystem safety or metadata correctness for convenience.

A successful implementation is not merely one that compiles. It must preserve user files, make its planned changes visible, use verified TMDB data, and remain easy to test and extend.

## Instruction precedence

When instructions conflict, apply them in this order:

1. The user's current request.
2. More specific AGENTS.md instructions located closer to the file being changed, if any are added later.
3. This AGENTS.md.
4. The product contract in README.md.
5. Normal Rust, Cargo, and operating-system conventions.

A current user request can intentionally change a documented behavior. When that happens:

- implement the requested change only if it is within the requested scope;
- update README.md when the product contract changes;
- update this file when the engineering rule or architecture changes;
- call out the behavior change in the final response.

Do not silently reinterpret a requirement because a different design seems more elegant.

## Repository status

At the time this document was written, the repository is intentionally minimal:

~~~text
title-tmdb-file/
├── Cargo.toml
├── README.md
├── AGENTS.md
└── src/
    └── main.rs
~~~

The current Rust program is only a placeholder. Do not describe an unimplemented feature as already available.

The repository is a binary application, not a library product at this stage. Nevertheless, the core logic must be structured so it can be tested without driving a real terminal or contacting the real TMDB service.

## Project contract

README.md is the authoritative product specification. The following rules are repeated here because they are safety-critical and must remain visible while coding.

### Startup configuration

The normal interactive run has a mandatory startup configuration stage before any destination or source-folder discovery.

Required order:

1. prompt for the TMDB API key with masked/password-style input;
2. prompt for the TMDB metadata language;
3. validate the key and language;
4. continue to current-directory, destination, and source discovery only after configuration succeeds.

Rules:

- the API-key prompt must always be shown on a normal interactive run;
- TMDB_API_KEY may supply a masked default, but it must not silently bypass the prompt;
- the key must remain in memory only for the current execution;
- the language must be selected through an English prompt;
- pt-BR is the initial language default;
- TMDB_LANGUAGE may supply a default, but the language prompt must still be shown;
- the selected metadata language does not translate the application UI;
- an invalid key or language must allow retry or safe cancellation;
- help, version, and invalid-command paths handled by clap occur before this wizard and must not require a key or network access.

### Source and destination

- The application uses the process current working directory as the source root.
- The executable's directory is not automatically the source root.
- Before filesystem discovery, the application must ask for the TMDB API key and metadata language.
- The destination folder is requested after startup configuration and before source folders are listed.
- Destination paths may be absolute or relative to the current working directory.
- A destination that does not exist may be created only after explicit user confirmation.
- The current directory itself cannot be the destination.
- A selected source folder cannot also be the destination.
- If the destination is inside the current directory, it must not appear as a source option.

### Folder and file discovery

- Source folders are direct child directories of the current working directory.
- The MVP does not recursively select nested source folders.
- Video files are direct regular files inside the selected source folder.
- The MVP recognizes the .mkv extension case-insensitively.
- Nested folders are not searched automatically.
- Symbolic links must not be followed in the MVP.
- Discovery and display order must be deterministic.
- The user must explicitly select folders and files; the first item must never be selected silently.

### Unit of work

Each source folder represents exactly one TMDB item.

- A movie folder must have exactly one selected .mkv file.
- A series folder may have one or more selected .mkv files.
- For a series, each selected file receives its own season and episode numbers.
- Multiple selected series files represent episodes of the same series.
- Different movies or series in one source folder are outside the MVP and must not be guessed or merged.

### TMDB identification

- The user can search by text or enter an ID manually.
- Search results must distinguish movies from TV series.
- Search must never silently accept the first result.
- A manually entered numeric ID must be paired with a media type.
- The selected item must be resolved and confirmed through TMDB before a move plan is created.
- A series episode must be validated against TMDB before the final preview.
- TMDB data is the authority for the ID and title.
- The API language is chosen during startup and defaults to pt-BR.
- The application UI, help, prompts, errors, and progress text are always in English.
- The application must not move a file if it cannot obtain and validate the metadata required for its final name.

### Naming

Movie:

~~~text
<tmdb_id> - <normalized_title>.mkv
~~~

Series:

~~~text
<tmdb_id> - S<season>E<episode> - <normalized_series_title>.mkv
~~~

Examples:

~~~text
550 - Fight Club.mkv
1399 - S01E01 - Game of Thrones.mkv
~~~

Naming rules:

- use the numeric TMDB ID without a prefix;
- use the localized title returned by TMDB, after mandatory filename normalization, falling back to the original title only when needed;
- use lowercase .mkv for generated files;
- use at least two digits for season and episode;
- do not add the year, codec, resolution, release group, or original filename;
- do not include the individual episode title in the MVP;
- normalize filesystem-invalid characters, including colon, without changing the ID or episode numbers;
- do not add arbitrary conflict suffixes such as (1).

### Confirmation and movement

- Build and validate the complete plan before any move.
- Display every source and destination path before execution.
- Use a negative default for the final confirmation.
- A declined confirmation must produce no filesystem mutation.
- Never overwrite an existing destination by default.
- A same-volume move should use an atomic or no-replace strategy where the platform permits.
- A cross-volume move must copy to a destination-side temporary file, verify the copy, publish the final name, and delete the source only afterward.
- If an unexpected move fails, stop by default and report completed, failed, and pending items.
- Preserve the source when a copy cannot be verified.

## Engineering principles

### Safety before cleverness

File operations are irreversible from the user's point of view. Prefer a design that is easy to inspect and fail safely over one that is shorter or more clever.

Every operation that can change the filesystem must have:

1. a clear input;
2. a pure or inspectable plan;
3. validation before mutation;
4. an explicit commit point;
5. a useful result or error.

Never combine discovery, prompting, metadata lookup, path generation, and file mutation in one opaque function.

### Make invalid states hard to represent

Use typed domain values instead of passing loosely related strings through the application.

Examples:

- Use a media-type enum instead of a string containing movie or tv.
- Use a numeric type for a TMDB ID instead of storing an unchecked ID string.
- Use a dedicated episode value containing season and episode numbers.
- Use Path and PathBuf for filesystem paths instead of concatenated strings.
- Use a move-plan item containing source, destination, and verified metadata rather than recomputing names during execution.

Validation belongs at boundaries. Once a value enters the domain layer, it should already satisfy the invariants expected by that layer.

### Keep side effects at the edges

Business rules should be testable without:

- an interactive terminal;
- a real current working directory;
- the real filesystem;
- a network connection;
- a real TMDB credential.

Side effects belong behind small interfaces or adapters. Pure transformations such as filename sanitization, filename parsing, plan construction, and collision detection should be ordinary deterministic functions.

### Prefer explicit control flow

This application is a multi-step workflow with cancellation and partial failures. Make the state transitions visible in code.

Prefer:

- explicit result values;
- typed errors;
- named intermediate values;
- small functions with one responsibility;
- state objects that make the next permitted action clear.

Avoid:

- hidden global state;
- implicit retries;
- silently skipping invalid files;
- automatic choices made on behalf of the user;
- callbacks that mutate unrelated parts of the workflow;
- deeply nested closures that hide error paths.

### Determinism matters

The same directory contents and the same confirmed metadata should produce the same plan.

To preserve determinism:

- sort directory entries before displaying or planning them;
- use a documented case-insensitive or platform-aware ordering;
- do not depend on filesystem enumeration order;
- do not use timestamps or random values in final filenames;
- use temporary names only for in-progress cross-volume copies;
- keep plan order stable in previews and reports;
- use stable cache keys for in-memory TMDB results.

## Language and naming conventions

### Language

- Source code identifiers, module names, comments, error variants, help text, and developer documentation must be in English.
- All application-owned user-facing strings must be in English, including prompts, option labels, validation errors, progress messages, summaries, and diagnostics.
- Keep user-facing strings centralized in the CLI layer so wording remains consistent and future localization remains possible.
- The TMDB metadata language is user-configurable and defaults to pt-BR; it is not the language of the application UI.
- Do not mix Portuguese and English inside application-owned messages.
- Treat TMDB titles and other API metadata as external content; do not translate or rewrite them except at the filename normalization boundary.
- README.md and AGENTS.md are written in English so they can serve as stable engineering references.

### Rust naming

Follow standard Rust naming:

- Types and traits: PascalCase.
- Functions, methods, modules, and variables: snake_case.
- Constants and statics: SCREAMING_SNAKE_CASE only when they are genuinely constant values.
- Enum variants: PascalCase.
- Lifetimes: short lowercase names only when needed.
- Acronyms in identifiers should follow Rust style, for example TmdbClient rather than TMDBClient, and HttpError rather than HTTPError.
- Use ID in prose, but prefer id in Rust field names such as tmdb_id.
- Avoid abbreviations that are not already established by the domain.

Names should describe intent. Prefer build_move_plan over process_files, and fetch_series_details over get_data.

### Comments and documentation

Comments should explain why a non-obvious decision exists, not restate what the code visibly does.

Good comment:

~~~rust
// Destination-side temp files prevent a partially copied video from being
// mistaken for a completed organized file.
~~~

Weak comment:

~~~rust
// Copy the file.
~~~

Add Rust documentation comments to public types, traits, and functions when they form a reusable boundary or encode an important invariant.

When a rule comes from README.md, reference the rule in a short comment only when that helps future maintainers. Do not copy the whole README into source comments.

## Rust and Cargo standards

### Formatting

Run rustfmt on every Rust change.

Required checks:

~~~bash
cargo fmt --all -- --check
cargo check
cargo test
~~~

For a change that affects linting or adds dependencies, also run:

~~~bash
cargo clippy --all-targets --all-features -- -D warnings
~~~

If a command cannot run because a dependency or tool is unavailable, report that fact rather than claiming the check passed.

Do not hand-format code against rustfmt. Let rustfmt determine whitespace and layout.

### Compiler warnings

New compiler warnings are not acceptable.

Prefer fixing the underlying issue rather than adding an allow attribute. A narrowly scoped allow can be used only when:

- the warning is intentional;
- the reason is documented;
- the allow is placed at the smallest appropriate scope;
- the behavior is covered by a test when practical.

Do not suppress broad lint groups to make a build green.

### Error propagation

Use Result for fallible operations. Do not panic on:

- user input;
- missing files;
- invalid paths;
- network responses;
- malformed API data;
- permission errors;
- expected cancellation.

Avoid unwrap and expect in production paths. An expect can be acceptable for an invariant that is proven locally and cannot be caused by user or external input, but it must include a precise reason.

Prefer the question-mark operator and preserve useful context.

Example shape:

~~~rust
let entries = filesystem.list_source_folders(&source_root)?;
let plan = planner.build(entries, metadata)?;
executor.execute(plan)?;
~~~

Do not turn every error into a generic string at the first boundary. Preserve enough structure for the CLI to present a useful message and for tests to assert the correct category.

### Ownership and borrowing

- Borrow with Path and &str when ownership is not required.
- Return PathBuf only when the caller needs an owned path.
- Avoid cloning large strings or path collections without a reason.
- Clone small immutable metadata only when it makes ownership and lifetime boundaries clearer.
- Do not store references in long-lived plan objects unless the lifetime relationship is intentional and easy to maintain.
- Prefer owned plan data because the plan must remain valid after the discovery or prompt layer returns.

### Paths and operating systems

- Use Path and PathBuf for paths.
- Do not build paths by concatenating strings or inserting slash characters manually.
- Use Path::join, Path::file_name, Path::extension, and related APIs.
- Treat filenames as operating-system data, not necessarily valid UTF-8.
- Use to_string_lossy only for display or diagnostics, never as the authoritative path representation.
- Be careful with case sensitivity: a case-insensitive extension check is a product rule, while destination collision behavior depends on the host filesystem.
- Do not assume Unix path behavior if the project claims Windows support.
- Do not assume that canonicalize succeeds for a destination that has not been created.
- Preserve the original PathBuf in plans and use normalized or canonical paths only for comparisons and validation.

### Strings and Unicode

Titles come from users and an external API. They may contain:

- accents;
- non-Latin scripts;
- punctuation;
- leading or trailing whitespace;
- characters that are invalid in filenames on one platform but valid on another.

Use Unicode-aware string operations. Do not index a Rust String by byte position unless the code is explicitly operating on byte boundaries.

When truncating a title to satisfy a path-length limit:

- preserve valid UTF-8 boundaries;
- preserve the fixed ID and episode prefix;
- truncate only the title component;
- avoid cutting a grapheme in the middle when the chosen implementation can support grapheme-aware truncation;
- document the chosen length policy;
- test both ASCII and non-ASCII titles.

### Dependencies

Keep dependencies small and justified.

Likely categories include:

- clap for command-line parsing, help, version, and argument diagnostics;
- an HTTP client;
- serde and JSON deserialization;
- typed error handling;
- a dedicated interactive terminal prompt/rendering library;
- temporary directories for tests.

Do not add a crate merely to avoid writing a few lines of straightforward code.

Before adding a dependency:

1. confirm that it supports the project's target platforms;
2. check that it is maintained enough for the project's needs;
3. check its license and transitive dependency impact;
4. confirm that it does not duplicate an existing capability;
5. add a focused use for it rather than adding it speculatively.

clap is a required dependency, but its exact version must still be selected according to the project's supported Rust toolchain. The interactive prompt/rendering library is a separate dependency and must be evaluated for keyboard navigation, multiple selection, search, styling, terminal fallback, maintenance, and platform support. Do not invent a package name or put an unverified crate into Cargo.toml.

For a binary application, Cargo.lock should be generated and versioned once dependencies are introduced, unless the repository's explicit policy changes.

## Required architecture

The implementation should follow the boundaries below. The file names are a guide, but the responsibilities are mandatory.

~~~text
src/
├── main.rs
├── app.rs
├── cli.rs
├── config.rs
├── domain.rs
├── error.rs
├── filesystem.rs
├── naming.rs
└── tmdb/
    ├── mod.rs
    ├── client.rs
    └── models.rs
~~~

### main.rs

main.rs should be thin.

It should:

- initialize the clap parser;
- handle standard help/version paths through clap;
- initialize the application;
- invoke the top-level workflow;
- map the final result to an exit code;
- print only final top-level errors that were not already rendered by the CLI;
- avoid containing business rules;
- avoid performing file moves directly.

Do not put the complete prompt flow in main.rs. Do not make main.rs responsible for parsing TMDB JSON, normalizing titles, or moving files.

### app.rs

app.rs orchestrates the use case.

It should coordinate:

1. startup API-key and language configuration;
2. TMDB configuration validation;
3. current-directory discovery;
4. destination selection;
5. source-folder selection;
6. per-folder video selection;
7. TMDB identification;
8. series episode input;
9. plan construction;
10. full validation;
11. preview and confirmation;
12. execution and final reporting.

It should depend on abstractions or focused modules, not on terminal-specific implementation details.

The application layer may decide which step happens next, but it must not contain low-level path manipulation, raw HTTP parsing, or prompt rendering details.

### cli.rs

cli.rs is the clap command-line boundary and the interactive terminal boundary.

It should:

- define the clap Parser, Subcommand, and argument types;
- render the interactive wizard through a dedicated terminal UI adapter;
- collect text, masked secrets, language choices, single-choice, multiple-choice, confirmation, and numeric input;
- show step indicators, progress, previews, warnings, errors, and reports;
- translate typed application results into consistent English user-facing messages;
- expose cancellation as a normal control-flow result;
- detect non-interactive terminals and report the current limitation clearly;
- avoid performing filesystem or HTTP work directly beyond calling the relevant use case.

clap parsing and interactive rendering are related responsibilities, but they are not the same thing. Keep the clap command model independent from the prompt renderer so future non-interactive commands can reuse the parser without importing terminal UI code.

The CLI layer must not:

- construct final filenames;
- parse TMDB response JSON;
- call std::fs::rename directly;
- decide whether a destination conflict is safe;
- silently choose a search result;
- log credentials.

Keep prompt text in one place where possible. This makes future localization, snapshot tests, and wording changes safer.

### config.rs

config.rs should contain configuration parsing and normalization.

It should handle:

- current working directory;
- destination input normalization;
- TMDB_API_KEY default loading;
- the interactive API-key and language configuration;
- TMDB language;
- timeout and other explicitly supported settings.

Configuration parsing should be separate from business validation. For example, reading a masked default from TMDB_API_KEY is configuration parsing; deciding whether a series episode exists is domain/API validation. The interactive prompt must still be shown even when an environment default is available.

Do not expose raw secret strings through Debug output. If a configuration struct derives Debug, redact or omit credential fields.

### domain.rs

domain.rs should contain the application's stable concepts and invariants.

Expected concepts include:

~~~text
MediaType
TmdbMedia
EpisodeRef
SourceFolder
VideoFile
SelectedMedia
MovePlanItem
MovePlan
ExecutionReport
~~~

Exact names may vary, but the concepts should remain explicit.

The domain layer must not depend on:

- clap;
- terminal rendering or any interactive UI library;
- reqwest or a specific HTTP client;
- environment-variable access;
- direct filesystem mutation.

It may contain pure validation and transformation logic when that logic is independent of external I/O.

### error.rs

error.rs should define errors that preserve actionable categories.

At minimum, distinguish:

- invalid configuration;
- current-directory access failure;
- destination validation failure;
- source discovery failure;
- no eligible videos;
- invalid user input;
- TMDB authentication failure;
- TMDB item-not-found;
- TMDB rate limit;
- TMDB server/network/timeout failure;
- invalid TMDB response;
- episode validation failure;
- destination conflict;
- source changed or disappeared;
- permission failure;
- same-volume move failure;
- cross-volume copy verification failure;
- cancellation;
- partial execution.

Do not create a single ApplicationError::Failed(String) variant for everything.

Error messages should include safe context such as an affected path or TMDB ID, but never include:

- API keys;
- authorization headers;
- complete raw response bodies when they may contain sensitive data;
- unnecessarily noisy implementation details in normal CLI output.

### filesystem.rs

filesystem.rs owns filesystem interaction.

It should provide focused operations such as:

- discover direct source folders;
- discover direct .mkv files;
- normalize and compare paths;
- validate the destination;
- validate a plan;
- execute a safe same-volume move;
- execute a safe cross-volume move;
- produce per-file results.

The filesystem layer should not:

- query TMDB;
- prompt the user;
- decide a movie title;
- infer seasons from filenames;
- create arbitrary conflict suffixes;
- invoke shell commands such as mv, cp, or powershell move commands.

Use Rust filesystem APIs directly. Shelling out creates quoting, platform, error-reporting, and security problems.

### naming.rs

naming.rs should be as pure as possible.

It owns:

- movie filename generation;
- series filename generation;
- title normalization and filesystem sanitization;
- season/episode formatting;
- filename parsing;
- round-trip validation;
- filename-length handling.

It must not:

- perform network calls;
- inspect directories;
- move files;
- read environment variables;
- make UI decisions.

The same input must always result in the same output. Every rule that affects the generated name must be covered by unit tests.

### tmdb/client.rs

client.rs owns HTTP transport and endpoint calls.

It should:

- construct requests with URL/query APIs instead of manual string interpolation;
- reuse one configured HTTP client where possible;
- apply a finite timeout;
- apply the configured language;
- use TMDB_API_KEY according to the documented API-key authentication behavior;
- never print or expose the API key while constructing or reporting a request;
- map HTTP statuses to typed application errors;
- deserialize only the fields the application needs;
- avoid returning raw transport-specific response types to the domain;
- never log credentials.

The client must not:

- prompt the user;
- move files;
- generate final filenames;
- decide which search result the user intended.

### tmdb/models.rs

models.rs should contain API-facing serde models and explicit mapping into domain types.

Keep API models separate from domain models because TMDB field names and optionality are external concerns.

Examples of differences that should remain explicit:

- movie title is returned under title;
- TV series title is returned under name;
- movie year may come from release_date;
- series year may come from first_air_date;
- search results may contain fields that are absent from detail responses;
- episode data has its own season and episode fields.

Do not make external API JSON structs the application's core data model.

## Domain invariants

The following invariants must be enforced by code, not left as comments.

### Media identity

- TMDB IDs are positive numeric values.
- A media item has exactly one media type.
- A movie has no episode reference.
- A series episode reference has non-negative season and episode values.
- The final title comes from verified TMDB details, with the documented localized/original fallback.

### Source selection

- Every selected source path is a regular file.
- Every selected source file has a case-insensitive .mkv extension.
- Every selected source file belongs to one selected source folder.
- No source file appears more than once in a plan.
- A movie plan contains exactly one file.
- A series plan contains at least one file.
- Series episode keys are unique within one execution.

### Destination selection

- The destination is a directory or a validated directory that can be created after confirmation.
- The destination is not the current source root.
- The destination is not one of the selected source folders.
- Every generated destination is a direct child of the destination folder in the MVP.
- A title cannot inject a path separator or escape the destination folder.

### Plan integrity

- Every plan item has one source and one destination.
- Every plan item has verified media metadata.
- Every destination is unique within the plan.
- No destination exists at commit time according to the strongest safe check available on the host.
- The plan shown to the user is the plan executed; do not silently recompute names after confirmation.
- If the plan changes, the user must see and confirm the changed plan again.

## TMDB integration rules

### Authentication and startup configuration

The normal interactive workflow must ask for the TMDB API key before destination selection or filesystem discovery.

Configuration behavior:

1. show a masked API-key prompt;
2. use TMDB_API_KEY as a masked default when present, while still showing the prompt;
3. keep the confirmed key in memory for the current execution only;
4. validate the key before the workflow reaches filesystem selection;
5. fail clearly when the key is missing or rejected.

The MVP uses the TMDB API-key authentication path. Do not add a second authentication mechanism or silently change authentication behavior without updating README.md and this file.

Never:

- print a credential;
- include a credential in a URL shown to the user;
- store a credential in a plan;
- serialize a credential in a debug report;
- commit a credential file;
- use a credential as a cache key that could appear in logs.

### Endpoints

Use the endpoints defined by README.md:

~~~text
GET /3/search/movie
GET /3/search/tv
GET /3/movie/{movie_id}
GET /3/tv/{series_id}
GET /3/tv/{series_id}/season/{season_number}/episode/{episode_number}
~~~

Use query parameters for language and search behavior. Do not hand-build query strings.

The selected language comes from startup configuration and defaults to pt-BR. It must not change the filename contract without an intentional product decision.

### Search behavior

- Search movies and TV series separately or through a carefully filtered equivalent.
- Do not expose people, keywords, or unrelated media types as choices.
- Display type, ID, title, and year when available.
- Preserve enough result information to build a detail request.
- Limit or paginate result display.
- Return control to the user when no result is found.
- Never auto-select a result because it is first, popular, or close enough.
- Fetch detail data after selection before creating the plan.

### Detail and episode behavior

- A manual ID lookup must fetch details for the explicitly selected media type.
- A series episode must be checked using the series ID, season number, and episode number.
- If the API rejects the episode, ask for correction or allow cancellation.
- Do not silently accept a season/episode pair just because the numbers parse as integers.
- If TMDB omits a localized title, use the documented original-title fallback.
- Do not make the title typed into a prompt authoritative.

### Network behavior

- Use a finite request timeout.
- Reuse a client configured with the timeout and authentication behavior.
- Handle 401/403-style authentication failures distinctly from 404 not-found responses.
- Handle 429 rate limits without an unbounded retry loop.
- Handle 5xx responses and transport failures with actionable errors.
- Any retry policy must be bounded, deterministic enough to test, and safe for an interactive command.
- Do not retry a request indefinitely while the terminal appears frozen.
- Do not perform a filesystem mutation while waiting for unresolved metadata.

An in-memory cache is allowed during one execution. Persistent caching is outside the MVP.

Recommended cache keys:

~~~text
search + media type + normalized query + language
movie details + id + language
series details + id + language
episode details + series id + season + episode + language
~~~

A cache must never cause stale or failed metadata to be treated as verified success.

## CLI and clap rules

### clap's role

clap is mandatory for the public command-line interface. It is the source of truth for command-line syntax, standard help/version behavior, and argument diagnostics.

The default command is the interactive wizard:

~~~text
title-tmdb-file
~~~

The initial clap contract must provide:

- application name and description;
- package version;
- --help;
- --version;
- consistent invalid-argument errors;
- usage examples;
- a stable exit-code mapping;
- room for future subcommands and non-interactive modes.

The help and version paths must work without:

- a TMDB API key;
- a network connection;
- a readable media directory;
- an interactive terminal.

Expected development checks:

~~~bash
cargo run -- --help
cargo run -- --version
~~~

Do not parse command-line arguments manually with std::env::args, string matching, or ad-hoc positional conventions. Do not duplicate clap argument validation inside prompt handlers.

A clap parser may contain future options, but an option must not be exposed until its behavior, safety implications, and README documentation are defined.

### First-run configuration order

For a normal interactive invocation, the exact high-level order is:

1. clap parses the command line;
2. the UI asks for the TMDB API key using a masked input;
3. the UI asks for the TMDB metadata language;
4. the application validates the key and language;
5. the application obtains and validates the current working directory;
6. the UI asks for the destination;
7. the UI lists and selects source folders;
8. the UI lists and selects direct .mkv files for each folder;
9. the UI identifies one movie or series per source folder;
10. the UI collects season and episode per series file;
11. the application builds and validates the complete plan;
12. the UI displays the complete preview;
13. the UI asks for explicit confirmation;
14. the executor performs the approved plan and the UI shows the final report.

The API-key and language questions must happen before destination, source-folder, or video-file discovery. The only earlier user-visible paths are clap's --help, --version, and invalid-argument handling.

### Interactive UI boundary

clap handles command-line syntax. A dedicated terminal interaction library handles the wizard controls and visual presentation. The chosen UI library must remain behind the cli.rs boundary.

The UI adapter should expose operations conceptually similar to:

~~~rust
trait InteractiveUi {
    fn ask_tmdb_api_key(&mut self, masked_default: Option<&str>) -> Result<String, UiError>;
    fn choose_tmdb_language(
        &mut self,
        default_language: &str,
    ) -> Result<String, UiError>;
    fn ask_destination(&mut self, initial: &Path) -> Result<PathBuf, UiError>;
    fn select_source_folders(
        &mut self,
        folders: &[SourceFolder],
    ) -> Result<Vec<usize>, UiError>;
    fn select_video_files(
        &mut self,
        folder: &SourceFolder,
        files: &[VideoFile],
    ) -> Result<Vec<usize>, UiError>;
    // Additional operations for search, episode input, preview,
    // confirmation, progress, and reporting.
}
~~~

The exact trait shape is not prescribed. The separation is required:

- clap types belong to the command-line boundary;
- interactive controls belong to the UI adapter;
- workflow decisions belong to app.rs;
- domain rules belong to domain modules;
- filesystem mutation belongs to filesystem.rs.

### Modern terminal quality bar

The CLI must feel like a deliberate product. It must not look like a sequence of unstyled println calls.

Provide, where supported by the selected terminal UI library:

- a branded header with the application name and version;
- a visible step indicator;
- consistent English labels and terminology;
- keyboard navigation;
- obvious selected/unselected states;
- searchable or filterable long lists;
- clear selection counts;
- aligned source/destination preview tables;
- distinct success, warning, error, and informational styles;
- progress feedback for network requests and file operations;
- clear retry, back, and cancel actions;
- useful empty states;
- a concise final summary.

The interface must remain understandable without color, Unicode symbols, or a mouse. Color and symbols are enhancements, not the only carriers of safety-critical information.

### Responsive and accessible terminal behavior

- do not assume an 80-column terminal;
- wrap or truncate long paths deliberately;
- never hide the generated filename or conflict state;
- avoid unreadable escape sequences when output is redirected;
- detect non-interactive output and report the current limitation;
- avoid requiring a mouse;
- make keyboard shortcuts visible;
- keep focus and selection state clear;
- do not make network requests appear to freeze the terminal;
- use a spinner or status line for search, detail loading, episode validation, and plan preparation;
- keep prompts consistent so users can learn the interaction model.

All application-owned CLI text must be in English, including clap help, prompt labels, validation errors, progress messages, summaries, and diagnostics. TMDB titles and other API metadata may appear in the selected TMDB language.

### Cancellation and mutation boundary

Treat cancellation as a normal outcome before commit.

Before confirmation:

- do not create the destination;
- do not rename anything;
- do not move anything;
- do not delete anything;
- return a canceled result that maps to exit code 0.

The UI must never call the move executor from a selection callback. The executor may run only after the plan has been validated, displayed, and explicitly confirmed.

During execution:

- do not pretend a UI cancellation can undo a completed move;
- finish or safely abort the current low-level operation where possible;
- report the exact state of the affected file;
- stop new operations by default if continuing would make the result less predictable.

### Output quality

- Show the current destination while building the plan.
- Show the media type clearly.
- Show the full source-to-destination mapping in the preview.
- Do not communicate safety-critical information by color alone.
- Keep paths readable; allow wrapping or scrolling for long paths.
- Show a useful progress indicator for large files or batches.
- Do not print raw JSON in normal operation.
- Do not display API keys, authorization headers, or request URLs containing credentials.
- Keep normal errors concise and actionable.
- Provide detailed diagnostics only through an explicit development/debug mechanism.

## Filesystem safety rules

### Discovery

Directory enumeration is external input. Entries can disappear or change between listing and execution.

For every discovered entry:

- inspect the entry type;
- ignore symbolic links in the MVP;
- include only the required directory or regular-file kinds;
- handle permission and metadata errors without panicking;
- preserve the actual path for later revalidation;
- sort before returning results.

Do not use a broad recursive walk when the product contract says direct children only.

### Path comparison

Path comparison must account for:

- relative versus absolute forms;
- redundant components;
- platform separators;
- case sensitivity;
- a destination that does not yet exist;
- symbolic-link behavior;
- paths that cannot be canonicalized because a component is missing.

Use canonicalization for existing paths when it is safe and useful, but do not make a nonexistent destination impossible to configure. Compare the nearest existing ancestor or use a carefully normalized absolute path when needed.

If path equality semantics are platform-dependent, make the policy explicit and cover it with tests.

### Plan validation

Plan construction should not mutate the filesystem.

Plan validation should verify:

- all source files still exist;
- all source files are still regular .mkv files;
- source metadata has not changed in a way that makes the selected item ambiguous;
- destination state is compatible with no-overwrite rules;
- generated names are valid;
- destinations are unique;
- no source equals its destination;
- no destination escapes the chosen destination directory;
- the destination can be created if needed;
- required permissions are present where they can be checked.

Revalidate as close as practical to commit because directory state can change after preview.

### No-overwrite movement

A preflight check alone is not a complete no-overwrite guarantee because another process can create the destination between the check and the move.

Use the strongest no-replace primitive available on the target platform. If the chosen Rust abstraction cannot guarantee no replacement:

- reserve the destination safely;
- fail closed when reservation is not possible;
- do not use a plain overwriting rename as a substitute;
- document any platform-specific limitation;
- add a test for the chosen behavior.

Never silently replace a file merely because the operating system's default rename semantics allow it.

### Same-volume moves

Same-volume movement should avoid unnecessary copying.

Requirements:

- preserve the source content;
- publish the destination under the final name only after safety checks;
- avoid destination replacement;
- remove the source only after the destination is known to represent the same file;
- return a typed error if the operation cannot be completed safely.

Do not assume std::fs::rename has no-replace semantics on every operating system.

### Cross-volume moves

Cross-volume movement is a copy followed by source removal.

Required sequence:

1. ensure the destination directory exists only at the approved commit point;
2. create a uniquely named temporary file inside the destination directory;
3. copy the source bytes to the temporary file;
4. flush or close the temporary file as required by the chosen durability policy;
5. verify the copy using at least a reliable size check and, when required by the implementation, a content digest;
6. publish the temporary file as the final destination using no-replace behavior;
7. remove the original source only after publication succeeds;
8. remove temporary artifacts if any step fails.

The temporary filename must not match the final filename. Temporary artifacts should be recognizable, scoped to this application, and cleaned up after failure where safe.

If verification fails, the source must remain. If publication succeeds but source removal fails, report a partial result rather than pretending the operation was a normal move.

### Directory creation

Creating a destination is a filesystem mutation and must obey the confirmation boundary.

Before confirmation:

- inspect whether the destination exists;
- validate its parent and intended path;
- show that it will be created in the preview if it does not exist;
- do not create it as a side effect of merely typing the path.

After confirmation:

- create it with the narrowest required behavior;
- verify it is a directory;
- stop safely if creation fails;
- do not fall back to another directory.

### File identity and changes

A path can point to a different file after discovery. Depending on platform support, revalidation may use:

- file type;
- size;
- modification time;
- filesystem identity metadata;
- an optional content fingerprint.

Do not silently move a newly substituted file under metadata chosen for the previous file.

The exact fingerprint policy may be refined for performance, but the implementation must make a deliberate choice and test the race-sensitive behavior it claims to support.

## Filename generation and parsing rules

### Generation pipeline

Filename generation should follow an explicit sequence:

1. validate the TMDB media identity;
2. choose the localized title or original-title fallback;
3. normalize only the title component for filesystem use;
4. format the numeric ID;
5. format season and episode when the item is a series;
6. assemble the fixed prefix and normalized title;
7. enforce the final filename-length policy;
8. append lowercase .mkv;
9. return a filename component, not an arbitrary path.

The generator must not accept a title containing path separators as an already-safe path.

### Mandatory title normalization

The raw TMDB title must never be placed directly into a filename. Every title must pass through one deterministic function, conceptually named normalize_title_for_filename, before the final filename is assembled.

The original title and the filename title are different values:

- the original title is retained for display, confirmation, and metadata;
- the normalized title is used only as one filename component;
- the normalized title must not be sent back to TMDB as if it were the original title;
- changing normalization must not change the TMDB ID or episode numbers.

The normalization pipeline is:

1. trim leading and trailing Unicode whitespace;
2. remove control characters;
3. replace filesystem-invalid characters, including /, \, :, *, ?, ", <, >, and |, with a readable safe separator;
4. use a readable replacement such as " - ", so Mission: Impossible becomes Mission - Impossible;
5. collapse accidental repeated spaces and replacement separators;
6. remove trailing spaces, periods, and replacement separators;
7. preserve accents and safe Unicode characters;
8. avoid Windows-reserved filename components when Windows support is claimed;
9. shorten only the title component if the operating system path limit requires it;
10. reject the plan if normalization produces an empty title instead of inventing an unverified title.

Examples:

~~~text
Mission: Impossible       -> Mission - Impossible
Spider-Man: No Way Home   -> Spider-Man - No Way Home
What?                     -> What
Title / Director          -> Title - Director
~~~

Normalization must be deterministic and idempotent:

~~~text
normalize_title_for_filename(normalize_title_for_filename(title))
    == normalize_title_for_filename(title)
~~~

The normalized title must never contain a path separator or escape the destination folder. It must not remove the fixed ID or SxxEyy prefix, because those components are assembled outside the title-normalization function.

### Parsing

The parser must recognize only the generated contract, not arbitrary media filenames.

Conceptual patterns:

~~~text
Movie:   ^(?<id>[0-9]+) - (?<title>.+)\.mkv$
Series:  ^(?<id>[0-9]+) - S(?<season>[0-9]+)E(?<episode>[0-9]+) - (?<title>.+)\.mkv$
~~~

The actual parser should:

- handle the extension case-insensitively when reading external files;
- validate numeric ranges;
- distinguish movie and series forms;
- reject empty titles;
- reject path separators in the parsed title component;
- return typed data rather than a map of strings;
- preserve a title hint only as display data;
- never treat the title hint as stronger than the TMDB ID.

### Round-trip tests

For valid domain inputs:

- generated filenames must parse successfully;
- parsed IDs must equal the original IDs;
- parsed media types must equal the original types;
- parsed seasons and episodes must equal the original values;
- the generated extension must be lowercase;
- invalid or ambiguous names must be rejected.

Do not promise that arbitrary old filenames can be parsed unless the product specification explicitly adds backward compatibility.

## Testing standards

Testing is part of the implementation, not a final cleanup step.

### Test layers

Use the smallest test layer that proves the behavior.

#### Unit tests

Use unit tests for pure logic:

- ID validation;
- media-type validation;
- season/episode validation;
- title fallback;
- title normalization for invalid characters, including colon;
- filename generation;
- filename parsing;
- round trips;
- collision detection;
- plan ordering;
- destination containment checks;
- normalization idempotence and path-separator rejection.

Unit tests should not need a real network or a real terminal.

#### Filesystem integration tests

Use temporary directories for:

- direct-folder discovery;
- direct .mkv discovery;
- case-insensitive extension handling;
- symbolic-link exclusion where supported;
- destination exclusion;
- destination creation after confirmation;
- same-volume movement;
- existing-destination conflicts;
- cancellation with no changes;
- source preservation after failures;
- report contents after partial execution.

Do not use the repository itself as a test fixture. Do not create test files in the user's working directory.

#### TMDB client tests

Use a mock HTTP server or a transport abstraction.

Cover:

- movie search;
- series search;
- movie details;
- series details;
- episode details;
- missing credentials;
- invalid credentials;
- not found;
- rate limiting;
- server errors;
- malformed JSON;
- timeout/transport failure;
- language and query parameters;
- absence of secrets in errors and logs.

Automated tests must not depend on live TMDB data.

#### Workflow tests

Test the complete use case with fake UI, fake TMDB client, and temporary filesystem adapters.

Cover at least:

- one movie file;
- multiple series episodes;
- multiple source folders;
- cancellation at each pre-commit stage;
- movie selection with multiple files rejected;
- invalid episode corrected;
- duplicate episode rejected;
- search result explicitly selected;
- manual ID with explicit media type;
- preview differs after correction and must be reconfirmed;
- preflight conflict blocks all moves;
- unexpected failure stops remaining work and reports partial state.

### Test naming

Use test names that describe behavior and expected result.

Prefer:

~~~rust
#[test]
fn movie_filename_contains_id_and_normalized_title() {}
~~~

Over:

~~~rust
#[test]
fn test_1() {}
~~~

When a test encodes a README rule, make that visible in the test name.

### Test isolation

- Use tempfile-based directories.
- Do not rely on the caller's current working directory.
- Do not rely on a user's TMDB environment variables.
- Do not write credentials to fixtures.
- Restore any process-global state changed by a test.
- Avoid tests that depend on enumeration order unless order is the behavior under test.
- Avoid sleeping to wait for filesystem state; use deterministic synchronization or fakes.

### Property and fuzz testing

Property-based tests are valuable for filename logic and parsing if the added dependency is justified.

Useful properties include:

- sanitization never creates path separators;
- sanitization is idempotent;
- generated names stay within the destination as one filename component;
- generated names round-trip through the parser;
- arbitrary invalid titles cannot alter the ID prefix;
- parser rejection does not panic.

Do not add property testing solely for appearance. Add it where it protects a meaningful class of Unicode, punctuation, or boundary bugs.

### CLI snapshots

Snapshot tests can help with preview formatting, but keep them stable:

- do not snapshot terminal colors unless colors are part of the contract;
- use fixed paths and metadata;
- do not include timestamps, random temporary names, or host-specific separators without normalization;
- assert safety-critical fields separately from cosmetic formatting.

## Workflow for agents

### Before editing

1. Read AGENTS.md and README.md.
2. Inspect the repository status.
3. Inspect the relevant source and tests.
4. Identify the smallest change that satisfies the request.
5. Check whether the request changes the product contract or only the implementation.
6. Preserve existing user changes, including changes not related to the current task.

Use read-only commands first. Do not reset, checkout, clean, or delete files to make the worktree look simpler.

### While editing

- Make small, coherent changes.
- Use apply_patch for local file edits.
- Keep unrelated formatting changes out of the diff.
- Add or update tests alongside behavior changes.
- Update README.md when user-visible behavior changes.
- Update AGENTS.md when an engineering convention is introduced or changed.
- Do not add credentials, generated output, or local machine paths.
- Do not silently change the filename format.
- Do not introduce a new dependency without using and justifying it.
- Do not call external services in tests or during a documentation-only task.
- Do not make destructive filesystem changes during development unless the user explicitly requested them and the target is exact and verified.

### After editing

Run proportionate validation.

For documentation-only changes:

~~~bash
git diff --check
~~~

For Rust source changes:

~~~bash
cargo fmt --all -- --check
cargo check
cargo test
cargo clippy --all-targets --all-features -- -D warnings
~~~

For filesystem behavior changes, also run the focused integration tests and inspect the test-created paths.

For TMDB changes, run mocked HTTP tests and confirm no secret appears in captured output.

Review the final diff:

~~~bash
git status --short
git diff --stat
git diff --check
~~~

Do not claim a validation command passed if it was not run.

### Communication

Intermediate updates should say:

- what was inspected;
- what is being changed;
- what safety or compatibility decision was made;
- what validation remains.

The final response should state:

- what changed;
- which files changed;
- which tests/checks passed;
- any known limitation or follow-up;
- whether the behavior contract or only internal structure changed.

Do not bury an unresolved safety issue under a statement that the task is complete.

## Git and change-management rules

- Do not create commits unless the user explicitly asks for a commit.
- Do not push branches or tags unless explicitly asked.
- Do not reset, force-push, or discard user changes.
- Do not use git checkout --, git restore, git reset --hard, or broad clean commands to remove work.
- Do not rewrite unrelated history.
- Inspect status before and after changes.
- Keep generated build output out of the change.
- Version Cargo.lock for the binary once the project has real dependencies, unless the repository policy says otherwise.
- Prefer focused diffs that are easy to review.
- If unrelated modifications prevent a safe change, preserve them and explain the conflict.

Suggested change categories:

- documentation and contract;
- domain and validation;
- pure filename logic;
- filesystem discovery;
- safe movement;
- TMDB transport;
- CLI integration;
- tests and fixtures.

Avoid combining a broad refactor with a behavior change unless the refactor is necessary for safety or testability.

## Security and privacy

### Secrets

- Read TMDB_API_KEY only as a masked default for the mandatory startup prompt, or use a later approved secret mechanism.
- Keep secret fields out of Debug, Display, error chains, snapshots, and reports.
- Do not pass API keys through shell commands.
- Do not include tokens in URLs printed to the terminal.
- Do not commit .env files containing real credentials.
- If .env support is added, ensure the file is ignored and document the behavior without documenting any real value.

### External data

TMDB titles and API fields are external input.

- Treat titles as data, never as shell fragments.
- Do not invoke a shell using a title or path.
- Sanitize only at the filename boundary.
- Use safe path APIs.
- Validate numeric IDs and episode numbers.
- Bound result sizes and response bodies where practical.
- Do not deserialize unbounded or irrelevant data when only a few fields are needed.
- Avoid echoing raw API responses into logs.

### Path traversal

A TMDB title must never be able to:

- create a nested path;
- escape the destination folder;
- select an arbitrary source path;
- overwrite a different file through path syntax.

Generate a single filename component, join it to the validated destination with Path::join, and verify containment according to the platform policy.

### Destructive operations

Moving and deleting are destructive from the user's perspective.

Before any destructive operation:

- verify the exact source path;
- verify the exact destination path;
- show both in the preview;
- require confirmation;
- revalidate near commit;
- use no-overwrite behavior;
- preserve the source until the destination is verified.

Never add a cleanup pass that deletes “unrecognized” files. That is outside the project scope.

## Documentation rules

### README.md

Update README.md when changing:

- the interactive flow;
- supported media types;
- folder traversal;
- extension behavior;
- filename format;
- TMDB endpoint behavior;
- credential configuration;
- collision policy;
- movement semantics;
- CLI options;
- exit codes;
- supported platforms;
- scope or roadmap.

A code change that changes user-visible behavior without a README update is incomplete.

### AGENTS.md

Update this file when changing:

- module responsibilities;
- required validation commands;
- dependency policy;
- security rules;
- testing strategy;
- naming or code style;
- agent workflow;
- safety guarantees.

Do not use AGENTS.md to hide product behavior that belongs in README.md.

### Architecture decisions

For a decision with meaningful long-term impact, add an ADR or a focused section under a future docs/decisions directory if that structure is introduced.

Examples:

- choosing synchronous versus asynchronous HTTP;
- choosing a cross-platform no-replace move primitive;
- choosing a durable operation log;
- adding recursive discovery;
- adding persistent metadata storage;
- changing from one-item-per-folder to per-file metadata.

An ADR should state:

1. context;
2. decision;
3. alternatives considered;
4. consequences;
5. migration or rollback considerations.

Do not create an ADR for ordinary formatting or a local variable rename.

## Implementation order

When implementing the project from its current skeleton, follow this order unless the user requests another sequence.

### Step 1: establish the build

- validate the Rust toolchain;
- add only the dependencies required for the next slice;
- make cargo check and cargo test pass;
- keep main.rs minimal.

### Step 2: implement pure domain and naming logic

- define media types and validated identifiers;
- define episode values;
- implement title fallback;
- implement title normalization and filesystem sanitization;
- implement movie and series filename generation;
- implement filename parsing;
- write unit tests first where practical.

This step should not need a network or terminal.

### Step 3: implement filesystem discovery

- discover the current directory;
- resolve the destination without creating it prematurely;
- list direct source folders;
- exclude the destination;
- list direct case-insensitive .mkv files;
- sort deterministically;
- test with temporary directories.

### Step 4: implement the TMDB boundary

- implement the interactive API-key and language configuration;
- validate the API key and selected language;
- create one reusable client;
- implement search and detail requests;
- map API models to domain models;
- implement bounded errors and timeouts;
- test with mocked HTTP.

### Step 5: implement planning

- combine source selections and verified TMDB metadata;
- collect series episode values;
- generate destinations;
- detect duplicates and existing conflicts;
- render a plan-independent data structure;
- test that a rejected plan causes no changes.

### Step 6: implement the CLI flow

- implement and verify the clap command parser and its help/version output;
- integrate the interactive UI only at the terminal boundary;
- connect prompts to the application workflow;
- implement back/cancel behavior;
- render the complete preview;
- require negative-default confirmation;
- keep all mutation behind the executor.

### Step 7: implement safe movement

- implement same-volume no-replace movement;
- implement cross-volume temp-copy movement;
- verify copy before source removal;
- produce per-file execution results;
- test failures and partial execution.

### Step 8: harden and document

- run the full validation suite;
- inspect behavior on supported operating systems;
- update README acceptance criteria;
- document any intentional limitation;
- review logs and errors for secret leakage;
- inspect the final diff for unrelated changes.

## Definition of done

A change is done only when all applicable conditions are true:

### Product behavior

- The behavior matches README.md.
- Any intentional behavior change is documented.
- No out-of-scope feature was introduced accidentally.
- The user can understand what will happen before it happens.

### Correctness

- Inputs are validated at boundaries.
- Invalid states are rejected with actionable errors.
- Generated names match the documented contract.
- TMDB data is verified before being used.
- The executed plan is the displayed and confirmed plan.

### Safety

- No destination is overwritten.
- Cancellation before confirmation produces no mutation.
- Source files are preserved when a move cannot be verified.
- Cross-volume operations use destination-side temporary files.
- Secrets are not exposed.
- Titles cannot escape the destination path.

### Maintainability

- Modules respect the ownership boundaries in this file.
- Pure logic is independently testable.
- Side effects are isolated.
- New dependencies are justified.
- Comments explain non-obvious decisions.
- Code is formatted and warning-free.

### Verification

- Relevant unit tests pass.
- Relevant integration tests pass.
- cargo fmt check passes.
- cargo check passes.
- clippy passes for source changes when applicable.
- git diff --check passes.
- The final response accurately reports what was and was not verified.

## Common mistakes to avoid

Do not:

- put the complete application in main.rs;
- use the original filename as an unverified source of TMDB metadata;
- infer season and episode when the MVP requires manual input;
- select the first TMDB search result automatically;
- accept a numeric ID without a media type;
- call an API after moving a file to discover whether the title was valid;
- create the destination merely because the user typed it;
- call std::fs::rename without considering overwrite semantics;
- delete the source before a cross-volume copy is verified;
- use a title in a shell command;
- concatenate filesystem paths as strings;
- assume all filenames are UTF-8;
- follow symbolic links by accident;
- walk recursively when the product says direct children only;
- add random or timestamped suffixes to resolve collisions;
- log raw request headers or API responses containing secrets;
- swallow errors and continue with an incomplete plan;
- write live-TMDB-dependent tests;
- add a persistent database before the product asks for one;
- change the default TMDB language merely because documentation is in English;
- bypass the mandatory API-key and language prompts during a normal interactive run;
- put Portuguese application text in the English CLI;
- use a raw TMDB title directly as a filename;
- assume that a colon is valid on every supported operating system;
- bypass clap with manual argument parsing;
- claim a feature is implemented because it appears in the roadmap;
- use destructive Git commands to clean up a worktree;
- mix unrelated refactors into a focused feature without a reason.

## Final checklist for agents

Before handing off any implementation change, verify:

- [ ] I read README.md and this AGENTS.md.
- [ ] I inspected the initial worktree and preserved unrelated changes.
- [ ] I identified whether the request changes product behavior.
- [ ] I confirmed that clap remains the command-line parser.
- [ ] I kept all application-owned code and text in English.
- [ ] I verified that the API key and language are requested before filesystem discovery.
- [ ] I kept UI, domain, TMDB, naming, and filesystem responsibilities separated.
- [ ] I used typed values and errors at important boundaries.
- [ ] I preserved the one-item-per-source-folder rule.
- [ ] I preserved the direct-folder/direct-.mkv discovery rule.
- [ ] I preserved the documented filename format.
- [ ] I normalized titles before filename generation, including colon-containing titles.
- [ ] I added preview and confirmation behavior before mutation.
- [ ] I considered destination collisions and no-overwrite semantics.
- [ ] I preserved source files on unverified cross-volume failures.
- [ ] I checked for credential leakage.
- [ ] I added or updated tests for the changed behavior.
- [ ] I ran the relevant formatting, build, test, and lint checks.
- [ ] I inspected git status and the final diff.
- [ ] My final response states exactly what changed and what was verified.
