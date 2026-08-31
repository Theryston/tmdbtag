# Task 02 — TMDB Configuration and Identification

**Status:** Completed — per-user configuration persistence, credential validation, search, manual identification, verified details, and episode validation are implemented and covered by offline tests with a local mocked HTTP server.
**Priority:** P0
**Dependencies:** Task 01
**Blocks:** Task 05

## Objective

Implement the TMDB boundary that configures credentials, selects the metadata language, searches for movies and TV series in real time, confirms manually entered IDs, and validates series episodes. This task supplies trusted metadata to the planning layer without knowing how files are moved or how filenames are assembled.

The TMDB API is the authority for the identifier, media type, and title. A user-entered title is never a substitute for a confirmed TMDB response. The application UI remains English even when the selected TMDB language is not English.

## Implementation delivered

The task is implemented across the following boundaries:

- `src/config.rs` owns the per-user JSON store, masked/default configuration prompts, locale normalization, and the shared `config`/startup configuration flow.
- `src/tmdb/client.rs` owns the reusable blocking HTTP client, TMDB v3 request construction, API-key query authentication, language propagation, bounded response handling, timeouts, and typed HTTP failures.
- `src/tmdb/models.rs` keeps TMDB response structs separate from the application's domain models and applies title fallback, year extraction, adult-result filtering, ID checks, and media-type checks.
- `src/domain.rs` owns positive TMDB IDs, typed media types, non-negative episode references, search candidates, verified items, and verified episodes.
- `src/app.rs` exposes reusable identification and episode-validation workflows that keep prompts, retries, confirmation, and TMDB calls separate from filesystem operations.
- `src/cli.rs` implements the English interactive TMDB method, result-selection, confirmation, and episode-input controls behind the renderer-neutral UI traits.

Credential validation uses `GET /3/configuration` before the future filesystem-selection stage. Search
uses the movie and TV endpoints separately, always sends the configured language and
`include_adult=false`, limits the mapped result set, and never selects a result automatically.

The client tests use a local loopback HTTP server and do not require a real API key, network access,
or live TMDB data. The normal application workflow intentionally stops after verified startup
configuration until Tasks 03–05 connect source selection, naming, planning, and movement.

## Required outcome

During every normal interactive run, after `clap` has handled the command line and before the application asks for the destination or discovers source folders:

1. resolve `~/.title-tmdb-file/config.json` in the current user's home directory;
2. load the optional `tmdb_api_key` and `tmdb_language` fields;
3. ask only for missing or invalid fields, using masked input for the API key;
4. save a complete configuration after local validation;
5. validate the configuration with TMDB;
6. continue only after the configuration is accepted or the user explicitly cancels.

When both fields are missing, the API-key prompt comes first and the language prompt comes second.
When both fields are valid, neither prompt is shown. The `title-tmdb-file config` command always
reopens both fields and uses the same prompt, validation, and persistence implementation. The user
may use `TMDB_API_KEY` and `TMDB_LANGUAGE` as fallback defaults for missing fields, but their presence
must not silently remove a required prompt. The initial language default is `pt-BR`. All app-owned
prompts, diagnostics, and status text remain in English.

## Scope

### 1. Credential and language configuration

- Define a typed startup configuration containing the API key and selected metadata language.
- Define a storage boundary for `~/.title-tmdb-file/config.json` with optional fields so missing values can be detected.
- Read an existing saved API key as a masked default and an existing saved language as an editable default.
- Read `TMDB_API_KEY` and `TMDB_LANGUAGE` only as fallback defaults when the corresponding saved field is unavailable.
- During the normal run, prompt only for missing or invalid fields; the `config` command must prompt for both.
- Never echo the API key, include it in debug output, put it in a preview, or write it into a filename or operation plan.
- Persist the accepted key only in the documented per-user configuration file, never in a project file or repository.
- Keep the key in memory while the current execution uses it.
- Use owner-only directory/file permissions where the host supports them (`0700`/`0600` on Unix-like systems).
- Validate that the language is a supported TMDB language/locale format before making identification requests.
- Validate the API key against TMDB before filesystem selection.
- Let the user retry or cancel when validation fails.
- Avoid passing the credential through shell commands, filenames, error strings, or structured logs.

The exact TMDB authentication mechanism must follow the current official TMDB application-authentication documentation. Do not add a second, undocumented authentication mode merely to make a request pass locally.

### 2. Build one reusable TMDB client

Create a client boundary with:

- one configured base URL;
- one finite request timeout policy;
- one authentication configuration;
- one language parameter applied consistently to metadata requests;
- typed request and response errors;
- bounded handling for rate limits, authentication failures, server failures, and network failures;
- no file-system or terminal responsibilities.

The client should centralize request construction so that authentication, language, `include_adult=false` search behavior, timeout settings, and error mapping do not drift between endpoints.

### 3. Search movies and TV series

Provide real-time search for both supported media types. The UI must offer an explicit way to choose the search path and must clearly label each result as `MOVIE` or `SERIES`.

Each displayed candidate should include, when available:

- media type;
- numeric TMDB ID;
- localized title/name;
- year or first-air/release year;
- enough context to distinguish similar results.

Search behavior must:

- allow the query to be repeated;
- limit or paginate displayed results so the terminal remains usable;
- never silently choose the first result;
- fetch details for the selected result;
- show a final identification confirmation before the result enters the plan;
- preserve the authoritative numeric ID even when displayed text is localized.

