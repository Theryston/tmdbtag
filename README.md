# tmdbtag

> Turn a messy video folder into a clean, searchable, TMDB-powered library —
> directly from your terminal.

`tmdbtag` is a modern, interactive Rust CLI for giving video files a reliable
identity. It finds videos recursively, lets you select exactly what you want in
a keyboard-friendly file explorer, identifies every file with
[The Movie Database (TMDB)](https://www.themoviedb.org/), and creates
deterministic names that retain the essential metadata needed to understand the
file later.

It is deliberately focused: no fragile filename guessing, no silent first-result
selection, no overwrites, and no irreversible filesystem action hidden behind a
prompt. You see the complete plan first, choose whether to copy or move, confirm
it, and then watch a real byte-based progress bar with live transfer speed while
the operation runs.

## Why tmdbtag feels different

Organizing a media collection should not require remembering obscure naming
rules or manually looking up IDs in a browser. `tmdbtag` turns that repetitive
work into a calm, guided workflow:

- Search TMDB as you type with debounced live results.
- See whether a result is a movie or a TV series before selecting it.
- Identify each selected file independently, even when several files share a
  folder.
- Enter a TMDB ID directly when you already know exactly what you want.
- Enter and validate a season and episode for series files through TMDB.
- Browse the entire source tree in one expandable explorer instead of jumping
  between folders.
- Keep the interface in polished, consistent English while requesting TMDB
  metadata in your chosen language.
- Copy files while preserving the originals, or move them only after safe
  publication.
- Preview every source-to-destination mapping before anything changes.
- Use a reserved filename delimiter so basic metadata can be recovered
  programmatically later.
- See progress based on actual transferred bytes, not an arbitrary file counter,
  together with a live `KB/s`, `MB/s`, `GB/s`, or `TB/s` transfer-rate display.

The result is a collection that is easier to scan today and easier to automate
tomorrow.

## A quick look

The normal workflow is intentionally easy to understand:

```text
tmdbtag
  │
  ├─ Load or collect your TMDB API key and metadata language
  ├─ Choose the source storage (local filesystem or S3)
  ├─ Choose the destination storage (local filesystem or S3)
  ├─ Load the saved S3 bucket catalog when it is needed
  ├─ Require one bucket now if S3 has not been configured yet
  ├─ Choose the source and destination bucket independently when needed
  ├─ Choose an optional source or destination prefix for this run
  ├─ Choose Copy or Move
  ├─ Choose the destination folder or object prefix
  ├─ Select videos from the recursive file explorer
  ├─ Identify each file with live TMDB search or a direct ID
  ├─ Enter and validate episode data for series
  ├─ Review the complete plan
  ├─ Confirm
  └─ Transfer files with safe publication and byte-based progress
```

Example output names:

```text
550__S__MOVIE__S__Fight Club.mkv
1399__S__SERIES__S__S01E01__S__Game of Thrones.mp4
```

Every part has a purpose. The ID is stable, the media type is explicit, the
series episode is machine-readable, and the title remains pleasant for humans to
read.

## Quick start

You do not need Rust to use `tmdbtag`. The installer scripts detect your
operating system and CPU architecture, download the latest GitHub Release,
verify its SHA-256 checksum, and install the binary for your user.

For Linux or macOS:

```bash
curl -fsSL https://raw.githubusercontent.com/Theryston/tmdbtag/main/unix.sh | bash
```

For Windows PowerShell:

```powershell
irm https://raw.githubusercontent.com/Theryston/tmdbtag/main/win.ps1 | iex
```

The commands intentionally use GitHub's `raw.githubusercontent.com` endpoint. A
GitHub `/blob/` URL opens an HTML page for humans and is not the script content
expected by `bash` or `iex`. You can inspect the installer source before running
it: [unix.sh](https://github.com/Theryston/tmdbtag/blob/main/unix.sh) and
[win.ps1](https://github.com/Theryston/tmdbtag/blob/main/win.ps1).

The installers currently support these release targets:

| Platform | Architectures        | Installation location             |
| -------- | -------------------- | --------------------------------- |
| Linux    | x86_64, ARM64        | `~/.local/bin` or `$XDG_BIN_HOME` |
| macOS    | Intel, Apple Silicon | `~/.local/bin` or `$XDG_BIN_HOME` |
| Windows  | x86_64, ARM64        | `%LOCALAPPDATA%\Programs\tmdbtag` |

The Unix installer can use `TMDBTAG_INSTALL_DIR` to select another user-writable
directory. The Windows installer accepts the same setting as an environment
variable. Neither installer requires `sudo` or administrator privileges when the
default user-local location is writable. Restart your terminal if the new
command is not immediately visible through `PATH`.

### Basic usage

```bash
tmdbtag
```

The guided session looks like this:

1. `tmdbtag` loads the current user's configuration.
2. If needed, it securely asks for the TMDB API key and asks which language TMDB
   should use for metadata.
3. It asks where the source files live: the local filesystem or S3-compatible
   object storage.
4. It asks where the organized files should be written: locally or in S3.
5. If either side is S3 and no bucket is saved, it requires you to add one
   before continuing. If multiple buckets are saved, it lets you select the
   source bucket and destination bucket independently.
6. If the source is S3, it asks for an optional source prefix. The empty value
   means the selected bucket root.
7. It asks whether the operation should be `Copy` or `Move`.
8. It asks for a local destination folder or an optional S3 destination prefix.
   The empty value means the destination bucket root.
9. It scans the selected source recursively and opens one unified video
   explorer.
10. You expand folders, select individual videos, and confirm the selected array.
11. For each selected file, you search TMDB live or enter a TMDB ID manually.
12. For series, you enter a season and episode, which are validated against
    TMDB.
13. It builds and displays the complete plan, including every destination name.
14. A final confirmation starts the operation. The default is always negative,
    and the progress bar shows the current transfer speed while bytes move.

When local storage is selected, the directory where you launch the command is
the source root; the executable's location is not used as the source root. When
S3 is selected, the selected bucket and optional source prefix are the source
root for that run.

## Commands

The command surface is intentionally small:

```bash
tmdbtag                # Start the interactive organization workflow
tmdbtag config         # Reopen and update the saved TMDB configuration
tmdbtag storage add    # Add one named S3 bucket to the local catalog
tmdbtag storage remove # Remove one named S3 bucket from the local catalog
tmdbtag --help         # Show help without starting the wizard
tmdbtag --version      # Show the installed version
```

The `tmdbtag config` command uses the same validation, prompts, persistence, and
secret handling as the normal startup flow. It always reopens the TMDB fields
and changes only TMDB configuration; it never scans media or starts a copy/move
operation. Use `tmdbtag storage add` and `tmdbtag storage remove` to manage the
saved S3 bucket catalog.

`tmdbtag storage add` collects one named bucket and appends it to the catalog
without changing the other buckets or the TMDB settings. `tmdbtag storage
remove` displays the saved buckets, asks for confirmation, and removes only the
selected entry. Neither storage command contacts TMDB or touches media files.

Help, version, command-help, and invalid-command paths are handled by `clap`
before the wizard. They do not require a TMDB key, a network connection, or a
readable media directory.

## The interactive experience

### Configuration first

Before any media discovery, the CLI asks for:

- **TMDB API key** — entered using masked/password-style input.
- **TMDB metadata language** — initially defaulted to `pt-BR`, with the
  application UI remaining in English.

The normal workflow prompts only for fields that are missing or invalid in the
saved configuration. If one field is present and valid, it is not unnecessarily
requested again. An environment value from `TMDB_API_KEY` or `TMDB_LANGUAGE` can
provide a masked or visible default when the corresponding saved field is
unavailable, but it does not bypass a required prompt.

The API key is stored only in:

```text
~/.tmdbtag/config.json
```

On Unix-like systems, `tmdbtag` uses private permissions for the configuration
directory and file when it creates them. The key is never included in filenames,
previews, plans, logs, errors, or debug output.

### Storage selection and the S3 bucket catalog

After TMDB configuration, the organization wizard asks for two independent
backends:

- **Source storage** — the local filesystem or S3-compatible object storage.
- **Destination storage** — the local filesystem or S3-compatible object
  storage.

The catalog can contain any number of independently named buckets. Adding one
asks for:

- a local storage name used by selectors and `storage remove`;
- an access key;
- a secret key, entered with masked input;
- a bucket name;
- an optional custom endpoint URL; pressing Enter uses
  `https://s3.amazonaws.com`, the standard AWS S3 endpoint;
- a signing region.

Each entry is stored beside the TMDB settings in
`~/.tmdbtag/config.json`. Complete entries are reused without prompting on later
runs. The `storage add` and `storage remove` commands are the supported way to
manage this catalog. Credentials are held in memory only for the current
process after loading and are never rendered in previews, progress messages,
errors, or debug output.

The organization wizard handles the catalog deliberately. If S3 is selected on
either side and the catalog is empty, the wizard requires one bucket to be added
before it can continue. If multiple buckets exist, it displays an independent
selector for each S3 role. This makes all of these workflows possible without
reconfiguring anything:

- local source → S3 destination bucket `archive`;
- S3 source bucket `incoming` → local destination;
- S3 source bucket `incoming` → S3 destination bucket `organized`;
- the same bucket used on both sides with different prefixes.

The S3 endpoint is optional. Leave it empty to use AWS's standard endpoint,
`https://s3.amazonaws.com`. Provide a custom endpoint only when using an
S3-compatible service that requires one, such as a self-hosted or alternative
object-storage provider.

S3 prefixes are selected per organization run and are not part of a saved
bucket entry. When the source is S3, `tmdbtag` asks for `S3 source prefix
(optional; empty = bucket root)`. Discovery then lists objects recursively
under that prefix, and the explorer displays paths relative to it. When the
destination is S3, it asks for `S3 destination prefix (optional; empty = bucket
root)`. Generated object keys are placed under that prefix. The two prompts are
independent, so a source prefix is never accidentally reused as a destination
prefix.

The same organization plan supports all combinations:

| Source | Destination | Transfer behavior                                                  |
| ------ | ----------- | ------------------------------------------------------------------ |
| Local  | Local       | Verified destination-side local copy                               |
| Local  | S3          | Bounded local-to-S3 upload                                         |
| S3     | Local       | S3 download to a local temporary file, then no-replace publication |
| S3     | S3          | Server-side S3 copy for a shared endpoint; verified temporary bridge otherwise |

For `Move`, the destination is always verified first. A local-to-S3 or
S3-to-local move removes the source only after the cross-storage copy has
published successfully. An S3-to-S3 move uses server-side `CopyObject` followed
by a conditional source deletion when both buckets use the same endpoint. When
the selected buckets use different endpoints, `tmdbtag` safely bridges the
transfer through a private local temporary file, verifies the destination, and
deletes the source only after successful publication.

### One explorer for the whole source tree

The file picker is a compact terminal file explorer, not a sequence of unrelated
folder prompts. It displays:

- videos directly in the current directory;
- folders that contain at least one eligible video descendant;
- videos at any depth below those folders.

Folders start collapsed. The default state never silently selects the first
file.

Typical controls are:

| Key       | Action                                                          |
| --------- | --------------------------------------------------------------- |
| `↑` / `↓` | Move the highlight                                              |
| `j` / `k` | Move the highlight on terminals where those keys are convenient |
| `Tab`     | Expand or collapse the highlighted folder                       |
| `Space`   | Select or deselect a video                                      |
| `Enter`   | Confirm the selected videos                                     |
| `Esc`     | Cancel the current interaction                                  |

Only video files can be selected. Folder rows are navigation containers. All
paths shown in the interactive interface are relative to the current source
root, while the application retains the exact paths internally for validation
and execution.

Discovery is deterministic: entries are sorted by relative path, symbolic links
are not followed, and the destination subtree is excluded when the destination
is inside the source root.

### Live TMDB identification

Each selected file is its own identification unit. This matters when a folder
contains several different movies, several episodes, or a mixture of movies and
series.

For each file, the user chooses one of two paths:

1. **Search TMDB by title** — type a query and receive live results after a
   debounce interval.
2. **Enter a TMDB ID manually** — provide the numeric ID and choose whether it
   represents a movie or a series.

The live selector keeps the query and results together. As the query changes,
`tmdbtag` requests updated movie and TV results without requiring a separate
filter-and-submit cycle. Results are keyboard-selectable, clearly labeled as
`[MOVIE]` or `[SERIES]`, and never silently accepted just because they happen to
be first.

The selected item is resolved through TMDB and confirmed before it becomes part
of a move plan. TMDB is the authority for the numeric ID, media type, and title.

For a series, the CLI asks for the season and episode for that specific file. It
validates the episode through TMDB before showing the final plan. No episode or
title is inferred from a vague original filename in the MVP.

## Copy or move, by choice

The operation mode is selected near the beginning of the workflow:

- **Copy** creates a new independent file in the destination and leaves the
  original untouched.
- **Move** organizes the file into the destination and removes the source only
  after the destination has been published successfully.

Both modes use the same plan, naming rules, collision checks, confirmation
screen, and progress reporting. The choice is visible in the preview, so there
is no ambiguity about whether originals will remain.

## Naming that is human-friendly and machine-recoverable

Generated filenames use the reserved field delimiter `__S__`:

### Movies

```text
<tmdb_id>__S__MOVIE__S__<normalized_title>.<lowercase_video_extension>
```

Example:

```text
550__S__MOVIE__S__Fight Club.mkv
```

### Series episodes

```text
<tmdb_id>__S__SERIES__S__S<season>E<episode>__S__<normalized_series_title>.<lowercase_video_extension>
```

Example:

```text
1399__S__SERIES__S__S01E01__S__Game of Thrones.mp4
```

The season and episode are intentionally joined as `S01E01`. This keeps the
metadata fields stable while still allowing future code to split the episode
component at `S` and `E`.

### Normalization rules

Titles come from TMDB in the configured metadata language and are normalized
only at the filename boundary. The normalizer:

- preserves the numeric TMDB ID and episode numbers exactly;
- removes or replaces characters that are invalid or unsafe on supported
  filesystems, including `:`;
- removes control characters and path separators;
- replaces occurrences of the reserved `__S__` token so the title cannot create
  a false metadata field;
- collapses unnecessary whitespace and replacement separators;
- avoids trailing filename noise and platform-reserved names;
- preserves Unicode text when it is safe;
- truncates only the title component when a platform-safe filename length limit
  requires it;
- preserves the original video's extension while emitting it in lowercase.

The title is not used as a parser boundary. `:` may be normalized to a safe
visual separator such as `-`, while `__S__` remains the only metadata field
delimiter.

`tmdbtag` does not add years, codecs, resolutions, release groups, original
filenames, or episode titles in the MVP. It does not invent collision suffixes
such as `(1)`.

### Recovering basic metadata later

The generated filename is intentionally structured for future automation. A
future reader can:

1. remove the final extension;
2. split the stem on `__S__`;
3. read `[id, MOVIE, title]` for a movie;
4. read `[id, SERIES, S01E01, title]` for a series;
5. parse the `S` and `E` markers from the episode field;
6. use the ID to request richer metadata from TMDB when needed.

The title is a display hint. The TMDB ID is the durable lookup key.

## Safety you can see

`tmdbtag` treats local and S3 operations as a two-phase process:

```text
Configure → Select storages → Discover → Identify → Build plan → Validate everything → Preview → Confirm → Execute → Report
```

No file is copied, moved, renamed, or deleted while the user is still
identifying media or while the plan is being assembled.

Before execution, the complete plan is revalidated. This includes source
existence, source type, source snapshots, destination constraints, generated
names, collisions, and relevant filesystem state. A destination that does not
exist may be created only after explicit confirmation, and its actual creation
is deferred until the commit phase.

The local destination cannot be the current source directory, cannot overlap a
selected nested source container, and is excluded from discovery when it is
inside the current source tree. For S3, a source prefix cannot also be the
destination prefix in the same selected bucket, and the destination prefix is
excluded from source listing when both sides address the same bucket and
endpoint.

`tmdbtag` never overwrites an existing destination by default. If a source
disappears or changes, or an unexpected operation fails, execution stops by
default and the final report separates:

- completed operations;
- the failed operation and its safe error category;
- pending operations that were intentionally not attempted.

### Copy safety

Copies stream bytes to a destination-side temporary file. The temporary file is
verified against the source before the final name is published with no-replace
semantics. A failed or interrupted copy leaves the source intact and does not
make a partial file look complete. S3 uploads use a no-replace precondition;
large local uploads use bounded multipart parts, while S3-to-S3 copies use the
service-side copy operation.

### Move safety

Same-volume local moves use a no-replace publication strategy where the platform
permits it, then remove the source only after the destination is known to exist.
Cross-volume and cross-storage moves use the same verified temporary-copy
process as a copy and remove the source only after successful publication. An
S3-to-S3 move is a conditional server-side copy followed by a conditional delete
of the original object.

### Real byte-based progress

The aggregate progress percentage is calculated from bytes:

```text
completed source bytes ÷ total planned source bytes × 100
```

For local transfers and S3 uploads/downloads, progress advances as chunks or
bounded parts are transferred. For server-side S3 copies and safe same-volume
moves, the file's bytes are marked complete after publication succeeds because
no client-side byte stream needs to be transferred. Zero-byte files are treated
as complete once published.

The progress bar also shows a smoothed transfer-rate estimate based on the
reported byte movement. It uses decimal units and displays values such as
`850 B/s`, `12.40 KB/s`, `3.25 MB/s`, `1.10 GB/s`, or `1.00 TB/s`. Operations
that complete through logical publication without a client-side byte stream,
such as a same-volume move or a server-side S3 copy, may show `0 B/s` because
there were no transfer bytes to measure.

## Supported video files

Discovery uses one centralized, case-insensitive allowlist rather than assuming
that every video has the `.mkv` extension. It includes common formats such as:

```text
.mkv  .mp4  .avi  .mov  .webm  .m4v  .mpg  .mpeg  .ts  .m2ts
.wmv  .flv  .ogv  .vob  .mts   .mxf  .3gp  .asf   .rm  .rmvb
```

The allowlist also covers additional formats used by common media workflows.
Matching is case-insensitive, so `VIDEO.MKV` and `video.mkv` are both eligible.
Regular files are eligible; symbolic links are intentionally skipped in the MVP.

## Configuration

The persisted configuration is small and explicit:

```json
{
  "tmdb_api_key": "your-tmdb-api-key",
  "tmdb_language": "pt-BR",
  "s3": [
    {
      "name": "primary",
      "access_key": "your-s3-access-key",
      "secret_key": "your-s3-secret-key",
      "bucket": "your-bucket",
      "endpoint": "https://s3.example.com",
      "region": "us-east-1"
    }
  ]
}
```

Location:

```text
~/.tmdbtag/config.json
```

The `s3` value is an array because each entry represents one independently
selectable bucket. Older single-object S3 configurations can still be read for
compatibility; the former `base_path` value is ignored, and the next write
converts the data to the current catalog format. Choose the desired source and
destination prefixes during each organization run instead.

The language controls the language requested from TMDB for titles and metadata.
It does not translate the CLI. All application-owned prompts, labels, help text,
progress messages, errors, and reports are in English.

To change the TMDB values intentionally:

```bash
tmdbtag config
```

The command asks for both TMDB fields even when a complete configuration already
exists. To add another S3 bucket, use `tmdbtag storage add`; to remove one, use
`tmdbtag storage remove`. Prefixes are intentionally absent from this file:
`S3 source prefix` and `S3 destination prefix` belong to one organization run
and are relative to the selected bucket root.

For local automation or development environments, `TMDB_API_KEY` and
`TMDB_LANGUAGE` may provide fallback defaults for missing saved fields. They do
not cause the normal wizard to skip a required interactive configuration prompt.

## TMDB integration

`tmdbtag` uses TMDB for:

- title search across movies and TV series;
- media-type distinction;
- item details and canonical IDs;
- season and episode validation;
- localized metadata according to the saved language.

Network requests use a finite timeout, the configured language, and typed
handling for authentication errors, rate limits, not-found responses, invalid
payloads, server failures, and timeouts. A file is never placed into the final
plan when the metadata required for its name cannot be verified.

Create or manage a TMDB API key through the official
[TMDB developer documentation](https://developer.themoviedb.org/docs/getting-started).
Use of the TMDB API is subject to
[TMDB's terms and policies](https://www.themoviedb.org/api-terms-of-use).

## What tmdbtag does not do yet

The current MVP intentionally organizes video files and nothing more. It does
not:

- rename folders or build a media-server folder hierarchy;
- rename or move subtitles, images, NFO files, or other auxiliary files;
- download media, transcode video, inspect codecs, or alter file contents;
- infer series episodes from original filenames;
- overwrite existing destination files;
- follow symbolic links during discovery;
- provide a database or a metadata retrieval command;
- provide a non-interactive/batch mode;
- automatically select a TMDB result on the user's behalf.

These boundaries keep the first version predictable and make the naming contract
a dependable foundation for future retrieval and automation features.

## Development

Format, compile, lint, and test the project with:

```bash
cargo fmt --all -- --check
cargo check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

Tests should cover pure transformations first, followed by filesystem behavior
and HTTP behavior behind local test doubles. Tests must not require a real TMDB
key, real media directories, or live network data.

When changing behavior, update this README as part of the same change. It is the
product contract: it should explain what the tool promises to users, what it
refuses to do, how its filenames can be interpreted later, and which safety
guarantees must remain intact.

## License and TMDB attribution

`tmdbtag` is an independent tool and is not endorsed, sponsored, or certified by
TMDB. TMDB data, branding, and API access remain subject to TMDB's own terms.
See the official [TMDB website](https://www.themoviedb.org/),
[API documentation](https://developer.themoviedb.org/docs), and
[API terms of use](https://www.themoviedb.org/api-terms-of-use) for the
authoritative policies.
