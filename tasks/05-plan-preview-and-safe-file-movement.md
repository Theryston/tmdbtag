# Task 05 — Plan, Preview, and Safe File Movement

**Status:** Not started
**Priority:** P0
**Dependencies:** Tasks 01, 02, 03, and 04
**Blocks:** None

## Objective

Connect the validated selections, confirmed TMDB metadata, normalized filenames, and filesystem operations into one safe end-to-end workflow. This task is the only place where the application is allowed to mutate media files or create a missing destination directory.

The central rule is: build and validate the complete operation plan first, display exactly that plan, require explicit confirmation, and execute only the confirmed plan. A failure during planning must result in zero file mutations.

## Required outcome

For every selected source folder:

- associate one confirmed TMDB movie or series;
- enforce the one-TMDB-item-per-source-folder rule;
- require exactly one selected file for a movie;
- allow one or more selected files for a series;
- collect and validate season/episode values for every series file;
- generate normalized destination names;
- detect all conflicts before confirmation;
- display complete source and destination paths;
- request a negative-default confirmation;
- move and rename only after confirmation;
- report per-file success, failure, and pending status.

## Scope

### 1. Build a typed operation plan

The plan should contain enough information to render and execute each operation without recomputing user decisions:

```text
OperationPlan {
    source_root,
    destination,
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

The exact Rust types may differ, but a plan must be immutable from preview through execution. If the displayed plan changes, confirmation must be requested again.

The planner must:

- associate every file with the selected source folder;
- ensure all files in one source folder use the same confirmed TMDB item;
- reject a movie folder with zero or more than one selected file;
- collect series season and episode values per file;
- reject duplicate series + season + episode combinations in one run;
- use only metadata confirmed by TMDB;
- call the naming module for every final filename;
- calculate all destination paths before any mutation.

### 2. Pre-validate the complete plan

Before showing an executable confirmation, validate every operation:

- source folder still exists;
- source file still exists;
- source file is still a regular file;
- source file still has a case-insensitive `.mkv` extension;
- selected source paths are not duplicated;
- source and destination are not the same file;
- destination exists as a directory or is eligible for creation after confirmation;
- destination is writable when it already exists;
- no destination path already exists;
- no two operations produce the same destination path;
- IDs and media types were confirmed by TMDB;
- all series episodes were validated;
- generated names contain no unsafe path traversal;
- relevant operating-system path limits are respected;
- the destination is not one of the selected source folders.

If any operation fails validation, do not offer confirmation and do not mutate any file. Show the affected item and an actionable correction path.

### 3. Render the complete preview

The preview must show the exact operation that will be executed, not a simplified approximation. Include:

- destination folder;
- total file count;
- every full source path;
- every full destination path;
- TMDB ID;
- media type;
- season and episode where applicable;
- normalized filename;
- warnings and conflicts, if any;
- whether a destination directory will need to be created.

Example:

```text
Destination: /library/organized

SOURCE                                      DESTINATION
/input/movies/Fight Club.mkv                /library/organized/550 - Fight Club.mkv
/input/series/episode-01.mkv                /library/organized/1399 - S01E01 - Game of Thrones.mkv

