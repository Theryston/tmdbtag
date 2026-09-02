use std::{fmt, path::PathBuf, str::FromStr, time::SystemTime};

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

/// The filesystem operation selected for the current organization run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileOperation {
    /// Create destination files and keep every source file unchanged.
    Copy,
    /// Create destination files and remove each source only after successful publication.
    Move,
}

impl FileOperation {
    /// Returns the English operation name used in summaries and diagnostics.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Copy => "Copy",
            Self::Move => "Move",
        }
    }

    /// Returns the lowercase verb used in confirmation prompts.
    pub const fn verb(self) -> &'static str {
        match self {
            Self::Copy => "copy",
            Self::Move => "move",
        }
    }

    /// Returns whether the operation preserves the source file.
    pub const fn preserves_source(self) -> bool {
        matches!(self, Self::Copy)
    }
}

impl fmt::Display for FileOperation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label())
    }
}

/// A storage service that can provide source media or receive organized output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StorageKind {
    /// The process current directory and the local operating-system filesystem.
    Local,
    /// An S3-compatible object bucket selected from the saved bucket catalog.
    S3,
}

impl StorageKind {
    /// Returns the stable label used by prompts, previews, and diagnostics.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Local => "Local",
            Self::S3 => "S3",
        }
    }
}

impl fmt::Display for StorageKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label())
    }
}

/// The side of an organization workflow for which a storage choice is collected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageRole {
    /// The place from which eligible videos are discovered.
    Source,
    /// The place to which generated names are published.
    Destination,
}

impl StorageRole {
    /// Returns the lowercase label used in interactive questions.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::Destination => "destination",
        }
    }
}

/// Common video filename extensions supported by the discovery and naming layers.
///
/// The policy is intentionally extension-based. Portable filesystem metadata does not provide a
/// reliable MIME type, so both discovery and filename parsing must use this same explicit list.
pub const VIDEO_EXTENSIONS: &[&str] = &[
    "3g2", "3gp", "3gp2", "3gpp", "amv", "asf", "avi", "bik", "braw", "dav", "divx", "drc", "dv",
    "dvr-ms", "f4v", "flv", "flic", "h264", "h265", "hevc", "ivf", "m1v", "m2p", "m2t", "m2ts",
    "m2v", "m4v", "mj2", "mjpeg", "mjpg", "mk3d", "mkv", "mod", "mov", "mp2", "mp2v", "mp4", "mpe",
    "mpeg", "mpg", "mpv", "mts", "mxf", "nsv", "nut", "ogm", "ogv", "qt", "r3d", "rm", "rmvb",
    "roq", "svi", "tod", "ts", "vdr", "vob", "webm", "wmv", "xvid", "y4m", "yuv",
];

/// A validated, lowercase video filename extension without its leading period.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct VideoExtension(String);

impl VideoExtension {
    /// Validates an extension against the shared video-extension policy.
    pub fn parse(value: &str) -> Result<Self, DomainError> {
        VIDEO_EXTENSIONS
            .iter()
            .find(|known| value.eq_ignore_ascii_case(known))
            .map(|known| Self((*known).to_owned()))
            .ok_or_else(|| DomainError::UnsupportedVideoExtension {
                extension: sanitize_domain_text(value),
            })
    }

    /// Returns the canonical lowercase extension without a leading period.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for VideoExtension {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
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

/// The absolute directory from which the process was started and where the media tree is scanned.
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

/// A real directory retained as an internal source container for selected files.
///
/// The interactive workflow no longer asks the user to select source folders. A nested video's
/// container is its direct child of the source root, while a video directly in the source root is
/// grouped under the root itself for plan validation.
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

/// Selected files associated with one internal source container.
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

/// The complete non-mutating filesystem selection returned by the media explorer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilesystemSelection {
    source_root: SourceRoot,
    destination: DestinationSelection,
    operation: FileOperation,
    sources: Vec<SelectedSource>,
    delete_source_folders: bool,
}

impl FilesystemSelection {
    /// Creates a selection for later metadata planning and near-commit revalidation.
    pub fn new(
        source_root: SourceRoot,
        destination: DestinationSelection,
        operation: FileOperation,
        sources: Vec<SelectedSource>,
    ) -> Self {
        Self {
            source_root,
            destination,
            operation,
            sources,
            delete_source_folders: false,
        }
    }

    /// Enables or disables recursive cleanup of selected files' containing folders after moves.
    ///
    /// The executor applies this only to move plans. Keeping the choice on the selection makes it
    /// available to the immutable plan without coupling the domain to the terminal prompt.
    pub fn with_delete_source_folders(mut self, delete_source_folders: bool) -> Self {
        self.delete_source_folders = delete_source_folders;
        self
    }

    /// Returns the source root used for discovery.
    pub fn source_root(&self) -> &SourceRoot {
        &self.source_root
    }

    /// Returns the validated destination selection.
    pub fn destination(&self) -> &DestinationSelection {
        &self.destination
    }

