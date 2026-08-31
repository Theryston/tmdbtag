use std::{fmt, path::PathBuf, str::FromStr};

use thiserror::Error;

/// The two TMDB media namespaces supported by the application.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MediaType {
    /// A TMDB movie.
    Movie,
    /// A TMDB TV series.
    Series,
}

impl MediaType {
    /// Returns the stable uppercase label used by the terminal UI.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Movie => "MOVIE",
            Self::Series => "SERIES",
        }
    }
}

impl fmt::Display for MediaType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label())
    }
}

/// A positive numeric identifier from TMDB.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TmdbId(u64);

impl TmdbId {
    /// Creates an identifier, rejecting zero because TMDB IDs are positive.
    pub const fn new(value: u64) -> Result<Self, DomainError> {
        if value == 0 {
            Err(DomainError::InvalidTmdbId)
        } else {
            Ok(Self(value))
        }
    }

    /// Returns the numeric TMDB identifier.
    pub const fn value(self) -> u64 {
        self.0
    }
}

impl fmt::Display for TmdbId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Parses a user-entered TMDB ID without accepting signs, decimals, or surrounding content.
pub fn parse_tmdb_id(value: &str) -> Result<TmdbId, DomainError> {
    let value = value.trim();
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(DomainError::InvalidTmdbId);
    }

    let parsed = u64::from_str(value).map_err(|_| DomainError::InvalidTmdbId)?;
    TmdbId::new(parsed)
}

/// A non-negative season and episode pair entered for a TV series.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EpisodeRef {
    season: u32,
    episode: u32,
}

impl EpisodeRef {
    /// Creates an episode reference. Season zero is retained for TMDB specials.
    pub const fn new(season: u32, episode: u32) -> Self {
        Self { season, episode }
    }

    /// Parses non-negative decimal season and episode values.
    pub fn parse(season: &str, episode: &str) -> Result<Self, DomainError> {
        let season = parse_non_negative_number(season, "season")?;
        let episode = parse_non_negative_number(episode, "episode")?;
        Ok(Self::new(season, episode))
    }

    /// Returns the season number.
    pub const fn season(self) -> u32 {
        self.season
    }

    /// Returns the episode number.
    pub const fn episode(self) -> u32 {
        self.episode
    }
}

/// The absolute directory from which the process was started and where source folders are found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceRoot {
    path: PathBuf,
}

impl SourceRoot {
    /// Creates a source-root value. Filesystem validation is performed by the filesystem layer.
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Returns the exact source-root path.
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
}

/// A direct child directory that can be selected as a source folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceFolder {
    path: PathBuf,
}

impl SourceFolder {
    /// Creates a source-folder value. Discovery is responsible for proving that it is a real
    /// directory rather than a symbolic link.
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Returns the exact source-folder path.
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
}

/// A regular video file discovered inside one source folder or one of its real subdirectories.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoFile {
    path: PathBuf,
    size_bytes: Option<u64>,
}

impl VideoFile {
    /// Creates a video-file value while retaining an optional display-only size.
    pub fn new(path: PathBuf, size_bytes: Option<u64>) -> Self {
        Self { path, size_bytes }
    }

    /// Returns the exact path that must be revalidated before a later move.
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// Returns the size observed during discovery, when metadata was available.
    pub const fn size_bytes(&self) -> Option<u64> {
        self.size_bytes
    }
}

/// A validated destination path. A missing destination is retained without being created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DestinationSelection {
    path: PathBuf,
    exists: bool,
    may_create_after_confirmation: bool,
}

impl DestinationSelection {
    /// Creates a destination value after filesystem-layer validation.
    pub fn new(path: PathBuf, exists: bool, may_create_after_confirmation: bool) -> Self {
        Self {
            path,
            exists,
            may_create_after_confirmation,
        }
    }

    /// Returns the normalized destination path.
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// Returns whether the destination already existed during discovery.
    pub const fn exists(&self) -> bool {
        self.exists
    }

    /// Returns whether creation is permitted later, after the final plan confirmation.
    pub const fn may_create_after_confirmation(&self) -> bool {
        self.may_create_after_confirmation
    }
}

/// The selected files associated with one source folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedSource {
    folder: PathBuf,
    files: Vec<PathBuf>,
}

impl SelectedSource {
    /// Creates a selection while retaining exact paths instead of display labels.
    pub fn new(folder: PathBuf, files: Vec<PathBuf>) -> Self {
        Self { folder, files }
    }

    /// Returns the exact selected source-folder path.
    pub fn folder(&self) -> &std::path::Path {
        &self.folder
    }

    /// Returns the exact selected file paths in deterministic UI order.
    pub fn files(&self) -> &[PathBuf] {
        &self.files
    }
}

/// The complete non-mutating filesystem selection returned by Task 03.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilesystemSelection {
    source_root: SourceRoot,
    destination: DestinationSelection,
    sources: Vec<SelectedSource>,
}

