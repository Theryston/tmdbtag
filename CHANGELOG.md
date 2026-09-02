# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.4](https://github.com/Theryston/tmdbtag/compare/tmdbtag-v0.1.3...tmdbtag-v0.1.4) - 2026-09-02

### Fixed

- delete source folder

## [0.1.3](https://github.com/Theryston/tmdbtag/compare/tmdbtag-v0.1.2...tmdbtag-v0.1.3) - 2026-09-02

### Fixed

- support automatic recovery of tagged filenames

## [0.1.2](https://github.com/Theryston/tmdbtag/compare/tmdbtag-v0.1.1...tmdbtag-v0.1.2) - 2026-09-01

### Fixed

- Use same-namespace rename for local moves

### Other

- Add live transfer-rate display to progress bar

## [0.1.1](https://github.com/Theryston/tmdbtag/compare/tmdbtag-v0.1.0...tmdbtag-v0.1.1) - 2026-08-31

### Fixed

- .gitignore

### Other

- Implement multi-bucket S3 catalog with per-run prefixes
- Add S3 storage backends and cross-storage transfers

## [0.1.0](https://github.com/Theryston/tmdbtag/releases/tag/tmdbtag-v0.1.0) - 2026-08-31

### Fixed

- clean up readme

### Other

- Add automated release pipeline and installers
- Rename project to tmdbtag and refresh README
- Add copy-or-move operation selection
- Update filename separator to __S__
- Switch naming format to `__TMDB__` field delimiter
- Update live search debounce to 500 ms and preserve file logs
- **Add live debounced TMDB search selector with crossterm**
- Replace per-file context box with active file line
- Unify video discovery with expandable source-root explorer
- Complete interactive MVP workflow through safe file movement
- Implement naming normalization and metadata recovery
- Implement filesystem discovery and media selection
- Add TMDB client with search and identification
- Persist shared TMDB configuration between runs
- Implement CLI foundation and interactive shell
- Add implementation task breakdowns
- Add engineering and product specification docs
- Initialize Rust project with TMDb file title tool
