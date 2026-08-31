# Task 04 — Naming Normalization and Metadata Recovery

**Status:** Not started
**Priority:** P0
**Dependencies:** Task 01; uses the metadata model from Task 02 when integrated
**Blocks:** Task 05

## Objective

Implement the pure, deterministic naming layer that converts confirmed TMDB metadata into filesystem-safe filenames and later recovers the essential metadata from those filenames. This is the identity contract of the product: the generated name must remain useful even though there is no local database in the MVP.

The raw title returned by TMDB is metadata. It must never be inserted directly into a filename. Every title passes through one mandatory normalization function before composition.

## Required outcome

Generate exactly these filename shapes:

```text
Movie:
{tmdb_id} - {normalized_title}.mkv

Series episode:
{tmdb_id} - S{season:02}E{episode:02} - {normalized_series_title}.mkv
```

Examples:

```text
550 - Fight Club.mkv
1399 - S01E01 - Game of Thrones.mkv
519182 - Spider-Man - No Way Home.mkv
```

The normalization must be deterministic and idempotent. A generated filename must be parseable into the TMDB ID, media type, and, for a series, season and episode without relying on a database or the original source filename.

## Scope

### 1. Define typed naming inputs

Use validated values rather than arbitrary strings where practical:

- positive TMDB ID;
- explicit movie or series media type;
- non-negative season and episode values for series;
- raw localized title;
- original title as an optional fallback;
- fixed `.mkv` extension.

Keep the raw title separate from the normalized title. The raw value is needed for display, diagnostics, and future metadata operations. The normalized value is only for the destination filename.

### 2. Implement mandatory title normalization

Implement one central function, conceptually:

```text
normalize_title_for_filename(raw_title) -> normalized_title or error
```

The pipeline must:

1. preserve the raw TMDB title outside the function;
2. trim leading and trailing Unicode whitespace;
3. replace filesystem-invalid characters and control characters with a readable safe separator;
4. include at least `/`, `\\`, `:`, `*`, `?`, `"`, `<`, `>`, and `|` in the invalid-character policy;
5. use a readable separator such as ` - `, so `Mission: Impossible` becomes `Mission - Impossible`;
6. collapse repeated whitespace and repeated replacement separators;
7. remove trailing whitespace, periods, and replacement separators;
8. preserve accents and safe Unicode characters;
9. account for Windows-reserved filename components if Windows support is claimed;
10. shorten only the title component when path-length limits require it;
11. return an error if normalization would produce an empty title rather than inventing metadata.

The function must be platform-safe even when executed on a platform where a character such as `:` happens to be accepted. Cross-platform output is the reason the invalid-character policy is explicit.

Required examples:

```text
Mission: Impossible       -> Mission - Impossible
Spider-Man: No Way Home   -> Spider-Man - No Way Home
What?                     -> What
Title / Director          -> Title - Director
```

Normalization must satisfy:

```text
normalize_title_for_filename(
    normalize_title_for_filename(title)
) == normalize_title_for_filename(title)
```

Normalization may change only the title component. It must never change the TMDB ID, media type, season, episode, extension, or raw metadata held in memory.

### 3. Compose movie and series names

Movie rules:

- use the numeric TMDB ID without a `tmdb` prefix;
- use one ` - ` separator between ID and title;
- use the normalized localized title, falling back to the original title only when the configured-language title is unavailable;
- use lowercase `.mkv` in generated names;
- omit year, codec, resolution, language, release group, and original filename.

Series rules:

- use the numeric series ID;
- use `SxxEyy` with at least two digits for season and episode;
- do not truncate values above 99;
- use the series title, not the episode title;
- omit arbitrary source filename data;
- use lowercase `.mkv`.

Do not add random, timestamped, or `(1)`-style collision suffixes. Collisions are a plan-validation error owned by Task 05.

### 4. Parse generated filenames

Implement a parser for the documented generated forms. The parser should return a typed reference equivalent to:

```text
ParsedMediaReference {
    tmdb_id: positive integer,
    media_type: Movie or Series,
    season: optional non-negative integer,
    episode: optional non-negative integer,
    title_hint: normalized title,
}
```

Parsing rules:

- recognize the exact movie and series separators and `.mkv` extension policy;
- infer series type from the presence of `SxxEyy`, but keep API validation as the authority when metadata is fetched later;
- reject malformed, missing, zero, negative, overflowed, or ambiguous IDs;
- reject a series marker with only one of season/episode;
- preserve the title hint as a display hint, not as authoritative metadata;
- never interpret arbitrary source filenames as confirmed TMDB references.

The parser must not perform a network request. A future metadata command can use the parsed ID and type to query TMDB again, then treat the returned title as authoritative.

### 5. Keep naming path-safe

- Compose a filename, not a path, in the naming module.
- Never allow normalized title content to introduce a path separator.
- Do not permit `.` or `..` as a title component.
- Keep extension handling explicit and case-stable.
- Expose length or path-limit errors to the plan validator rather than silently dropping identity fields.

## Explicit non-goals

Do not implement in this task:

- TMDB HTTP requests;
- title search or result selection;
- filesystem discovery;
- source or destination path resolution;
- actual file movement or directory creation;
- automatic season/episode detection from original names;
- episode-title inclusion;
- arbitrary collision suffixes;
- a local database;
- rewriting raw TMDB metadata in memory.

## Tests and verification

Because this layer is pure, prefer exhaustive unit tests and property-style tests where practical. Cover at least:

- movie composition;
- series composition;
- one- and two-digit season/episode values;
- values greater than 99;
- IDs with invalid, zero, negative, or overflowing values;
- accents and safe Unicode;
- leading/trailing whitespace;
- every invalid character in the documented set;
- colon-containing titles, including `Mission: Impossible`;
- slash and backslash path-separator rejection;
- control characters;
- repeated separators and spaces;
- trailing periods and spaces;
- empty-after-normalization titles;
- Windows-reserved names when supported;
- title normalization idempotence;
- generated-name round trips through the parser;
- malformed movie and series names;
- parser rejection of ambiguous or unsafe names;
- guarantee that parsing does not access the network or filesystem.

## Acceptance checklist

- [ ] All final filenames pass through one deterministic title-normalization function.
- [ ] A colon is always normalized even on platforms that permit it.
- [ ] Invalid characters cannot create path separators or unsafe components.
- [ ] Accents and safe Unicode are preserved.
- [ ] The raw TMDB title remains available separately from the normalized title.
- [ ] Movie names match the documented pattern exactly.
- [ ] Series names match the documented pattern exactly.
- [ ] Season and episode values are padded to at least two digits without truncation.
- [ ] The generated extension is `.mkv`.
- [ ] The title never contributes unverified source filename data.
- [ ] Generated names can be parsed back into ID, type, and episode values.
- [ ] Parsing treats the title as a hint and the ID/type as the recoverable identity.
- [ ] Unit and round-trip tests cover normal, invalid, Unicode, and edge cases.