impl FilesystemSelection {
    /// Creates a selection for later metadata planning and near-commit revalidation.
    pub fn new(
        source_root: SourceRoot,
        destination: DestinationSelection,
        sources: Vec<SelectedSource>,
    ) -> Self {
        Self {
            source_root,
            destination,
            sources,
        }
    }

    /// Returns the source root used for discovery.
    pub fn source_root(&self) -> &SourceRoot {
        &self.source_root
    }

    /// Returns the validated destination selection.
    pub fn destination(&self) -> &DestinationSelection {
        &self.destination
    }

    /// Returns the selected source groups in deterministic order.
    pub fn sources(&self) -> &[SelectedSource] {
        &self.sources
    }
}

fn parse_non_negative_number(value: &str, field: &'static str) -> Result<u32, DomainError> {
    let value = value.trim();
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(DomainError::InvalidNonNegativeNumber { field });
    }

    u32::from_str(value).map_err(|_| DomainError::InvalidNonNegativeNumber { field })
}

/// The method the user chooses to identify a TMDB item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentificationMethod {
    /// Search the movie and TV namespaces by text.
    Search,
    /// Fetch one explicitly typed numeric TMDB ID.
    ManualId,
}

/// A search candidate returned by TMDB before the user confirms its details.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TmdbSearchCandidate {
    /// The candidate's positive TMDB ID.
    pub id: TmdbId,
    /// The candidate's TMDB namespace.
    pub media_type: MediaType,
    /// The localized title/name selected by the API response, with its original fallback applied.
    pub title: String,
    /// The original TMDB title/name when it differs or is available.
    pub original_title: Option<String>,
    /// The release or first-air year when TMDB returned a valid year.
    pub year: Option<u16>,
}

/// One bounded page of TMDB search candidates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TmdbSearchPage {
    /// Candidates in the order returned by TMDB.
    pub results: Vec<TmdbSearchCandidate>,
    /// The one-based page represented by this value.
    pub page: u32,
    /// The number of pages TMDB reports for the query.
    pub total_pages: u32,
}

impl TmdbSearchPage {
    /// Returns whether another TMDB page can be requested.
    pub const fn has_next_page(&self) -> bool {
        self.page < self.total_pages
    }
}

/// A verified movie or TV series returned by a TMDB details endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TmdbItem {
    /// The verified TMDB ID.
    pub id: TmdbId,
    /// The verified media namespace.
    pub media_type: MediaType,
    /// The localized title/name, falling back to the original TMDB value when necessary.
    pub title: String,
    /// The original TMDB title/name when available.
    pub original_title: Option<String>,
    /// The release or first-air year when TMDB returned a valid year.
    pub year: Option<u16>,
}

/// A verified episode response used to prove that a season/episode pair exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TmdbEpisode {
    /// The series containing the episode.
    pub series_id: TmdbId,
    /// The requested, verified season/episode pair.
    pub episode: EpisodeRef,
    /// The episode title when TMDB returned one. It is not used in MVP filenames.
    pub title: Option<String>,
}

/// Errors raised when external input cannot become a valid domain value.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum DomainError {
    /// A TMDB ID was empty, malformed, zero, or outside the supported numeric range.
    #[error("TMDB IDs must be positive numeric values.")]
    InvalidTmdbId,
    /// A season or episode value was not a non-negative decimal integer.
    #[error("The {field} number must be a non-negative integer.")]
    InvalidNonNegativeNumber { field: &'static str },
}

/// The high-level result of one application invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunOutcome {
    /// The user canceled before any filesystem mutation was possible.
    Cancelled,
    /// The CLI foundation completed its startup setup.
    StartupConfigured,
    /// The startup configuration and non-mutating filesystem selection completed.
    MediaSelectionReady,
    /// The saved TMDB configuration was intentionally updated by the `config` command.
    ConfigurationUpdated,
}

impl RunOutcome {
    /// Returns the process exit code for this non-mutating outcome.
    pub const fn exit_code(self) -> i32 {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tmdb_id_parser_accepts_trimmed_positive_decimal_values() {
        assert_eq!(parse_tmdb_id(" 550 ").unwrap().value(), 550);
        assert_eq!(TmdbId::new(1).unwrap().to_string(), "1");
    }

    #[test]
    fn tmdb_id_parser_rejects_zero_signs_and_malformed_values() {
        for value in ["", "0", "-1", "+1", "1.0", "abc", "12 34"] {
            assert_eq!(parse_tmdb_id(value), Err(DomainError::InvalidTmdbId));
        }
    }

    #[test]
    fn episode_parser_accepts_non_negative_numbers_including_specials() {
        let episode = EpisodeRef::parse("0", " 7 ").unwrap();

        assert_eq!(episode.season(), 0);
        assert_eq!(episode.episode(), 7);
    }

    #[test]
    fn episode_parser_rejects_negative_or_overflowing_numbers() {
        assert_eq!(
            EpisodeRef::parse("-1", "1"),
            Err(DomainError::InvalidNonNegativeNumber { field: "season" })
        );
        assert_eq!(
            EpisodeRef::parse("1", "4294967296"),
            Err(DomainError::InvalidNonNegativeNumber { field: "episode" })
        );
    }
}
