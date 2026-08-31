# Task 05 — Plan, Preview, and Safe File Operations

**Status:** Completed **Priority:** P0 **Dependencies:** Tasks 01, 02, 03, and
04 **Blocks:** None

## Objective

Connect the validated selections, confirmed TMDB metadata, normalized filenames,
and filesystem operations into one safe end-to-end workflow. This task is the
only place where the application is allowed to write organized media files,
remove sources for an approved move, or create a missing destination directory.

The central rule is: build and validate the complete operation plan first,
display exactly that plan, require explicit confirmation, and execute only the
confirmed plan. A failure during planning must result in zero file mutations.

## Required outcome

For every selected video file:

- ask whether the run should `Copy` or `Move` before destination and media
  selection;
- run one complete identification loop and associate one confirmed TMDB movie or
  series;
- allow multiple independent TMDB items to come from the same directory tree;
- collect and validate season/episode values when that file is identified as a
  series;
- generate normalized destination names;
- detect all conflicts before confirmation;
- display complete source and destination paths as relative UI paths;
- request a negative-default confirmation;
- copy or move and rename only after confirmation;
- preserve every source for `Copy`, producing independent destination data;
- show aggregate progress based on transferred or logically completed bytes;
- report per-file success, failure, and pending status.

## Scope

### 1. Build a typed operation plan

The plan should contain enough information to render and execute each operation
without recomputing user decisions:

```text
OperationPlan {
    source_root,
    destination,
    operation: Copy | Move,
    operations: [
        PlannedOperation {
            source_path,
            destination_path,
            tmdb_id,
            media_type,
            normalized_filename,
            season,
            episode,
        }
    ],
}
```

The exact Rust types may differ, but a plan must be immutable from preview
through execution. If the displayed plan changes, confirmation must be requested
again.

The selected operation is part of the immutable plan. The executor must never
infer or change it after confirmation:

- `Copy` creates an independent destination file and leaves the source present
  and unchanged;
- `Move` publishes the destination safely and removes the source only after
  successful publication and source revalidation.

The planner must:

- associate every file with its internal source container for validation
  context;
- identify and confirm the TMDB item independently for every selected file;
- create one movie operation for every file identified as a movie;
- collect series season and episode values per file identified as a series;
- reject duplicate series + season + episode combinations in one run;
- use only metadata confirmed by TMDB;
- call the naming module for every final filename;
- calculate all destination paths before any mutation.
- calculate the total source-byte size before preview and use it as the progress
  denominator during execution.

### 2. Pre-validate the complete plan

Before showing an executable confirmation, validate every operation:

- source folder still exists;
- source file still exists;
- source file is still a regular file;
- source file still has a recognized video extension, case-insensitively;
- the generated destination name preserves that source extension in lowercase;
- selected source paths are not duplicated;
- source and destination are not the same file;
- destination exists as a directory or is eligible for creation after
  confirmation;
- destination is writable when it already exists;
- no destination path already exists;
- no two operations produce the same destination path;
- IDs and media types were confirmed by TMDB;
- all series episodes were validated;
- generated names contain no unsafe path traversal;
- relevant operating-system path limits are respected;
- the destination is not one of the selected nested source containers.

If any operation fails validation, do not offer confirmation and do not mutate
any file. Show the affected item and an actionable correction path.

### 3. Render the complete preview

The preview must show the exact operation that will be executed, not a
simplified approximation. Include:

- destination folder;
- the selected operation (`Copy` or `Move`) and whether source files will be
  kept;
- the total source-byte size that determines the progress percentage;
- total file count;
- every relative source path;
- every relative destination path;
- TMDB ID;
- media type;
- season and episode where applicable;
- normalized filename;
- warnings and conflicts, if any;
- whether a destination directory will need to be created.

Example:

```text
Destination: ../library/organized
Operation: Copy (original files will be kept)
Total bytes: 1.2 GiB

SOURCE                                      DESTINATION
movies/Fight Club.mkv                       ../library/organized/550__S__MOVIE__S__Fight Club.mkv
series/season-01/episode-01.mp4             ../library/organized/1399__S__SERIES__S__S01E01__S__Game of Thrones.mp4

Copy and rename 2 files? [y/N]
```

Keep relative paths readable in narrow terminals through wrapping, truncation
with an explicit detail affordance, or a responsive table. Never hide a conflict
or replace a path with an unexplained abbreviation. Exact normalized paths
remain in the immutable plan and are the only values passed to the executor.

### 4. Confirm safely

The final action must be explicit and default to no. Declining must:

- copy or move nothing;
- rename nothing;
- delete nothing;
- create no missing destination directory;
- return a clear cancellation result.

If the user goes back or changes any selection after preview, invalidate the old
plan and build a new one. Do not retain a stale confirmation.

The confirmation text must name the selected operation. It must not say only
"execute" or "process", because the distinction between preserving and removing
the source is safety-critical.

### 5. Create the destination only at commit

If the validated destination does not exist and the user agreed that it may be
created, create it only after final confirmation and immediately before the
first copy or move. Revalidate the path after creation and before each relevant
operation.

Do not create arbitrary parent trees without the documented user agreement. Do
not treat a path that became a file as a directory. If creation fails, preserve
all sources and report the failure without attempting copy or move operations.

### 6. Copy with source preservation

An explicit `Copy` operation must leave the source file present and unchanged.
The destination must contain independent data, not a hard link or another alias
of the source, even when both paths are on the same filesystem.

