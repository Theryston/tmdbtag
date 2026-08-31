# title-tmdb-file

Modern, polished, and highly interactive CLI for organizing .mkv video files using the identifier and title registered in [The Movie Database (TMDB)](https://www.themoviedb.org/).

> Status: MVP in progress. The CLI foundation and persisted startup-configuration foundation are implemented. TMDB verification, filesystem discovery, naming, planning, and file movement remain for the following tasks.

This README is the source of truth for the expected behavior. Any implementation, flow change, or new feature must be compared against this document before it is incorporated.

The command-line contract must be implemented with clap. The normal workflow is a guided terminal experience with clear steps, keyboard-friendly selection, searchable lists, progress feedback, a complete preview, and safe confirmation before any file operation. All source code, help text, prompts, status messages, errors, and documentation must be written in English. The language selected for TMDB metadata is independent from the language of the application interface.

## Table of Contents

- [Objective](#objective)
- [MVP Scope](#mvp-scope)
- [Concepts](#concepts)
- [Main Flow](#main-flow)
- [Interface Contract](#interface-contract)
- [Modern CLI Experience](#modern-cli-experience)
- [Naming Convention](#naming-convention)
- [TMDB Integration](#tmdb-integration)
- [File and Safety Rules](#file-and-safety-rules)
- [How to Retrieve the Data in the Future](#how-to-retrieve-the-data-in-the-future)
- [Planned Architecture](#planned-architecture)
- [Folder Structure](#folder-structure)
- [Configuration and Execution](#configuration-and-execution)
- [Errors and Exit Codes](#errors-and-exit-codes)
- [Acceptance Criteria](#acceptance-criteria)
- [Testing Strategy](#testing-strategy)
- [Implementation Tasks](#implementation-tasks)
- [Roadmap](#roadmap)
- [References](#references)

## Objective

The program is run inside a folder that contains other folders with video files. It must:

1. load the saved TMDB configuration from `~/.title-tmdb-file/config.json` on Unix-like systems, or the equivalent current user's home directory on Windows;
2. ask only for missing or invalid TMDB configuration fields, using secure prompts;
3. ask which folder will be used as the destination;
4. list the folders found in the current directory;
5. allow one or more folders to be selected;
6. list the .mkv files in each selected folder;
7. allow one or more video files to be selected;
8. allow a movie or TV series to be identified by searching TMDB or entering an ID manually;
9. ask for the season and episode when the item is a TV series;
10. show a complete operation plan;
11. move the selected files to the destination folder, renaming them with enough data to locate the item in TMDB again.

The program must not alter the video contents. Its job is to organize: select, rename, and move.

## MVP Scope

### Included

- clap-based command-line parsing and standard --help and --version behavior.
- A modern, polished, highly interactive terminal interface.
- English source code and English user-facing text.
- Execution using the current working directory as the root of the source folders.
- Multiple-folder selection.
- Multiple .mkv-file selection.
- Real-time online search in TMDB.
- Per-user persistence for the TMDB API key and metadata language in `~/.title-tmdb-file/config.json`.
- Conditional startup prompts that ask only for missing or invalid saved fields.
- A `config` command that deliberately reopens both configuration fields.
- Manual identification by numeric TMDB ID.
- Support for movies and TV series.
- Manual season and episode input for each series file.
- Plan preview before any change is made to disk.
- Actual file movement rather than an implicit copy.
- Deterministic, filesystem-safe names.
- Protection against accidental overwrites.
- Per-file final report.

### Initially out of scope

- Renaming or moving folders.
- Creating a subfolder for each movie or series.
- Moving subtitles, images, .nfo files, or other auxiliary files.
- Automatically detecting season and episode from the original filename.
- Changing the codec, resolution, audio, subtitles, or any video content.
- Downloading videos.
- Synchronizing data with an external library.
- Maintaining a local database.
- Overwriting or replacing existing files.
- Processing multiple different movies or series inside the same source folder.
- Automatically traversing nested folders.
- Operating without interactive confirmation.

These items may be considered in the future, but they must not be implemented implicitly.

## Concepts

| Term | Meaning |
| --- | --- |
| Current directory | The directory from which the executable was started. It is the source root in the MVP. |
| Source folder | A direct child folder of the current directory that contains videos to organize. |
| Destination folder | The directory selected after startup configuration, where videos will be moved. It may also be called the target folder. |
| TMDB item | A movie or TV series returned by and confirmed against TMDB. |
| TMDB ID | The numeric identifier of a movie or TV series in TMDB. |
| Movie | A TMDB item of type movie. |
| Series | A TMDB item of type tv. |
| Episode | The combination of a season and episode number for a TV series. |
| Plan | The final mapping between each source file and the name/path it will have in the destination. |
| Configuration file | The per-user JSON file at `~/.title-tmdb-file/config.json` that stores the TMDB API key and metadata language. |

### Unit-of-work rule

Each source folder represents one TMDB item.

- A movie folder must have exactly one selected .mkv file.
- A series folder may have one or more selected .mkv files; each file represents an episode of the same series.
- If different movies or series are in the same folder, the user must separate them into different folders or run the program again for each group.

This rule prevents one prompt from assigning incorrect metadata to files belonging to different works.

## Main Flow

The mandatory MVP flow is:

~~~text
Start and parse command-line arguments with clap
        |
        v
Load ~/.title-tmdb-file/config.json
        |
        v
Ask only for missing TMDB configuration fields
        |
        v
Save any newly completed configuration
        |
        v
Validate TMDB configuration
        |
        v
Choose the destination folder
        |
        v
Select one or more source folders
        |
        v
For each folder:
  select .mkv files
  identify a movie or series in TMDB
  if it is a series, enter season/episode per file
        |
        v
Validate all paths and conflicts
        |
        v
Display the complete preview
        |
        v
Confirm
        |
        v
Move and rename the files
        |
        v
Display the final summary
~~~

The saved TMDB configuration must be loaded before filesystem discovery. If the file is absent, or if either field is absent, empty, or invalid, the corresponding English prompt is shown and the completed values are saved. When both fields are valid in the file, the normal organization workflow does not prompt for them again. The destination folder must then be requested before source folders are listed. This allows it to be excluded from the list when it is a subfolder of the current directory and prevents the destination itself from being selected as a source. clap must process --help, --version, and command-level help before the interactive wizard; those paths exit without asking for credentials or touching the filesystem.

## Interface Contract

The exact prompt wording may be refined during implementation, but the order, decisions, and validations below are mandatory.

### 1. Command-line parsing with clap

clap is mandatory for the public command-line interface. It must own:

- parsing command-line arguments and options;
- --help output;
- --version output;
- invalid-argument diagnostics;
- command metadata and usage examples;
- the boundary between command-line mode and the interactive wizard.

The default invocation must start the interactive workflow:

~~~bash
title-tmdb-file
~~~

The explicit configuration command must be available through the same clap parser:

~~~bash
title-tmdb-file config
~~~

`config` opens the shared TMDB configuration wizard, asks for both fields, and saves the result to
the per-user configuration file. It must not start the media-selection workflow.

The help and version paths must work without a TMDB API key, a network connection, or a readable media directory:

~~~bash
title-tmdb-file --help
title-tmdb-file --version
~~~

Do not parse arguments manually with std::env::args, string matching, or ad-hoc positional conventions. Interactive questions are not a replacement for clap's command-line contract.

### 2. Startup configuration

Before the normal workflow reaches the destination prompt or discovers any media, it must load the
per-user configuration file:

~~~text
~/.title-tmdb-file/config.json
~~~

The home-directory portion is resolved using the host operating system's current-user convention.
The supported JSON fields are:

~~~json
{
  "tmdb_api_key": "your-tmdb-api-key",
  "tmdb_language": "pt-BR"
}
~~~

The normal workflow follows these rules:

- if the file is missing, ask for the API key first and the metadata language second;
- if only the API key is missing or invalid, ask only for the masked API-key field;
- if only the language is missing or invalid, ask only for the language field;
- if both saved fields are valid, do not show either startup prompt;
- save a complete configuration after missing values have been accepted;
- keep all application-owned prompts and messages in English.

When both fields need to be collected, the API-key question is always first. The prompts happen
before the destination prompt, source-folder discovery, or video-file discovery.

Rules for the API-key prompt:

- use a masked/password-style input;
- never echo the key;
- require a non-empty key;
- use the saved key as the masked default when it exists;
- use `TMDB_API_KEY` as a masked default only when the saved key is unavailable;
- never display the key in a preview, error, debug representation, log, or report;
- validate the key against TMDB before proceeding to filesystem selection;
- allow retry or cancellation when the key is rejected.

Rules for the language prompt:

- show a searchable list of common TMDB language/locale codes;
- allow a supported code to be entered manually when needed;
- use `pt-BR` as the initial default;
- use the saved language before considering `TMDB_LANGUAGE` as a default;
- validate or normalize the selected code before making the first metadata request;
- keep the application UI in English regardless of the selected TMDB language.

The `config` command always asks for both fields, even when the configuration file is complete.
Existing values may be shown as editable defaults, and pressing Enter for the masked key may reuse
the saved value. A canceled update must leave the existing file unchanged.

The selected language affects metadata returned by TMDB, especially the title used in the generated filename. It does not translate the CLI itself.

### 3. Initialization

At startup:

- obtain the current working directory;
- verify that it exists and can be read;
- do not assume that the executable's folder is the source folder;
- do not change anything before final confirmation.

Expected usage:

~~~bash
cd /path/to/input-folder
title-tmdb-file
~~~

### 4. Choosing the destination folder

The first filesystem-related prompt must ask for the destination folder path, after the TMDB API key and language have been configured.

Rules:

- accept an absolute path or a path relative to the current directory;
- normalize the path before using it;
- accept an existing folder;
- if the folder does not exist, explicitly ask whether it should be created;
- reject a path that exists as a file;
- reject the current directory as the destination;
- reject a selected source folder as the destination;
- when the destination is inside the current directory, do not list it as an available source folder;
- do not create the folder while the path is being entered; creation may happen only after validation and the user's confirmation.

Once defined, the destination must remain visible throughout the rest of the flow.

### 5. Selecting source folders

List the direct child folders of the current directory.

Listing rules:

- include real directories;
- do not include files;
- do not follow symbolic links in the MVP;
- sort deterministically, preferably case-insensitively;
- show the name and, when useful, the relative path;
- exclude the destination when it is inside the current directory;
- allow one or more folders to be selected;
- require at least one folder to continue;
- allow cancellation without modifying any files.

There is no recursive folder selection in the MVP. Only folders immediately inside the current directory participate in this step.

### 6. Selecting video files

For each source folder, list the eligible video files.

Listing rules:

- consider only regular files;
- consider the .mkv extension case-insensitively (.mkv, .MKV, .Mkv, and so on);
- look only for files directly inside the source folder;
- do not include nested folders;
- do not follow symbolic links;
- sort files deterministically;
- show at least the filename; size and relative path are recommended;
- allow one or more files to be selected;
- require at least one file for the folder to continue;
- if there are no .mkv files, explain why and allow the user to cancel or go back.

The program must not automatically select the first file. The choice must be explicit.

### 7. Identifying the item in TMDB

After the files for a folder are selected, present two options:

1. search by text;
2. enter a TMDB ID directly.

#### Text search

The prompt must:

- accept the entered text;
- search movies and TV series;
- display paginated results or a limited result set;
- clearly distinguish the result type;
- show at least the ID, type, title, and year when available;
- allow a result to be selected;
- allow the search to be repeated;
- never silently choose the first result.

Example presentation:

~~~text
Results for: the office

1. [SERIES] 2316  The Office                 (2001)
2. [SERIES] 2315  The Office                 (1995)
3. [MOVIE]  ...   The Office                 (year)
~~~

After a result is selected, the program must fetch the item's details and show an identification confirmation before building the plan.

#### Manually entered ID

When the user chooses to enter an ID:

- ask for the type (movie or series) before the ID, to remove ambiguity between namespaces;
- accept only a positive numeric ID;
- fetch details for the selected type from TMDB;
- reject IDs that do not exist or do not match the selected type;
- show the returned title and ask for confirmation;
- never accept a manually entered title as a substitute for TMDB data;
- if the confirmed type is movie and more than one file was selected, reject the plan and request a selection containing exactly one file.

### 8. Series episode data

For each selected .mkv file belonging to a series, ask for:

- the season number;
- the episode number.

Rules:

- allow season 0 for specials when TMDB accepts that combination;
- accept non-negative integers;
- validate the episode against TMDB data before the preview;
- do not accept the same series + season + episode combination twice in one execution;
- ask for the data individually even when multiple files belong to the same series;
- allow the input to be corrected before the preview;
- do not infer these numbers from the original filename in the MVP.

Example:

~~~text
Confirmed series: Game of Thrones (TMDB 1399)

File: episode-01.mkv
Season: 1
Episode: 1

File: episode-02.mkv
Season: 1
Episode: 2
~~~

The title used in the filename is the series title returned by TMDB, not the individual episode title. The series ID combined with SxxEyy identifies the episode.

### 9. Plan preview

Before moving any file, display every operation that will be performed:

~~~text
Destination: /library/organized

SOURCE                                                DESTINATION
/input/movies/Fight Club.mkv                          /library/organized/550 - Fight Club.mkv
/input/series/episode-01.mkv                          /library/organized/1399 - S01E01 - Game of Thrones.mkv
/input/series/episode-02.mkv                          /library/organized/1399 - S01E02 - Game of Thrones.mkv
~~~

The preview must show:

- the destination folder;
- the total number of files;
- the full source path for every file;
- the full destination path for every file;
- the TMDB ID;
- the item type;
- season and episode when applicable;
- detected conflicts or warnings.

If there is a validation error or conflict, confirmation of the plan must not be allowed until the issue is corrected or the group is canceled.

### 10. Confirmation and result

The final prompt must be explicit, for example:

~~~text
Move and rename 3 files? [y/N]
~~~

The default must be do not execute (N).

If the user declines:

- move nothing;
- rename nothing;
- delete nothing;
- report that the operation was canceled.

If the user confirms:

- execute only the plan that was displayed;
- show progress per file;
- report success or failure for each move;
- display totals at the end.

## Modern CLI Experience

The application must feel like a polished modern terminal product, not like a collection of raw console questions. The visual design can evolve, but the quality bar is explicit: the interface must be clear, responsive, keyboard-friendly, consistent, and safe.

### Command-line layer

clap is responsible for the command-line contract. The interactive wizard is the default command, while clap owns the standard command-line behavior around it.

The initial command-line experience must provide:

- a clear application name and description;
- --help with a concise overview, usage, options, and examples;
- --version with the package version;
- consistent invalid-argument errors;
- a stable exit-code contract;
- room for future subcommands and non-interactive modes without rewriting the wizard;
- no API-key prompt when the user asks only for --help or --version.

The interactive wizard must not replace clap parsing, and clap argument parsing must not be duplicated inside prompt handlers.

### Visual hierarchy

The terminal UI should include, where supported by the chosen interaction library:

- a small branded header containing the application name and version;
- a visible step indicator such as Configuration, Destination, Sources, Metadata, Preview, and Execute;
- clear section titles;
- consistent success, warning, error, and informational styles;
- aligned tables or panels for source and destination paths;
- a visible count of selected folders and files;
- a clear indication of the current source folder while processing a batch;
- a concise final summary;
- a graceful fallback when color, Unicode, or advanced terminal features are unavailable.

Color and symbols may improve the experience, but safety-critical information must remain understandable without color alone.

### Interaction quality

Interactive controls should support:

- keyboard navigation;
- visible selection state;
- multiple selection with an obvious toggle action;
- search or filtering for long folder, file, and TMDB result lists;
- back navigation when a previous decision can be safely edited;
- Escape or an equivalent cancellation action;
- confirmation before destructive changes;
- helpful empty states;
- retry for recoverable network or input errors;
- non-blocking-looking feedback during network requests;
- progress feedback during large file operations.

The UI must never appear frozen during a network request. A spinner or status line should explain whether it is searching, loading details, validating an episode, or preparing the move plan.

### English interface

All application-owned text must be in English:

- clap descriptions and help output;
- headers and step labels;
- prompts and option labels;
- validation errors;
- API and filesystem errors;
- progress messages;
- completion summaries;
- debug labels and developer-facing diagnostics.

Titles and other metadata returned by TMDB are external content and may appear in the language selected by the user. That does not change the language of the application interface.

### Responsive terminal behavior

The interface must remain usable in small and large terminals:

- avoid assuming an 80-column terminal;
- truncate or wrap long paths deliberately;
- never hide the final filename or conflict state;
- keep tables readable when paths are long;
- avoid emitting unreadable escape sequences when output is redirected;
- detect non-interactive output and fail with an actionable message until a documented non-interactive mode exists;
- do not require a mouse;
- preserve useful behavior when color is disabled.

### Performance perception

The CLI should feel fast even when the work is not instantaneous:

- show immediate feedback after each action;
- use a progress indicator for network and file operations;
- reuse the TMDB HTTP client;
- use bounded requests and bounded retries;
- avoid rescanning a folder unnecessarily;
- avoid repeating identical metadata requests during one run;
- do not sacrifice correctness for a faster-looking result;
- never perform a hidden filesystem mutation to improve perceived speed.

## Naming Convention

The final filename must contain the TMDB ID and the title returned by the API after mandatory filename normalization. The final extension is always .mkv in lowercase.

| Type | Format | Example |
| --- | --- | --- |
| Movie | &lt;id&gt; - &lt;normalized_title&gt;.mkv | 550 - Fight Club.mkv |
| Series | &lt;id&gt; - S&lt;season&gt;E&lt;episode&gt; - &lt;normalized_series_title&gt;.mkv | 1399 - S01E01 - Game of Thrones.mkv |

### Composition rules

- use the numeric TMDB ID, without a tmdb prefix;
- separate components with " - ";
- do not include the year, codec, resolution, language, release group, or original filename;
- use the localized title returned by TMDB, after mandatory filename normalization;
- if no localized title is available, use the original title returned by the API;
- preserve accents and safe Unicode characters;
- write the season and episode with at least two digits (S01E02, S10E03); larger numbers must not be truncated;
- do not include the episode title in the MVP;
- do not include information that was not obtained from the confirmed TMDB item.

### Mandatory title normalization

The raw TMDB title must never be placed directly into a filename. Every title must pass through one deterministic function, conceptually named normalize_title_for_filename, before the final filename is assembled.

The normalization pipeline is:

1. preserve the original TMDB title for display and metadata;
2. trim leading and trailing Unicode whitespace;
3. replace filesystem-invalid characters and control characters, including /, \, :, *, ?, ", <, >, and |, with a safe separator;
4. use a readable separator such as " - " for replacements, so Mission: Impossible becomes Mission - Impossible;
5. collapse accidental repeated spaces or replacement separators;
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

Normalization may change only the title component. It must never change the TMDB ID, media type, season, episode, extension, or the original metadata value held in memory.

### Collisions

The destination path is calculated before execution. The following are conflicts:

- two files in the same plan produce the same destination;
- the destination already exists;
- the source and destination are the same file;
- two files receive the same series + season + episode combination.

The default behavior for any conflict is to block confirmation and request a correction. The program must not:

- overwrite;
- add arbitrary suffixes such as (1);
- delete the existing file;
- silently choose another name.

Support for overwriting or automatic conflict resolution must be a future, explicit decision.

## TMDB Integration

### Startup configuration and credentials

The normal interactive run must load the TMDB API key and metadata language from the per-user
configuration file before asking for the destination or discovering any folders. The API key prompt,
when required, must be masked and must never echo the secret.

The startup configuration sequence is:

1. resolve `~/.title-tmdb-file/config.json` in the current user's home directory;
2. load the JSON file, treating a missing file as an empty configuration;
3. ask only for missing or invalid fields, with the API key before the language when both are needed;
4. save the complete configuration after successful local validation;
5. validate the key and language with TMDB;
6. only then continue to destination and source-folder selection.

The `title-tmdb-file config` command deliberately skips the media workflow and reopens both fields so
the user can replace the saved values. It uses the same prompt, validation, and persistence code as
the normal startup path.

The environment may provide defaults for convenience:

~~~bash
# Optional masked default for the first startup prompt
export TMDB_API_KEY="your-key"

# Optional default for the second startup prompt
export TMDB_LANGUAGE="pt-BR"
~~~

Environment values are fallback defaults only when the corresponding saved field is unavailable.
They do not bypass a required prompt. A complete saved configuration takes precedence over both
environment variables during a normal run.

Rules:

- persist the API key only in the documented per-user configuration file after the user accepts it;
- keep the API key in memory only while the current execution is using it;
- never show it in a preview, error, debug representation, log, or report;
- never place it in a filename, operation plan, or any persistent state other than the documented configuration file;
- validate the key before filesystem selection;
- allow retry or cancellation when validation fails;
- keep all API-key handling out of the domain and filename modules;
- do not put credentials directly in Cargo.toml, the source code, or this README.

The selected TMDB language must be chosen through an English prompt. It controls the language of metadata returned by TMDB, especially the title used in generated filenames; it does not translate the CLI.

### Configuration file safety

The configuration file contains the API key in JSON because the CLI must reuse it on later runs. The
application should create the containing directory with owner-only permissions where the platform
supports them and should create the file with owner read/write permissions (`0700` for the directory
and `0600` for the file on Unix-like systems). The application must never print the file contents,
include the key in diagnostics, or commit the file to the repository. A malformed file must stop the
normal workflow with an actionable error; `title-tmdb-file config` may replace it after the user
explicitly completes the configuration prompts.

### Planned endpoints

The integration must use the TMDB v3 API with the user-selected language and include_adult=false for searches. The initial default is pt-BR.

Required operations:

| Operation | Planned endpoint | Purpose |
| --- | --- | --- |
| Search movies | GET /3/search/movie | Find movie candidates. |
| Search series | GET /3/search/tv | Find TV series candidates. |
| Movie details | GET /3/movie/{movie_id} | Confirm the ID and obtain the final title. |
| Series details | GET /3/tv/{series_id} | Confirm the ID and obtain the final title. |
| Episode details | GET /3/tv/{series_id}/season/{season_number}/episode/{episode_number} | Validate the season/episode combination. |

Search results must be filtered to movies and TV series. People, keywords, and other types must not appear as identification options.

### Request rules

- every search must be performed in real time;
- do not maintain persistent caching in the MVP;
- an in-memory cache may be used during one execution to avoid repeating the same request;
- use a finite timeout;
- handle authentication failures, missing items, rate limits, server errors, and lack of network access;
- do not proceed with an item whose details were not confirmed;
- when there are too many results, limit the displayed set and allow a new search;
- the displayed text may be localized, but the ID returned by TMDB is authoritative.

A network failure during identification must leave the source file untouched. The application must not move a file without being able to build and validate its final name.

### Attribution

The product must display the attribution required by TMDB in the interface or distribution documentation, according to the current rules. Official references are listed in the [References](#references) section.

## File and Safety Rules

### The operation is a move

The expected result is:

~~~text
source file --(move + rename)--> destination file
~~~

After a successful operation, the original file must no longer remain in the source folder. The video contents must not be re-encoded or modified.

### Mandatory pre-validation

Before confirmation, validate every item:

- the source folder still exists;
- each file still exists and is the same file that was selected;
- each file is still a regular .mkv file;
- the destination exists or can be created;
- the destination is writable;
- no calculated destination already exists;
- no destination is duplicated within the plan;
- no file was selected twice;
- all IDs and metadata were confirmed by TMDB;
- all series season/episode combinations are valid;
- no path exceeds relevant operating-system limits.

If pre-validation fails, do not move any file in that plan.

### Moving between volumes

When the source and destination are on the same filesystem, prefer an atomic rename/move operation.

When they are on different volumes, the implementation must:

1. copy to a temporary file inside the destination folder;
2. verify that the copy completed;
3. rename the temporary file to the final name;
4. remove the original file only afterward;
5. remove the temporary file if any step fails.

The temporary file must never appear under the final name before the copy is ready. If the copy cannot be verified, preserve the source and report the failure.

### Failures during execution

The complete plan must be pre-validated, but execution does not need to be transactional across volumes.

If an unexpected failure occurs after some files have already moved:

- stop new moves by default;
- do not overwrite or delete destinations;
- report which files completed, which failed, and which remain pending;
- keep unprocessed files in the source;
- allow a later execution to continue after the issue is fixed, treating existing destinations as conflicts.

### Cancellation

Canceling at any prompt before confirmation causes no changes. During a move that has already started, the application must finish the current file operation or fail safely; it must not leave the source and destination in an ambiguous state without reporting it.

## How to Retrieve the Data in the Future

There will be no local database in the MVP. The filename is the minimum index for locating the data again.

### Planned parsing

The future parser must recognize:

~~~text
Movie:   ^(?<id>[0-9]+) - (?<title>.+)\.mkv$
Series:  ^(?<id>[0-9]+) - S(?<season>[0-9]+)E(?<episode>[0-9]+) - (?<title>.+)\.mkv$
~~~

The actual expression must treat the extension case-insensitively when reading external files, although the program always writes .mkv in lowercase.

The parser must produce a reference equivalent to:

~~~text
{
  tmdb_id: 1399,
  media_type: tv,
  season: 1,
  episode: 1,
  title_hint: "Game of Thrones"
}
~~~

Authority rules:

- the TMDB ID is the source of truth;
- season and episode are the source of truth for an episode;
- the title in the filename is a readable copy and may become outdated;
- a future synchronization command must query TMDB again by ID, rather than trusting only the title text;
- the type may be inferred from the presence of SxxEyy, but ID lookups must still validate the type through the API.

## Planned Architecture

The interactive interface must be separated from business rules. clap is mandatory for command-line parsing, help, version output, and argument validation. A dedicated interactive terminal layer should provide prompts, multiple selection, confirmation, progress, and visual presentation. Its exact library may be selected during implementation, but it must remain behind the CLI boundary and must not replace clap.

Expected responsibilities:

~~~text
clap command layer
        |
        v
interactive terminal layer
        |
        v
Organization use case
        |
        +--> filesystem adapter
        +--> TMDB client
        +--> filename generator/parser
        +--> plan validator and executor
~~~

Principles:

- the interface layer collects choices and displays state;
- the domain must not depend on prompts;
- the TMDB client must not move files;
- the filesystem adapter must not decide which title to use;
- the filename generator must be deterministic and testable without a network;
- the executor must perform only an already validated and confirmed plan;
- errors must be typed well enough to produce useful messages without exposing credentials.

## Folder Structure

### Current state

~~~text
title-tmdb-file/
├── Cargo.toml
├── Cargo.lock
├── README.md
├── AGENTS.md
├── tasks/
│   ├── README.md
│   ├── 01-cli-foundation-and-interactive-shell.md
│   ├── 02-tmdb-configuration-and-identification.md
│   ├── 03-filesystem-discovery-and-media-selection.md
│   ├── 04-naming-normalization-and-metadata-recovery.md
│   └── 05-plan-preview-and-safe-file-movement.md
└── src/
    ├── main.rs
    ├── app.rs
    ├── cli.rs
    ├── config.rs
    ├── domain.rs
    ├── error.rs
    └── ui.rs
~~~

### Suggested target structure

~~~text
title-tmdb-file/
├── Cargo.toml
├── Cargo.lock
├── README.md
├── AGENTS.md
├── tasks/
│   ├── README.md
│   ├── 01-cli-foundation-and-interactive-shell.md
│   ├── 02-tmdb-configuration-and-identification.md
│   ├── 03-filesystem-discovery-and-media-selection.md
│   ├── 04-naming-normalization-and-metadata-recovery.md
│   └── 05-plan-preview-and-safe-file-movement.md
├── src/
│   ├── main.rs              # binary entry point and exit code
│   ├── app.rs               # orchestration of the complete flow
│   ├── cli.rs               # clap parser and terminal renderer
│   ├── config.rs            # per-user JSON config, prompts, and local validation
│   ├── domain.rs            # media, selection, and plan types
│   ├── error.rs             # application errors
│   ├── ui.rs                # renderer-neutral terminal interaction contracts
│   ├── filesystem.rs        # discovery, validation, and safe movement
│   ├── naming.rs            # title normalization, filename generation, and parsing
│   └── tmdb/
│       ├── mod.rs
│       ├── client.rs        # HTTP requests and timeouts
│       └── models.rs        # API responses and internal models
└── tests/
    ├── naming.rs
    ├── parser.rs
    ├── filesystem.rs
    └── fixtures/
~~~

Task 01 currently implements `clap` for command parsing, `dialoguer` for password/text/select/multi-select controls, and `indicatif` for activity feedback. These libraries are implementation details behind the CLI/UI boundary and may be replaced only after the interaction contract and user experience are preserved.

This is a suggested organization, not a requirement to create every file immediately. The important rule is to keep the UI, TMDB, filesystem, and filename-composition concerns decoupled.

## Configuration and Execution

### Prerequisites

- Rust compatible with the edition defined in Cargo.toml;
- an interactive terminal;
- an internet connection for TMDB queries;
- a TMDB API key configured through the first-run prompts or `title-tmdb-file config`;
- read permissions for sources and write permissions for the destination.

### Development execution

~~~bash
cargo run
~~~

Open or update the saved TMDB configuration without starting media selection:

~~~bash
cargo run -- config
~~~

Optimized execution, once the binary is implemented:

~~~bash
cargo run --release
~~~

The MVP is interactive and does not require source path arguments: the current directory is used automatically. The normal command reuses a complete configuration from `~/.title-tmdb-file/config.json`; use `config` when both saved values need to be edited. clap must still provide --help and --version. Non-interactive options such as --source, --destination, or --dry-run may be added only after a separate specification exists for them.

The command-line help and version paths can be exercised during development with:

~~~bash
cargo run -- --help
cargo run -- --version
~~~

### Secrets

The CLI stores the accepted TMDB API key in the per-user file `~/.title-tmdb-file/config.json` so
later runs can start without asking for it again. This file is local application state, not a
repository file:

- never commit it or copy it into the project;
- do not print its contents or include the key in logs, errors, previews, or debug output;
- keep the directory and file owner-only where the operating system supports permissions;
- use `title-tmdb-file config` to replace the saved values;
- environment variables are optional fallback defaults for missing fields, not a second persistent
  configuration store.

## Errors and Exit Codes

Messages must be short, actionable, and must not expose tokens.

Minimum categories:

- unreadable current directory;
- unavailable, unreadable, malformed, or unwritable per-user configuration file;
- invalid destination path;
- folder with no eligible .mkv files;
- invalid ID, season, or episode input;
- missing or rejected credential;
- item not found in TMDB;
- network error or API rate limit;
- missing source file;
- destination already exists;
- insufficient permissions;
- failure while moving between volumes.

Planned codes:

| Code | Meaning |
| ---: | --- |
| 0 | Operation completed or was canceled before any change. |
| 1 | API, filesystem, or execution failure; some items may have completed and others may remain pending. |
| 2 | Invalid usage/configuration or pre-validation failure with no changes. |

A detailed error backtrace may be enabled during development, but normal output should prioritize a message that is understandable to the person organizing the files.

## Acceptance Criteria

The MVP is complete only when all of the following criteria are met:

- [ ] Use clap for command-line parsing, --help, --version, and argument diagnostics.
- [ ] Keep all application-owned code and user-facing text in English.
- [ ] Provide a polished, keyboard-friendly, searchable, and responsive interactive terminal experience.
- [ ] Load `~/.title-tmdb-file/config.json` before the normal interactive workflow.
- [ ] Ask for the TMDB API key first only when the saved API key is missing or invalid.
- [ ] Ask for the TMDB metadata language next only when the saved language is missing or invalid.
- [ ] Skip both startup prompts when both saved fields are valid.
- [ ] Persist a complete configuration without exposing the API key in application output.
- [ ] Provide `title-tmdb-file config` to deliberately edit and save both fields.
- [ ] Validate the API key and language before filesystem discovery.
- [ ] Start in the current directory without requiring a separate source-folder configuration.
- [ ] Ask for the destination after startup configuration and before listing sources.
- [ ] List only direct child folders as source options.
- [ ] Allow multiple-folder selection.
- [ ] List only direct .mkv files from each selected folder.
- [ ] Allow multiple-file selection.
- [ ] Apply the one-TMDB-item-per-folder rule.
- [ ] Search movies and series in real time.
- [ ] Allow the user to enter an ID and type manually.
- [ ] Display the type and title before using the data.
- [ ] Ask separately for season and episode for each series file.
- [ ] Generate exactly the documented filename pattern.
- [ ] Normalize titles for filenames, including replacing invalid characters such as colon, without losing the ID, season, or episode.
- [ ] Detect collisions before execution.
- [ ] Never overwrite an existing destination.
- [ ] Show a complete preview.
- [ ] Require explicit confirmation with a negative default.
- [ ] Move the file while keeping its contents intact.
- [ ] Preserve the source when an unverified cross-volume copy fails.
- [ ] Report success, failure, and pending items per file.
- [ ] Allow the ID, type, and episode to be recovered from the generated filename.
- [ ] Have automated tests for naming, parsing, validation, and safe movement.

## Testing Strategy

### Unit tests

- sanitization of invalid characters;
- titles with accents and Unicode;
- Windows-reserved names;
- season and episode padding;
- movie and series filename generation;
- parsing generated filenames;
- rejection of invalid IDs;
- collision detection.

### Filesystem tests

Use temporary directories to verify:

- discovery of direct folders only;
- case-insensitive .mkv filtering;
- exclusion of the destination from source options;
- same-volume movement;
- existing-destination behavior;
- source preservation on failure;
- absence of overwrites.

### TMDB client tests

- use simulated HTTP responses;
- test movie and series searches;
- test lookup by ID;
- test valid and invalid episodes;
- test missing credentials, 401, 404, 429, 5xx, and timeout;
- do not depend on the real API in automated tests.

### Manual acceptance test

Before a usable release, test at least:

1. one folder containing one movie;
2. one folder containing multiple episodes of a series;
3. more than one folder in the same execution;
4. a destination outside the current directory;
5. a destination inside the current directory;
6. a title with characters invalid for filenames;
7. cancellation during selection and confirmation;
8. a destination that already contains an identical name;
9. source and destination on different volumes, when the environment allows it.

## Implementation Tasks

The implementation scope is intentionally divided into five cohesive tasks rather than many small tickets. Each task owns one meaningful capability and includes its own implementation boundaries, tests, safety requirements, and acceptance checklist.

The recommended task order and dependencies are documented in [`tasks/README.md`](tasks/README.md):

1. [CLI foundation and interactive terminal shell](tasks/01-cli-foundation-and-interactive-shell.md)
2. [TMDB configuration and identification](tasks/02-tmdb-configuration-and-identification.md)
3. [Filesystem discovery and media selection](tasks/03-filesystem-discovery-and-media-selection.md)
4. [Naming normalization and metadata recovery](tasks/04-naming-normalization-and-metadata-recovery.md)
5. [Plan, preview, and safe file movement](tasks/05-plan-preview-and-safe-file-movement.md)

These task files are execution guidance derived from this README. They do not replace this product contract. If implementation reveals a necessary behavior change, update the README, the relevant task, and AGENTS.md together before treating the change as intentional.

## Roadmap

The detailed implementation breakdown is maintained in [Implementation Tasks](tasks/README.md). The roadmap below provides the higher-level product phases; the task files provide the actionable scope within those phases.

### Phase 0 — Specification

- [x] Define the objective, flow, filename rules, and MVP limits.
- [x] Document the API contract and safety rules.

### Phase 1 — Interactive skeleton

- [x] Implement the clap command parser and verify its help/version output.
- [x] Choose and validate the dedicated interactive terminal UI library.
- [ ] Implement discovery of the current directory and destination.
- [ ] Implement multiple-folder and .mkv selection.
- [ ] Implement cancellation and local validation.

### Phase 2 — TMDB

- [x] Implement per-user credential persistence and the shared configuration wizard.
- [ ] Implement movie and series searches.
- [ ] Implement ID confirmation.
- [ ] Implement details and episode validation.

### Phase 3 — Plan and movement

- [ ] Implement domain models.
- [ ] Implement sanitization and filename generation.
- [ ] Implement preview and conflict detection.
- [ ] Implement same-volume movement.
- [ ] Implement safe cross-volume copying.
- [ ] Implement the final report.

### Phase 4 — Retrieval and extensions

- [ ] Implement parsing of generated filenames.
- [ ] Add a separate command to query metadata from a filename.
- [ ] Evaluate non-interactive mode and --dry-run.
- [ ] Evaluate nested folders, multiple titles per folder, subtitles, and auxiliary files.
- [ ] Evaluate undo/operation logs without changing the MVP's safe behavior.

## References

- [TMDB — application authentication](https://developer.themoviedb.org/docs/authentication-application)
- [TMDB — API getting started](https://developer.themoviedb.org/reference/intro/getting-started)
- [TMDB — movie search](https://developer.themoviedb.org/reference/search-movie)
- [TMDB — TV search](https://developer.themoviedb.org/reference/search-tv)
- [TMDB — movie details](https://developer.themoviedb.org/reference/movie-details)
- [TMDB — TV series details](https://developer.themoviedb.org/reference/tv-series-details)
- [TMDB — episode details](https://developer.themoviedb.org/reference/tv-episode-details)
- [TMDB — FAQ and general API rules](https://developer.themoviedb.org/docs/faq)

The name “TMDB” must be used consistently. The application must follow the current authentication, attribution, rate-limit, and API-use rules described in the official documentation.