    /// Returns the copy-or-move operation selected for this filesystem selection.
    pub const fn operation(&self) -> FileOperation {
        self.operation
    }

    /// Returns the selected source groups in deterministic order.
    pub fn sources(&self) -> &[SelectedSource] {
        &self.sources
    }

    /// Returns whether containing source folders should be deleted after successful moves.
    pub const fn delete_source_folders(&self) -> bool {
        self.delete_source_folders
    }
}

/// The source-file state captured while a plan is built.
///
/// The executor compares this snapshot again immediately before each operation. Size and
/// modification time are intentionally used as a portable, bounded identity check; a future
/// implementation may add a content digest without changing the plan shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileSnapshot {
    size_bytes: u64,
    modified: Option<SystemTime>,
}

impl FileSnapshot {
    /// Creates a source snapshot from portable filesystem observations.
    pub fn new(size_bytes: u64, modified: Option<SystemTime>) -> Self {
        Self {
            size_bytes,
            modified,
        }
    }

    /// Returns the observed file size.
    pub const fn size_bytes(&self) -> u64 {
        self.size_bytes
    }

    /// Returns the observed modification time when the filesystem provided one.
    pub fn modified(&self) -> Option<SystemTime> {
        self.modified
    }
}

/// One immutable source-to-destination operation in an approved organization plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedOperation {
    source_folder: PathBuf,
    source_path: PathBuf,
    destination_path: PathBuf,
    normalized_filename: String,
    tmdb_item: TmdbItem,
    episode: Option<EpisodeRef>,
    source_extension: VideoExtension,
    source_snapshot: FileSnapshot,
}

impl PlannedOperation {
    /// Creates an operation from already verified metadata and a source snapshot.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        source_folder: PathBuf,
        source_path: PathBuf,
        destination_path: PathBuf,
        normalized_filename: String,
        tmdb_item: TmdbItem,
        episode: Option<EpisodeRef>,
        source_extension: VideoExtension,
        source_snapshot: FileSnapshot,
    ) -> Self {
        Self {
            source_folder,
            source_path,
            destination_path,
            normalized_filename,
            tmdb_item,
            episode,
            source_extension,
            source_snapshot,
        }
    }

    /// Returns the selected source folder associated with this operation.
    pub fn source_folder(&self) -> &std::path::Path {
        &self.source_folder
    }

    /// Returns the exact source file path.
    pub fn source_path(&self) -> &std::path::Path {
        &self.source_path
    }

    /// Returns the exact destination file path.
    pub fn destination_path(&self) -> &std::path::Path {
        &self.destination_path
    }

    /// Returns the generated filename component retained by the immutable plan.
    pub fn normalized_filename(&self) -> &str {
        &self.normalized_filename
    }

    /// Returns the verified TMDB item used by this operation.
    pub fn tmdb_item(&self) -> &TmdbItem {
        &self.tmdb_item
    }

    /// Returns the verified series episode, or `None` for a movie.
    pub const fn episode(&self) -> Option<EpisodeRef> {
        self.episode
    }

    /// Returns the canonical source-video extension.
    pub fn source_extension(&self) -> &VideoExtension {
        &self.source_extension
    }

    /// Returns the source state captured while planning.
    pub fn source_snapshot(&self) -> &FileSnapshot {
        &self.source_snapshot
    }
}

/// The complete immutable plan shown to the user before any mutation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationPlan {
    source_root: SourceRoot,
    destination: DestinationSelection,
    operation: FileOperation,
    operations: Vec<PlannedOperation>,
    delete_source_folders: bool,
}

impl OperationPlan {
    /// Creates a plan. The filesystem layer validates that it has at least one safe operation.
    pub fn new(
        source_root: SourceRoot,
        destination: DestinationSelection,
        operation: FileOperation,
        operations: Vec<PlannedOperation>,
    ) -> Self {
        Self {
            source_root,
            destination,
            operation,
            operations,
            delete_source_folders: false,
        }
    }

    /// Associates the confirmed folder-cleanup choice with this immutable plan.
    pub fn with_delete_source_folders(mut self, delete_source_folders: bool) -> Self {
        self.delete_source_folders = delete_source_folders;
        self
    }

    /// Returns the source root used for relative display paths.
    pub fn source_root(&self) -> &SourceRoot {
        &self.source_root
    }

    /// Returns the selected destination and its deferred-creation state.
    pub fn destination(&self) -> &DestinationSelection {
        &self.destination
    }

    /// Returns the copy-or-move operation that execution must perform.
    pub const fn operation(&self) -> FileOperation {
        self.operation
    }

    /// Returns operations in the stable order shown by the preview.
    pub fn operations(&self) -> &[PlannedOperation] {
        &self.operations
    }

    /// Returns the number of files in the plan.
    pub fn operation_count(&self) -> usize {
        self.operations.len()
    }

