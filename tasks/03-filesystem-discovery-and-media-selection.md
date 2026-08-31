# Task 03 — Filesystem Discovery and Media Selection

**Status:** Completed — deterministic, non-mutating source-root discovery, destination-subtree exclusion, one expandable video explorer, relative path presentation, and scripted-UI tests are implemented.
**Priority:** P0
**Dependencies:** Task 01
**Blocks:** Task 05

## Objective

Implement safe, deterministic discovery of recognized video files from the current source root,
together with one interactive explorer selection that determines what the later planning task will
process. The destination is chosen after startup TMDB configuration and before the source-root tree
is scanned so the destination subtree can be excluded.

This task reads and validates filesystem state. It must not rename or move media. It must also avoid accidentally traversing more of the filesystem than the product explicitly allows.

## Implementation delivered

The task is implemented across the following boundaries:

- `src/filesystem.rs` resolves the current working directory, validates absolute and relative destinations without creating missing directories, recursively discovers regular files with case-insensitive recognized video extensions from the source root, excludes the destination subtree, skips symbolic links, sorts results deterministically, and reports non-fatal discovery warnings.
- `src/domain.rs` defines `SourceRoot`, `SourceFolder` (an internal source-container grouping), `VideoFile`, `DestinationSelection`, `SelectedSource`, and `FilesystemSelection` so later planning code receives typed paths rather than display strings.
- `src/error.rs` defines actionable filesystem failure categories and maps them to the documented CLI exit-code policy without exposing credentials or raw implementation details.
- `src/ui.rs` extends the renderer-neutral interaction boundary with destination and unified video-file selection operations. The terminal adapter displays source and destination paths relatively while retaining exact paths in typed values, and owns the expandable explorer rendering and English wording.
- `src/app.rs` connects the validated TMDB startup stage to the destination and unified explorer, handles empty/retry/cancel paths, derives internal source-container associations, and never mutates the filesystem during selection.

The selection boundary is consumed by the complete organization workflow: after this task returns,
the application runs a separate TMDB identification loop for every selected video before it builds
the plan, previews it, and performs any confirmed movement.

## Required outcome

After the TMDB API key and metadata language have been collected and validated:

1. obtain the current working directory;
2. ask for and validate the destination path;
3. recursively discover root-level and nested regular files with recognized video extensions;
4. exclude the destination and all descendants from the discovery;
5. present one collapsed-by-default expandable explorer;
6. let the user select one or more individual video files with `Space`, expand/collapse folders
   with `Tab`, and confirm with `Enter`;
7. return one flat typed selection to the application without mutating the filesystem.

The UI may show metadata such as file size, but all filesystem paths shown to the user must be
relative to the current source root. Folder rows are containers only and appear when they contain a
video descendant. The selection result must retain the exact source path needed for later
revalidation.

## Scope

### 1. Resolve the source root

- Use the current working directory, not the executable's directory, as the source root.
- Fail with an actionable English message if the current directory cannot be obtained or read.
- Do not change the process working directory as a side effect of discovery.
- Do not begin filesystem discovery before TMDB startup configuration has been validated.
- Do not modify any file or directory before final confirmation in Task 05.

### 2. Choose and validate the destination

The destination prompt is the first filesystem-related interaction. It must occur before the source
root is scanned and before the explorer is built.

Accept:

- absolute paths;
- paths relative to the current directory;
- existing directories;
- nonexistent directory paths only when the user explicitly agrees that the directory may be created later after plan validation and final confirmation.

Reject:

- a path that exists as a regular file;
- the current directory itself;
- a selected nested source container;
- an unresolved or invalid path;
- a destination that cannot be represented safely by the supported platform.

Normalize and resolve the destination consistently before comparing it with source paths. If the
destination is inside the current directory, exclude the destination and its complete subtree from
the source-root scan. Keep the selected destination visible in later UI stages.

Do not create a nonexistent destination merely because the user typed it. Creation belongs to the confirmed execution path and must be guarded by the final plan validation.

