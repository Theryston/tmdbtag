# Task 03 — Filesystem Discovery and Media Selection

**Status:** Completed — deterministic, non-mutating filesystem discovery, destination validation, recursive multi-format video selection, relative path presentation, and scripted-UI tests are implemented.
**Priority:** P0
**Dependencies:** Task 01
**Blocks:** Task 05

## Objective

Implement safe, deterministic discovery of source folders and recognized video files, together with the interactive selections that determine what the later planning task will process. Source folders are direct children of the current working directory, while video files are discovered recursively inside each selected folder. The destination is chosen after startup TMDB configuration and before source folders are listed.

This task reads and validates filesystem state. It must not rename or move media. It must also avoid accidentally traversing more of the filesystem than the product explicitly allows.

## Implementation delivered

The task is implemented across the following boundaries:

- `src/filesystem.rs` resolves the current working directory, validates absolute and relative destinations without creating missing directories, excludes overlapping destinations, discovers direct source folders, recursively discovers regular files with case-insensitive recognized video extensions, skips symbolic links, sorts results deterministically, and reports non-fatal discovery warnings.
- `src/domain.rs` defines `SourceRoot`, `SourceFolder`, `VideoFile`, `DestinationSelection`, `SelectedSource`, and `FilesystemSelection` so later planning code receives typed paths rather than display strings.
- `src/error.rs` defines actionable filesystem failure categories and maps them to the documented CLI exit-code policy without exposing credentials or raw implementation details.
- `src/ui.rs` extends the renderer-neutral interaction boundary with destination, source-folder, and video-file selection operations. The terminal adapter displays source and destination paths relatively while retaining exact paths in typed values, and continues to own dialoguer rendering and English wording.
- `src/app.rs` connects the validated TMDB startup stage to the destination and selection wizard, handles empty/retry/cancel paths, preserves folder associations, and never mutates the filesystem during selection.

The normal command now returns a `MediaSelectionReady` outcome after this task's non-mutating stage. TMDB item identification, filename generation, planning, confirmation, and movement remain later workflow steps.

## Required outcome

After the TMDB API key and metadata language have been collected and validated:

1. obtain the current working directory;
2. ask for and validate the destination path;
3. list direct child source folders, excluding the destination when applicable;
4. let the user select one or more source folders;
5. for each selected folder, recursively list regular files with recognized video extensions;
6. let the user select one or more files for that folder;
7. return typed selections to the application without mutating the filesystem.

The UI may show metadata such as file size, but all filesystem paths shown to the user must be relative. Source folders are relative to the current source root, and video files are relative to their selected source folder. The selection result must retain the exact source path needed for later revalidation.

## Scope

### 1. Resolve the source root

- Use the current working directory, not the executable's directory, as the source root.
- Fail with an actionable English message if the current directory cannot be obtained or read.
- Do not change the process working directory as a side effect of discovery.
- Do not begin filesystem discovery before TMDB startup configuration has been validated.
- Do not modify any file or directory before final confirmation in Task 05.

### 2. Choose and validate the destination

The destination prompt is the first filesystem-related interaction. It must occur before source folders are listed.

Accept:

- absolute paths;
- paths relative to the current directory;
- existing directories;
- nonexistent directory paths only when the user explicitly agrees that the directory may be created later after plan validation and final confirmation.

Reject:

- a path that exists as a regular file;
- the current directory itself;
- a selected source folder;
- an unresolved or invalid path;
- a destination that cannot be represented safely by the supported platform.

Normalize and resolve the destination consistently before comparing it with source folders. If the destination is inside the current directory, exclude it from the source-folder list. Keep the selected destination visible in later UI stages.

Do not create a nonexistent destination merely because the user typed it. Creation belongs to the confirmed execution path and must be guarded by the final plan validation.

The implementation must handle path comparison without concatenating strings manually. Use platform-aware path operations and account for equivalent path representations where practical.

### 3. Discover source folders

List only direct child entries of the current directory. Include only real directories that are eligible to be selected.

Rules:

- do not include regular files;
- do not recurse into nested directories;
- do not follow symbolic links in the MVP;
- exclude the destination when it is inside the source root;
- sort entries deterministically, preferably case-insensitively with a stable tie-breaker;
- retain exact paths even when the UI shows shortened labels;
- permit one or more selections;
- require at least one selected folder to continue;
- explain an empty source list and allow cancellation or return.

The discovery layer should distinguish an inaccessible entry from an entry that simply does not qualify. The UI should explain the relevant condition without dumping OS-specific debug details by default.

### 4. Discover eligible video files