Move and rename 2 files? [y/N]
```

Keep paths readable in narrow terminals through wrapping, truncation with an explicit detail affordance, or a responsive table. Never hide a conflict or replace a path with an unexplained abbreviation.

### 4. Confirm safely

The final action must be explicit and default to no. Declining must:

- move nothing;
- rename nothing;
- delete nothing;
- create no missing destination directory;
- return a clear cancellation result.

If the user goes back or changes any selection after preview, invalidate the old plan and build a new one. Do not retain a stale confirmation.

### 5. Create the destination only at commit

If the validated destination does not exist and the user agreed that it may be created, create it only after final confirmation and immediately before the first move. Revalidate the path after creation and before each relevant operation.

Do not create arbitrary parent trees without the documented user agreement. Do not treat a path that became a file as a directory. If creation fails, preserve all sources and report the failure without attempting moves.

### 6. Move on the same volume

When source and destination are on the same filesystem, prefer a no-replace atomic rename/move operation supported by the platform. The operation must:

- fail if the destination exists;
- never overwrite or delete the existing destination;
- preserve the video contents;
- report the exact source and destination paths;
- avoid a pre-move delete.

Do not assume a generic `rename` call has the required no-overwrite behavior on every supported platform. Wrap platform-specific behavior in the filesystem adapter and test it.

### 7. Move across volumes safely

When a direct rename cannot work because source and destination are on different volumes:

1. copy the source to a temporary file inside the destination directory;
2. keep the temporary name distinct from the final name;
3. verify the copy completed successfully;
4. optionally verify size and another appropriate integrity signal;
5. atomically promote the temporary file to the final name with no replacement;
6. remove the original source only after the destination is verified;
7. remove the temporary file if any step fails.

If verification fails, preserve the source. The system must not leave an apparently complete final destination while also deleting the only verified source.

### 8. Report partial execution

The complete plan is validated before execution, but a later OS failure may still occur. By default, stop starting new operations after an unexpected failure. Report:

- completed operations;
- the operation that failed;
- the failure reason;
- pending operations that were not started;
- any temporary artifact that was cleaned up or could not be cleaned safely.

Keep unprocessed source files in place. A later run must treat existing destination files as conflicts rather than silently guessing whether a previous move completed.

## Explicit non-goals

Do not implement in this task:

- overwriting existing destinations;
- automatic `(1)` or timestamp collision suffixes;
- rollback of files already moved successfully;
- folder renaming or folder creation beyond the confirmed destination;
- subtitles, images, `.nfo`, or auxiliary-file movement;
- recursive traversal;
- multiple TMDB items in one source folder;
- a persistent operation database;
- unattended or confirmation-free execution;
- re-encoding or modifying video contents;
- destructive cleanup of unrecognized files.

## Tests and verification

Cover the full lifecycle with temporary directories and fake TMDB/UI boundaries:

- one movie folder with one file;
- a movie folder with zero or multiple selected files;
- one series folder with multiple episodes;
- duplicate series episode values;
- multiple source folders in one run;
- missing or changed source file after selection;
- destination already existing;
- duplicate destination names within a plan;
- source equal to destination;
- destination inside the current directory;
- nonexistent destination creation after confirmation only;
- user cancellation before confirmation;
- negative confirmation;
- same-volume no-replace move;
- destination conflict during execution;
- cross-volume copy success;
- cross-volume copy verification failure preserving the source;
- failure after one operation completes, including correct pending reporting;
- cleanup of temporary files after failures;
- no mutation when any pre-validation check fails;
- complete preview matching the immutable execution plan;
- exit codes `0`, `1`, and `2` according to the root README.

Do not require a real TMDB API, a real media library, or a particular filesystem mount for unit tests. Cross-volume behavior may require a platform-specific integration test or an explicit test seam.

## Acceptance checklist

- [ ] The planner enforces one confirmed TMDB item per source folder.
- [ ] Movies require exactly one selected `.mkv` file.
- [ ] Series files receive individually validated season and episode values.
- [ ] The full plan is calculated before any filesystem mutation.
- [ ] All paths, metadata, names, conflicts, and warnings appear in the preview.
- [ ] Any pre-validation failure blocks confirmation and causes zero mutations.
- [ ] Confirmation is explicit and defaults to no.
- [ ] Declining confirmation creates no destination and changes no files.
- [ ] Existing destinations are never overwritten or silently renamed around.
- [ ] Same-volume movement uses a safe no-replace operation.
- [ ] Cross-volume movement verifies the temporary copy before removing the source.
- [ ] Unexpected execution failures stop new work by default and report completed, failed, and pending files.
- [ ] Per-file final results and totals are rendered in English.
- [ ] The implementation preserves the video contents.
- [ ] Automated tests cover planning, cancellation, conflicts, same-volume moves, cross-volume safety, and partial failure.
- [ ] Full formatting, build, test, lint, and diff checks pass.