Use the documented movie and TV search endpoints. Do not include people, keywords, collections, or other TMDB entities as identification candidates.

### 4. Confirm a manually entered ID

Manual identification must ask for the media type before asking for the numeric ID. The flow must:

1. ask whether the ID identifies a movie or a TV series;
2. accept only a positive numeric ID;
3. fetch details from the matching endpoint;
4. reject a missing ID or a response that does not match the requested type;
5. display the returned type, ID, and title/name;
6. require explicit confirmation;
7. allow correction, retry, or cancellation.

Never accept a free-form title as authoritative metadata, and never treat a numeric ID as untyped.

### 5. Validate series episodes

For each selected series file, the later workflow will provide a season and episode number. Implement the TMDB operation needed to verify that combination before a plan can be confirmed.

Rules:

- accept non-negative integers;
- allow season `0` only when TMDB accepts the requested special;
- do not infer season or episode from the original filename in the MVP;
- reject a combination that TMDB cannot confirm;
- return enough typed information for the UI to explain the invalid combination;
- do not use the episode title in the MVP filename.

The client may use series details to validate the series itself and the episode-details endpoint to validate each season/episode pair. The domain should receive a compact internal model rather than leaking HTTP response structs throughout the application.

## Domain data returned by this task

The exact Rust names are implementation details, but the boundary should provide values equivalent to:

```text
TmdbItem {
    id: positive integer,
    media_type: Movie or Series,
    title: canonical title selected for the configured language,
    original_title: optional original TMDB title,
}
```

The title must remain raw metadata at this boundary. Filename normalization belongs to Task 04. Keep enough original metadata to support display and a future metadata-retrieval command.

## Explicit non-goals

Do not implement in this task:

- source-folder or `.mkv` discovery;
- file selection;
- filename normalization or filename parsing;
- plan construction or collision handling;
- moving, copying, renaming, deleting, or creating media files;
- a persistent TMDB cache or local database;
- automatic search-result selection;
- automatic season/episode inference;
- Portuguese application text;
- a second media database or provider.

An in-memory request cache for the current process may be used to avoid duplicate requests, but it must not become a persistent data store without a separate product decision.

## Error behavior

Map failures into actionable, non-secret application errors. At minimum distinguish:

- missing or empty key;
- rejected credentials;
- invalid language;
- network unavailable or timed out;
- rate limited;
- TMDB server failure;
- search returned no candidates;
- detail ID not found;
- type mismatch;
- invalid season/episode;
- user cancellation.

Error messages must not contain the API key, authorization headers, full credential-bearing URLs, or raw response bodies when they may contain sensitive request data.

## Tests and verification

Use mocked HTTP responses or a local test server. Tests must not depend on a real API key, network availability, current TMDB data, or search-result ordering.

Cover at least:

- configuration-file round trips through the documented JSON schema;
- missing configuration files and partially populated files;
- complete saved configuration that skips both normal startup prompts;
- partially saved configuration that prompts only for the missing field;
- the `config` command reopening both fields through the shared wizard;
- masked startup configuration through a fake UI;
- environment defaults that still require a prompt when the saved field is absent;
- file and directory permission behavior where supported;
- API-key validation before filesystem discovery is invoked;
- language propagation to requests;
- movie search result mapping;
- TV search result mapping;
- repeated searches and empty-result handling;
- manual movie ID confirmation;
- manual series ID confirmation;
- rejection of zero, negative, malformed, and untyped IDs;
- missing details and media-type mismatch;
- series episode validation, including special season behavior;
- timeouts, rate limits, authentication failures, and server errors;
- redaction of credentials in errors and debug representations.

## Acceptance checklist

- [x] The normal workflow resolves the per-user configuration path after `clap` parsing.
- [x] The documented JSON fields are `tmdb_api_key` and `tmdb_language`.
- [x] A complete saved configuration skips both normal startup prompts.
- [x] A partially saved configuration prompts only for missing or invalid fields.
- [x] The `config` command reopens both fields and reuses the shared configuration code.
- [x] API-key input is masked and never echoed by the terminal adapter.
- [x] Saved configuration is written with owner-only file permissions where supported.
- [x] The API-key prompt is the first missing-field question after `clap` parsing.
- [x] API-key input is validated against TMDB before filesystem discovery.
- [x] The language prompt is the next missing-field question when both fields are missing.
- [x] `TMDB_API_KEY` and `TMDB_LANGUAGE` provide fallback defaults without bypassing required prompts.
- [x] The initial language default is `pt-BR`.
- [x] Configuration is validated against TMDB before destination or source-folder discovery.
- [x] One reusable TMDB client applies authentication, language, timeout, and error policy consistently.
- [x] Movie and TV searches happen in real time.
- [x] Search candidates are clearly typed and no result is selected silently.
- [x] Manual IDs require an explicit media type and positive numeric value.
- [x] Details are fetched and confirmed before metadata enters the operation plan.
- [x] Series season/episode combinations can be validated through TMDB.
- [x] All application-owned text is English.
- [x] No credential appears in logs, errors, previews, filenames, plans, or persistent state outside the documented per-user configuration file.
- [x] Mocked tests cover success, failure, cancellation, and redaction paths.