For every selected source folder, recursively list regular files whose extension is in the
centralized video-extension allowlist, case-insensitively. The initial allowlist includes common
formats such as `.mkv`, `.mp4`, `.avi`, `.mov`, `.webm`, `.m4v`, `.ts`, `.m2ts`, `.wmv`, and `.flv`.
The allowlist lives in the filesystem adapter so the supported policy remains explicit and easy to
extend.

Rules:

- recurse into real nested folders;
- do not follow symbolic links;
- do not include directories with a video-looking suffix;
- sort deterministically by the path relative to the selected source folder;
- show the relative path and, when useful, the file size;
- allow one or more explicit selections;
- never select the first item automatically;
- require at least one selected file for a selected folder;
- if no eligible file exists, explain why and offer cancel/back behavior.

The selection result must include the source folder association. This association is necessary to enforce the one-TMDB-item-per-source-folder rule later.

### 5. Support correction and cancellation

The user must be able to cancel before any mutation. Where the UI supports back navigation, it should allow the user to correct the destination, source-folder selection, or file selection before planning.

If the filesystem changes while the wizard is open, later tasks must revalidate the selected paths immediately before confirmation. This task should not assume that a path remains valid merely because it was visible during discovery.

## Domain data returned by this task

Return typed values equivalent to:

```text
SourceRoot {
    path: absolute or normalized path,
}

DestinationSelection {
    path: normalized path,
    exists: boolean,
    may_create_after_confirmation: boolean,
}

SelectedSource {
    folder: path,
    files: list of selected regular video-file paths,
}
```

The exact Rust representation may differ. It must not be a list of display strings that later code has to reinterpret as paths.

## Explicit non-goals

Do not implement in this task:

- TMDB authentication or searches;
- movie/series identification;
- season or episode entry;
- filename normalization or parsing;
- plan-level collision resolution;
- file creation, renaming, copying, or movement;
- subtitle or auxiliary-file discovery;
- automatic filename parsing;
- symbolic-link traversal;
- automatic first-item selection;
- recursively selecting source folders beyond the direct children of the current directory.

## Safety requirements

- Treat all paths as untrusted input, including paths typed by the user.
- Use `Path`/`PathBuf`-style operations rather than string concatenation.
- Avoid following symbolic links during discovery.
- Keep source-folder discovery direct-child-only while allowing recursive video discovery inside a
  selected source folder.
- Keep display paths relative without replacing exact internal paths.
- Do not delete, overwrite, or replace anything.
- Do not create the destination during prompt entry.
- Preserve exact selected paths for near-commit revalidation.
- Do not log credentials or unrelated directory contents.
- Keep platform-specific path behavior inside the filesystem adapter.

## Tests and verification

Use temporary directories and platform-aware path helpers. Cover at least:

- current-directory resolution;
- unreadable or unavailable source root handling;
- direct child folder discovery;
- recursive discovery of videos in nested real folders;
- regular-file exclusion;
- symbolic-link exclusion;
- destination exclusion when inside the current directory;
- rejection of the current directory as destination;
- rejection of a destination path that is a file;
- relative and absolute destination resolution;
- nonexistent destination deferred creation;
- case-insensitive matching for multiple recognized video extensions;
- nested-directory traversal without following symbolic links;
- directories with video-looking suffixes excluded from file results;
- deterministic sorting;
- relative display labels for folders, nested video files, and destination paths;
- empty-folder and no-video behavior;
- multiple-folder and multiple-file selection mapping;
- cancellation without filesystem mutation;
- selected-path preservation for later revalidation.

Tests must not depend on a user's actual media directories or require a real terminal. Use scripted UI responses for interaction tests.

## Acceptance checklist

- [x] Filesystem discovery occurs only after TMDB key and language validation.
- [x] The current working directory is the source root.
- [x] The destination is requested before source folders are listed.
- [x] Existing directories are accepted and existing files are rejected as destinations.
- [x] The current directory and selected source folders cannot be used as the destination.
- [x] A nonexistent destination is not created during input.
- [x] Only direct child source folders are listed.
- [x] The destination is excluded from source choices when it is inside the current directory.
- [x] Symbolic links are excluded and real nested folders are traversed for video discovery.
- [x] Recognized regular video files are listed recursively with case-insensitive extensions.
- [x] Filesystem paths are displayed relatively while exact paths are retained internally.
- [x] Multiple source folders and multiple files per folder can be selected explicitly.
- [x] The first item is never selected automatically.
- [x] Empty and invalid states have clear English recovery/cancellation paths.
- [x] Discovery returns typed paths associated with their source folders.
- [x] Automated tests cover path safety, filtering, sorting, and cancellation.