After confirmation, execute a copy using this sequence:

1. create a uniquely named temporary file inside the destination directory;
2. stream the source bytes into the temporary file;
3. report progress as each chunk is written;
4. flush or close the temporary file;
5. verify the temporary file against the selected source snapshot;
6. publish it under the final name with no-replace behavior;
7. preserve the source and clean up the temporary path.

If copying, verification, or publication fails, preserve the source and report
the failure. Never implement `Copy` as a move followed by source recreation.

### 7. Move on the same volume

When source and destination are on the same filesystem, prefer a no-replace
atomic rename/move operation supported by the platform. The operation must:

- fail if the destination exists;
- never overwrite or delete the existing destination;
- preserve the video contents;
- report the exact source and destination paths;
- avoid a pre-move delete.

Do not assume a generic `rename` call has the required no-overwrite behavior on
every supported platform. Wrap platform-specific behavior in the filesystem
adapter and test it.

### 8. Move across volumes safely

When a direct rename cannot work because source and destination are on different
volumes:

1. copy the source to a temporary file inside the destination directory;
2. keep the temporary name distinct from the final name;
3. verify the copy completed successfully;
4. optionally verify size and another appropriate integrity signal;
5. atomically promote the temporary file to the final name with no replacement;
6. remove the original source only after the destination is verified;
7. remove the temporary file if any step fails.

If verification fails, preserve the source. The system must not leave an
apparently complete final destination while also deleting the only verified
source.

### 9. Report byte progress and partial execution

The progress bar is one aggregate determinate bar for the complete plan. Its
percentage is calculated as:

```text
completed_or_transferred_source_bytes / total_plan_source_bytes * 100
```

For copies and cross-volume moves, update the bar while bytes are written. For
same-volume moves, mark the file's full source size complete only after the
no-replace publication succeeds, because no byte stream is copied. A successful
zero-byte operation is complete. Progress callbacks are presentation-only and
must not authorize, reorder, or mutate filesystem operations.

The complete plan is validated before execution, but a later OS failure may
still occur. By default, stop starting new operations after an unexpected
failure. Report:

- completed operations;
- the operation that failed;
- the failure reason;
- pending operations that were not started;
- any temporary artifact that was cleaned up or could not be cleaned safely.

Keep unprocessed source files in place. A later run must treat existing
 destination files as conflicts rather than silently guessing whether a previous
 copy or move completed.

## Explicit non-goals

Do not implement in this task:

- overwriting existing destinations;
- automatic `(1)` or timestamp collision suffixes;
- rollback of files already moved successfully;
- folder renaming or folder creation beyond the confirmed destination;
- subtitles, images, `.nfo`, or auxiliary-file movement;
- alternate filesystem traversal outside the current source-root explorer;
- a persistent operation database;
- unattended or confirmation-free execution;
- re-encoding or modifying video contents;
- destructive cleanup of unrecognized files.

## Tests and verification

Cover the full lifecycle with temporary directories and fake TMDB/UI boundaries:

- one directory tree containing one movie file;
- one directory tree with multiple selected movie files, each using its own
  identification loop;
- one directory tree with multiple series episodes;
- duplicate series episode values;
- root-level and nested videos selected together from one explorer;
- missing or changed source file after selection;
- destination already existing;
- duplicate destination names within a plan;
- source equal to destination;
- destination inside the current directory;
- nonexistent destination creation after confirmation only;
- user cancellation before confirmation;
- negative confirmation;
- explicit copy selection preserves the source and creates independent
  destination data;
- aggregate byte progress reaches the total source bytes for copy and move;
- same-volume no-replace move;
- destination conflict during execution;
- cross-volume copy success;
- cross-volume copy verification failure preserving the source;
- failure after one operation completes, including correct pending reporting;
- cleanup of temporary files after failures;
- no mutation when any pre-validation check fails;
- complete preview matching the immutable execution plan;
- exit codes `0`, `1`, and `2` according to the root README.

Do not require a real TMDB API, a real media library, or a particular filesystem
mount for unit tests. Cross-volume behavior may require a platform-specific
integration test or an explicit test seam.

## Acceptance checklist

- [x] The planner runs one confirmed TMDB identification loop for every selected
      video file.
- [x] The user selects `Copy` or `Move` before destination and media selection,
      and that operation remains in the immutable plan.
- [x] Multiple independent movie or series items can be selected from the same
      directory tree.
- [x] Series files receive individually identified and validated season and
      episode values.
- [x] The full plan is calculated before any filesystem mutation.
- [x] All relative paths, metadata, names, conflicts, and warnings appear in the
      preview or actionable validation output.
- [x] Any pre-validation failure blocks confirmation and causes zero mutations.
- [x] Confirmation is explicit and defaults to no.
- [x] Declining confirmation creates no destination and changes no files.
- [x] Existing destinations are never overwritten or silently renamed around.
- [x] Copy creates independent destination data and leaves every source intact.
- [x] Same-volume movement uses a safe no-replace operation.
- [x] Cross-volume movement verifies the temporary copy before removing the
      source.
- [x] Copy and move display aggregate progress based on source bytes rather than
      file count.
- [x] Unexpected execution failures stop new work by default and report
      completed, failed, and pending files.
- [x] Per-file final results and totals are rendered in English.
- [x] The implementation preserves the video contents.
- [x] Automated tests cover planning, cancellation, conflicts, same-volume
      moves, cross-volume safety, and partial failure.
- [x] Full formatting, build, test, lint, and diff checks pass.