    /// Returns the total number of source bytes represented by this plan.
    pub fn total_size_bytes(&self) -> u64 {
        self.operations.iter().fold(0, |total, operation| {
            total.saturating_add(operation.source_snapshot().size_bytes())
        })
    }

    /// Returns whether successful moves should recursively delete selected files' containing
    /// folders after the last selected file or selected descendant in each folder completes.
    pub const fn delete_source_folders(&self) -> bool {
        self.delete_source_folders
    }
}

/// The final state of one attempted operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperationStatus {
    /// The selected operation published the destination successfully.
    Completed,
    /// The operation failed and no later operation was started.
    Failed { reason: String },
    /// The operation was not started because an earlier operation failed.
    Pending,
}

impl OperationStatus {
    /// Returns whether this result represents a successful complete move.
    pub const fn is_completed(&self) -> bool {
        matches!(self, Self::Completed)
    }

    /// Returns whether this result represents a failure.
    pub const fn is_failed(&self) -> bool {
        matches!(self, Self::Failed { .. })
    }
}

/// The report for every operation attempted after confirmation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationResult {
    source_path: PathBuf,
    destination_path: PathBuf,
    status: OperationStatus,
}

impl OperationResult {
    /// Creates a per-file result.
    pub fn new(source_path: PathBuf, destination_path: PathBuf, status: OperationStatus) -> Self {
        Self {
            source_path,
            destination_path,
            status,
        }
    }

    /// Returns the exact source path associated with the result.
    pub fn source_path(&self) -> &std::path::Path {
        &self.source_path
    }

    /// Returns the exact destination path associated with the result.
    pub fn destination_path(&self) -> &std::path::Path {
        &self.destination_path
    }

    /// Returns the result status.
    pub fn status(&self) -> &OperationStatus {
        &self.status
    }
}

/// The deterministic per-file report produced after confirmation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionReport {
    source_root: SourceRoot,
    operation: FileOperation,
    results: Vec<OperationResult>,
}

impl ExecutionReport {
    /// Creates a report from results in plan order.
    pub fn new(
        source_root: SourceRoot,
        operation: FileOperation,
        results: Vec<OperationResult>,
    ) -> Self {
        Self {
            source_root,
            operation,
            results,
        }
    }

    /// Returns the source root used for relative report paths.
    pub fn source_root(&self) -> &SourceRoot {
        &self.source_root
    }

    /// Returns the operation used to produce this report.
    pub const fn operation(&self) -> FileOperation {
        self.operation
    }

    /// Returns per-file results in the original plan order.
    pub fn results(&self) -> &[OperationResult] {
        &self.results
    }

    /// Returns the number of completed operations.
    pub fn completed_count(&self) -> usize {
        self.results
            .iter()
            .filter(|result| result.status.is_completed())
            .count()
    }

    /// Returns the number of failed operations.
    pub fn failed_count(&self) -> usize {
        self.results
            .iter()
            .filter(|result| result.status.is_failed())
            .count()
    }

    /// Returns the number of operations that were not started.
    pub fn pending_count(&self) -> usize {
        self.results
            .iter()
            .filter(|result| matches!(result.status, OperationStatus::Pending))
            .count()
    }

    /// Returns whether execution completed without failures or pending operations.
    pub fn is_success(&self) -> bool {
        !self.results.is_empty()
            && self.failed_count() == 0
            && self.pending_count() == 0
            && self.completed_count() == self.results.len()
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
    /// A filename extension is outside the shared video-extension policy.
    #[error("The video extension `{extension}` is not recognized.")]
    UnsupportedVideoExtension { extension: String },
}

fn sanitize_domain_text(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_control() {
                '�'
            } else {
                character
            }
        })
        .collect()
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
    /// The saved S3 bucket catalog was intentionally updated by a storage command.
    StorageUpdated,
    /// The confirmed plan completed all file operations successfully.
    Completed,
    /// Execution stopped after one or more operations failed.
    PartiallyCompleted,
}

impl RunOutcome {
    /// Returns the process exit code for this outcome.
    pub const fn exit_code(self) -> i32 {
        match self {
            Self::PartiallyCompleted => 1,
            Self::Cancelled
            | Self::StartupConfigured
            | Self::MediaSelectionReady
            | Self::ConfigurationUpdated
            | Self::StorageUpdated
            | Self::Completed => 0,
        }
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

    #[test]
    fn video_extension_parser_canonicalizes_supported_case_variants() {
        assert_eq!(VideoExtension::parse("MP4").unwrap().as_str(), "mp4");
        assert_eq!(VideoExtension::parse("DVR-MS").unwrap().as_str(), "dvr-ms");
    }

    #[test]
    fn video_extension_parser_rejects_unknown_extensions() {
        assert_eq!(
            VideoExtension::parse("txt"),
            Err(DomainError::UnsupportedVideoExtension {
                extension: "txt".to_owned(),
            })
        );
    }
}