The implementation must handle path comparison without concatenating strings manually. Use platform-aware path operations and account for equivalent path representations where practical.

### 3. Discover the unified media tree

Recursively scan the current directory for regular files whose extension is in the centralized
video-extension allowlist, case-insensitively. The initial allowlist includes common formats such
as `.mkv`, `.mp4`, `.avi`, `.mov`, `.webm`, `.m4v`, `.ts`, `.m2ts`, `.wmv`, and `.flv`. The allowlist
lives in the filesystem adapter so the supported policy remains explicit and easy to extend.

Rules:

- include videos directly in the source root;
- recurse into real nested folders at every depth;
- do not follow symbolic links;
- do not include directories with a video-looking suffix as files;
- skip the destination subtree before descending into it;
- sort the flat result deterministically by relative path;
- retain exact paths even when the UI shows shortened labels;
- return a warning for an inaccessible nested directory while continuing other safe discovery;
- report an empty result with a clear cancellation or destination-retry path.

The terminal layer builds a display-only tree from this flat result. A folder appears only when it
contains at least one video descendant. The tree must not become the authoritative execution path.

### 4. Select videos with one expandable explorer

The explorer is a single multi-selection control over the complete discovered tree:

- folders start collapsed;
- arrow keys navigate visible folder and video rows;
- `Tab` expands or collapses the highlighted folder;
- `Space` selects or deselects only a highlighted video;
- `Enter` confirms one flat array of selected file positions;
- `Escape`/cancel returns without mutation;
- folders are never selected as media items;
- expanding a folder does not select its descendants automatically;
- long relative labels may be truncated from the left so the filename suffix remains visible;
- selection requires at least one video.

The application then derives an internal source-container association from each exact selected path.
That association supports plan and destination validation; it is not a second user-facing selection.

### 5. Support correction and cancellation

The user must be able to cancel before any mutation. Where the UI supports back navigation, it
should allow the user to correct the destination or the unified file selection before planning.

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
- adding search, bulk folder selection, or a persistent tree state beyond the current explorer.

## Safety requirements

- Treat all paths as untrusted input, including paths typed by the user.
- Use `Path`/`PathBuf`-style operations rather than string concatenation.
- Avoid following symbolic links during discovery.
- Keep the authoritative filesystem walk rooted at the current directory and exclude the destination
  subtree before recursive descent.
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
- unified source-root discovery;
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
- relative display labels for explorer folders, nested video files, and destination paths;
- collapsed-by-default explorer state and explicit expansion/collapse behavior;
- root-level videos mixed with nested folders in one selection;
- empty-folder and no-video behavior;
- one unified multi-file selection mapping across root-level and nested videos;
- cancellation without filesystem mutation;
- selected-path preservation for later revalidation.

Tests must not depend on a user's actual media directories or require a real terminal. Use scripted UI responses for interaction tests.

## Acceptance checklist

- [x] Filesystem discovery occurs only after TMDB key and language validation.
- [x] The current working directory is the source root.
- [x] The destination is requested before the source-root media tree is scanned.
- [x] Existing directories are accepted and existing files are rejected as destinations.
- [x] The current directory cannot be used as the destination, and nested destination subtrees are excluded.
- [x] A nonexistent destination is not created during input.
- [x] Videos directly in the current directory and all real nested directories are discovered.
- [x] Only folders containing at least one video descendant appear in the explorer.
- [x] Symbolic links are excluded and real nested folders are traversed for video discovery.
- [x] Recognized regular video files are listed recursively with case-insensitive extensions.
- [x] Filesystem paths are displayed relatively while exact paths are retained internally.
- [x] One explorer selects multiple videos across any depth explicitly.
- [x] Folders start collapsed and can be expanded or collapsed with `Tab`.
- [x] The first item is never selected automatically.
- [x] Empty and invalid states have clear English recovery/cancellation paths.
- [x] Discovery returns typed exact paths, with internal source-container grouping derived afterward.
- [x] Automated tests cover path safety, filtering, sorting, and cancellation.
