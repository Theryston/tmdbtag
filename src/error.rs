use std::{io, path::PathBuf};

use serde_json::Error as JsonError;
use thiserror::Error;

use crate::domain::{DomainError, MediaType};

/// Errors raised while collecting or validating startup configuration.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// The user did not provide an API key.
    #[error("A TMDB API key is required.")]
    MissingApiKey,
    /// The selected language is not a plausible TMDB locale tag.
    #[error("The TMDB metadata language must be a locale such as pt-BR or en-US.")]
    InvalidLanguage,
    /// The current user's home directory could not be resolved.
    #[error("The current user's home directory could not be resolved.")]
    HomeDirectoryUnavailable,
    /// The application could not read its configuration file.
    #[error("Cannot read the TMDB configuration file at {path}: {source}")]
    Read {
        /// The path that was read.
        path: PathBuf,
        /// The operating-system error.
        #[source]
        source: io::Error,
    },
    /// The configuration file is not valid JSON or does not match the supported schema.
    #[error(
        "The TMDB configuration file at {path} is invalid: {source}. Run `title-tmdb-file config` to replace it."
    )]
    InvalidFile {
        /// The path containing invalid data.
        path: PathBuf,
        /// The JSON parsing error without echoing the file contents.
        #[source]
        source: JsonError,
    },
    /// The application could not create the private configuration directory.
    #[error("Cannot create the TMDB configuration directory at {path}: {source}")]
    CreateDirectory {
        /// The directory that was requested.
        path: PathBuf,
        /// The operating-system error.
        #[source]
        source: io::Error,
    },
    /// The application could not persist the configuration file.
    #[error("Cannot write the TMDB configuration file at {path}: {source}")]
    Write {
        /// The path that was written.
        path: PathBuf,
        /// The operating-system error.
        #[source]
        source: io::Error,
    },
    /// The in-memory configuration could not be encoded as JSON.
    #[error("Cannot serialize the TMDB configuration: {0}")]
    Serialize(#[source] JsonError),
}

impl ConfigError {
    /// Returns the exit code appropriate for a configuration failure.
    pub const fn exit_code(&self) -> i32 {
        match self {
            Self::MissingApiKey | Self::InvalidLanguage | Self::InvalidFile { .. } => 2,
            Self::HomeDirectoryUnavailable
            | Self::Read { .. }
            | Self::CreateDirectory { .. }
            | Self::Write { .. }
            | Self::Serialize(_) => 1,
        }
    }
}

/// Errors returned by the TMDB transport and response-mapping boundary.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum TmdbError {
    /// The reusable HTTP client could not be initialized.
    #[error("The TMDB HTTP client could not be initialized: {message}")]
    ClientBuild {
        /// A sanitized client-construction explanation.
        message: String,
    },
    /// The configured base URL is not an HTTP(S) URL that can accept path segments.
    #[error("The TMDB base URL is invalid.")]
    InvalidBaseUrl,
    /// TMDB rejected the configured credential.
    #[error(
        "TMDB authentication failed (HTTP {status}). Check the API key with `title-tmdb-file config`."
    )]
    Authentication {
        /// The HTTP status returned by TMDB.
        status: u16,
    },
    /// TMDB asked the client to slow down.
    #[error("TMDB rate limit reached. Retry the request later.")]
    RateLimited {
        /// The optional server-provided delay, retained for callers that want to render it.
        retry_after_seconds: Option<u64>,
    },
    /// A requested TMDB resource does not exist.
    #[error("TMDB could not find {resource}.")]
    NotFound {
        /// A safe resource description such as `movie 550`.
        resource: String,
    },
    /// The selected endpoint returned an explicitly different media namespace.
    #[error("TMDB returned media type {actual}, but {expected} was requested.")]
    MediaTypeMismatch {
        /// The media namespace selected by the user.
        expected: MediaType,
        /// The safe type label returned by TMDB.
        actual: String,
    },
    /// TMDB returned a server-side failure.
    #[error("TMDB returned server error HTTP {status}. Retry later.")]
    Server {
        /// The HTTP status returned by TMDB.
        status: u16,
    },
    /// The request exceeded the configured timeout.
    #[error("The TMDB request timed out while {operation}.")]
    Timeout {
        /// A safe operation description without a URL or credential.
        operation: String,
    },
    /// The request failed before a valid response was received.
    #[error("The TMDB request failed while {operation}.")]
    Network {
        /// A safe operation description without a URL or credential.
        operation: String,
    },
    /// TMDB returned JSON that could not be mapped to the expected response.
    #[error("TMDB returned an invalid response while {operation}: {reason}")]
    InvalidResponse {
        /// A safe operation description without a URL or credential.
        operation: String,
        /// A parser or invariant explanation that does not contain the response body.
        reason: String,
    },
    /// TMDB returned an unhandled HTTP status.
    #[error("TMDB returned HTTP {status} while {operation}.")]
    UnexpectedStatus {
        /// A safe operation description without a URL or credential.
        operation: String,
        /// The unexpected HTTP status.
        status: u16,
    },
    /// A requested episode does not exist in the selected series.
    #[error("TMDB could not find series {series_id} episode S{season:02}E{episode:02}.")]
    EpisodeNotFound {
        /// The verified series ID.
        series_id: u64,
        /// The requested season.
        season: u32,
        /// The requested episode.
        episode: u32,
    },
    /// A search request was given no usable text.
    #[error("The TMDB search query cannot be empty.")]
    EmptySearchQuery,
    /// A search page number was outside the TMDB range.
    #[error("TMDB search pages start at 1.")]
    InvalidSearchPage,
}

impl TmdbError {
    /// Returns whether the error means the saved credential must be replaced.
    pub const fn is_authentication(&self) -> bool {
        matches!(self, Self::Authentication { .. })
    }

    /// Returns the process exit code appropriate for this TMDB failure.
    pub const fn exit_code(&self) -> i32 {
        match self {
            Self::Authentication { .. }
            | Self::InvalidBaseUrl
            | Self::EmptySearchQuery
            | Self::InvalidSearchPage
            | Self::MediaTypeMismatch { .. } => 2,
            Self::ClientBuild { .. }
            | Self::RateLimited { .. }
            | Self::NotFound { .. }
            | Self::Server { .. }
            | Self::Timeout { .. }
            | Self::Network { .. }
            | Self::InvalidResponse { .. }
            | Self::UnexpectedStatus { .. }
            | Self::EpisodeNotFound { .. } => 1,
        }
    }
}

/// Errors raised while composing or parsing metadata-bearing filenames.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum NamingError {
    /// A filename was requested for the wrong TMDB namespace.
    #[error("Cannot generate a {expected} filename for a {actual} TMDB item.")]
    MediaTypeMismatch {
        /// The namespace required by the filename operation.
        expected: MediaType,
        /// The namespace carried by the verified TMDB item.
        actual: MediaType,
    },
    /// No usable title remained after normalization and fallback selection.
    #[error("The TMDB title is empty after filename normalization.")]
    EmptyTitle,
    /// The selected or parsed extension is outside the supported video policy.
    #[error("The video extension `{extension}` is not recognized.")]
    UnsupportedVideoExtension {
        /// A safe representation of the unsupported extension.
        extension: String,
    },
    /// A parsed filename violates the generated filename contract.
    #[error("The generated filename is invalid: {reason}.")]
    InvalidGeneratedFilename {
        /// A concise, static explanation safe for normal CLI output.
        reason: &'static str,
    },
    /// The fixed identity prefix and extension leave no safe title capacity.
    #[error("The generated filename exceeds the supported filename length limit.")]
    FilenameTooLong,
}

/// Errors raised while combining selections, verified metadata, and generated names into a plan.
#[derive(Debug, Error)]
pub enum PlanningError {
    /// No source file was available to organize.
    #[error("The operation plan must contain at least one video file.")]
    EmptyPlan,
    /// A selected source folder unexpectedly contains no files.
    #[error("The source folder {folder} has no selected video files.")]
    NoSelectedFiles { folder: PathBuf },
    /// The same series episode was assigned to more than one selected file.
    #[error(
        "Series {series_id} episode S{season:02}E{episode:02} was assigned more than once (file: {file})."
    )]
    DuplicateSeriesEpisode {
        series_id: u64,
        season: u32,
        episode: u32,
        file: PathBuf,
    },
}

/// Errors raised while resolving the working directory, destination, or selectable media.
#[derive(Debug, Error)]
pub enum FilesystemError {
    /// The process working directory could not be obtained.
    #[error("Cannot obtain the current working directory: {source}")]
    CurrentDirectory {
        /// The operating-system error.
        #[source]
        source: io::Error,
    },
    /// The resolved source root could not be inspected as a directory.
    #[error("Cannot inspect the source root at {path}: {source}")]
    SourceRootMetadata {
        /// The source-root path.
        path: PathBuf,
        /// The operating-system error.
        #[source]
        source: io::Error,
    },
    /// The current working directory resolved to a non-directory path.
    #[error("The current working directory is not a directory: {path}")]
    SourceRootNotDirectory {
        /// The invalid source-root path.
        path: PathBuf,
    },
    /// A directory could not be enumerated.
    #[error("Cannot read directory {path}: {source}")]
    ReadDirectory {
        /// The directory that was being enumerated.
        path: PathBuf,
        /// The operating-system error.
        #[source]
        source: io::Error,
    },
    /// A selected source file or folder could not be inspected.
    #[error("Cannot inspect source path {path}: {cause}")]
    SourceMetadata {
        /// The source path that was inspected.
        path: PathBuf,
        /// The operating-system error.
        #[source]
        cause: io::Error,
    },
    /// The user submitted an empty destination path.
    #[error("The destination path cannot be empty.")]
    EmptyDestination,
    /// The destination string cannot be represented as a safe path input.
    #[error("The destination path `{input}` is invalid: {reason}")]
    InvalidDestination {
        /// The sanitized user input retained for the diagnostic.
        input: String,
        /// The boundary validation reason.
        reason: String,
    },
    /// The destination path could not be inspected.
    #[error("Cannot inspect destination path {path}: {source}")]
    DestinationMetadata {
        /// The destination path.
        path: PathBuf,
        /// The operating-system error.
        #[source]
        source: io::Error,
    },
    /// The destination path already exists as a regular file.
    #[error("The destination path is an existing file, not a directory: {path}")]
    DestinationIsFile {
        /// The existing file path.
        path: PathBuf,
    },
    /// A symbolic link was supplied where a real destination directory is required.
    #[error("The destination path is a symbolic link, which is not supported: {path}")]
    DestinationSymlink {
        /// The symbolic-link path.
        path: PathBuf,
    },
    /// The existing destination has a type that cannot safely be used as a directory.
    #[error("The destination path is not a supported directory: {path}")]
    DestinationUnsupportedType {
        /// The unsupported path.
        path: PathBuf,
    },
    /// A missing destination has a parent that is not a directory.
    #[error("The destination parent is not a directory: {path}")]
    DestinationParentNotDirectory {
        /// The invalid parent path.
        path: PathBuf,
    },
    /// The current source root cannot also be the destination.
    #[error("The destination cannot be the current source directory: {path}")]
    DestinationIsSourceRoot {
        /// The rejected destination path.
        path: PathBuf,
    },
    /// The destination would be inside a selected source folder.
    #[error("The destination cannot be a selected source folder or one of its descendants: {path}")]
    DestinationIsSelectedSource {
        /// The rejected destination path.
        path: PathBuf,
    },
    /// The operation plan contains no files.
    #[error("The operation plan must contain at least one video file.")]
    EmptyOperationPlan,
    /// A selected source folder no longer exists.
    #[error("The selected source folder no longer exists: {path}")]
    SourceFolderNotFound {
        /// The missing source folder.
        path: PathBuf,
    },
    /// A selected source folder is no longer a real directory.
    #[error("The selected source folder is not a real directory: {path}")]
    SourceFolderInvalid {
        /// The invalid source folder.
        path: PathBuf,
    },
    /// A source path selected during planning no longer exists.
    #[error("The selected source file no longer exists: {path}")]
    SourceNotFound {
        /// The missing source path.
        path: PathBuf,
    },
    /// A source path is no longer a regular file.
    #[error("The selected source path is not a regular video file: {path}")]
    SourceNotRegularFile {
        /// The changed source path.
        path: PathBuf,
    },
    /// A source path became a symbolic link after selection.
    #[error("The selected source file became a symbolic link and was not moved: {path}")]
    SourceSymlink {
        /// The unsafe source path.
        path: PathBuf,
    },
    /// A source path no longer has a recognized video extension.
    #[error("The selected source file no longer has a recognized video extension: {path}")]
    SourceUnsupportedExtension {
        /// The source path with the unsupported extension.
        path: PathBuf,
    },
    /// The source file's extension changed after it was selected.
    #[error(
        "The selected source file extension changed for {path}: expected .{expected}, found .{actual}."
    )]
    SourceExtensionChanged {
        /// The changed source path.
        path: PathBuf,
        /// The extension captured in the plan.
        expected: String,
        /// The extension observed during revalidation.
        actual: String,
    },
    /// The source file changed after the plan was built.
    #[error("The selected source file changed after planning: {path}")]
    SourceChanged {
        /// The changed source path.
        path: PathBuf,
    },
    /// A selected file is not inside the source folder recorded for its operation.
    #[error("The source file {source_path} is outside its selected source folder {folder}.")]
    SourceFolderMismatch {
        /// The source path.
        source_path: PathBuf,
        /// The recorded source folder.
        folder: PathBuf,
    },
    /// One source file was included more than once in a plan.
    #[error("The source file appears more than once in the operation plan: {path}")]
    DuplicateSource {
        /// The duplicated source path.
        path: PathBuf,
    },
    /// A generated destination path already exists.
    #[error("The destination file already exists: {path}")]
    DestinationAlreadyExists {
        /// The conflicting destination path.
        path: PathBuf,
    },
    /// Two operations generated the same destination path.
    #[error("Multiple operations generate the same destination file: {path}")]
    DuplicateDestination {
        /// The duplicated destination path.
        path: PathBuf,
    },
    /// A destination changed state between selection and validation.
    #[error("The destination changed after it was selected: {path}")]
    DestinationStateChanged {
        /// The changed destination path.
        path: PathBuf,
    },
    /// The existing destination directory cannot be written safely.
    #[error("The destination directory is not writable: {path}")]
    DestinationNotWritable {
        /// The destination directory.
        path: PathBuf,
    },
    /// The plan does not have permission to create the selected destination.
    #[error("The destination does not exist and was not approved for creation: {path}")]
    DestinationCreationNotAllowed {
        /// The deferred destination path.
        path: PathBuf,
    },
    /// A destination could not be created at the commit point.
    #[error("Cannot create the destination directory at {path}: {cause}")]
    DestinationCreation {
        /// The destination directory.
        path: PathBuf,
        /// The operating-system error.
        #[source]
        cause: io::Error,
    },
    /// A generated destination escaped the selected destination directory.
    #[error("The generated destination is outside the selected destination directory: {path}")]
    DestinationEscapes {
        /// The unsafe destination path.
        path: PathBuf,
    },
    /// A source and destination path refer to the same file.
    #[error("The source and destination refer to the same file: {path}")]
    SourceDestinationSame {
        /// The conflicting path.
        path: PathBuf,
    },
    /// A no-replace same-volume publication failed.
    #[error("Cannot safely move {source_path} to {destination}: {cause}")]
    SameVolumeMove {
        /// The source path.
        source_path: PathBuf,
        /// The destination path.
        destination: PathBuf,
        /// The operating-system error.
        #[source]
        cause: io::Error,
    },
    /// A cross-volume copy or its temporary-file lifecycle failed.
    #[error("Cannot safely copy {source_path} to {destination}: {reason}")]
    CrossVolumeCopy {
        /// The source path.
        source_path: PathBuf,
        /// The destination path.
        destination: PathBuf,
        /// A safe operation description.
        reason: String,
    },
    /// The copied bytes did not match the source snapshot.
    #[error("The copy from {source_path} to {destination} could not be verified: {reason}")]
    CopyVerification {
        /// The source path.
        source_path: PathBuf,
        /// The destination path.
        destination: PathBuf,
        /// A safe verification explanation.
        reason: String,
    },
    /// The temporary copy could not be published without replacement.
    #[error("Cannot publish the copied file at {path}: {cause}")]
    DestinationPublication {
        /// The final destination path.
        path: PathBuf,
        /// The operating-system error.
        #[source]
        cause: io::Error,
    },
    /// The verified destination exists but the source could not be removed.
    #[error(
        "The destination was published, but the source could not be removed at {path}: {cause}"
    )]
    SourceRemoval {
        /// The source path that remains.
        path: PathBuf,
        /// The operating-system error.
        #[source]
        cause: io::Error,
    },
    /// A temporary artifact could not be removed after a failed operation.
    #[error("A temporary file could not be cleaned up at {path}: {cause}")]
    TemporaryCleanup {
        /// The temporary artifact path.
        path: PathBuf,
        /// The operating-system error.
        #[source]
        cause: io::Error,
    },
}

impl FilesystemError {
    /// Returns the process exit code appropriate for a filesystem failure.
    pub const fn exit_code(&self) -> i32 {
        match self {
            Self::EmptyDestination
            | Self::InvalidDestination { .. }
            | Self::DestinationIsFile { .. }
            | Self::DestinationSymlink { .. }
            | Self::DestinationUnsupportedType { .. }
            | Self::DestinationParentNotDirectory { .. }
            | Self::DestinationIsSourceRoot { .. }
            | Self::DestinationIsSelectedSource { .. }
            | Self::EmptyOperationPlan
            | Self::SourceFolderNotFound { .. }
            | Self::SourceFolderInvalid { .. }
            | Self::SourceNotFound { .. }
            | Self::SourceNotRegularFile { .. }
            | Self::SourceSymlink { .. }
            | Self::SourceUnsupportedExtension { .. }
            | Self::SourceExtensionChanged { .. }
            | Self::SourceChanged { .. }
            | Self::SourceFolderMismatch { .. }
            | Self::DuplicateSource { .. }
            | Self::DestinationAlreadyExists { .. }
            | Self::DuplicateDestination { .. }
            | Self::DestinationStateChanged { .. }
            | Self::DestinationNotWritable { .. }
            | Self::DestinationCreationNotAllowed { .. }
            | Self::DestinationEscapes { .. }
            | Self::SourceDestinationSame { .. } => 2,
            Self::CurrentDirectory { .. }
            | Self::SourceRootMetadata { .. }
            | Self::SourceRootNotDirectory { .. }
            | Self::ReadDirectory { .. }
            | Self::DestinationMetadata { .. }
            | Self::SourceMetadata { .. }
            | Self::DestinationCreation { .. }
            | Self::SameVolumeMove { .. }
            | Self::CrossVolumeCopy { .. }
            | Self::CopyVerification { .. }
            | Self::DestinationPublication { .. }
            | Self::SourceRemoval { .. }
            | Self::TemporaryCleanup { .. } => 1,
        }
    }
}

/// Errors raised at the interactive terminal boundary.
#[derive(Debug, Error)]
pub enum UiError {
    /// The process was started without interactive input and output streams.
    #[error("The interactive terminal is unavailable.")]
    NotInteractive,
    /// The user interrupted a prompt in a context that cannot represent cancellation as a value.
    #[error("The interactive prompt was canceled.")]
    Canceled,
    /// A prompt or terminal operation failed.
    #[error("The interactive prompt failed: {0}")]
    Prompt(#[source] io::Error),
    /// A selection prompt was asked to render without any options.
    #[error("Cannot display an empty {context} selection.")]
    EmptySelection { context: &'static str },
    /// A UI implementation returned a position outside the options it was given.
    #[error("The interactive selection returned an invalid {context}.")]
    InvalidSelection { context: &'static str },
    /// The progress renderer could not be configured.
    #[error("The progress indicator could not be configured: {0}")]
    ProgressStyle(String),
}

impl UiError {
    /// Converts a dialoguer error while preserving Ctrl-C as a cancellation value.
    pub fn from_dialoguer(error: dialoguer::Error) -> Self {
        let io_error: io::Error = error.into();
        if io_error.kind() == io::ErrorKind::Interrupted {
            Self::Canceled
        } else {
            Self::Prompt(io_error)
        }
    }
}

/// Errors that can terminate the application before a successful outcome.
#[derive(Debug, Error)]
pub enum AppError {
    /// Startup configuration was invalid.
    #[error(transparent)]
    Configuration(#[from] ConfigError),
    /// An input could not become a valid domain value.
    #[error(transparent)]
    Domain(#[from] DomainError),
    /// TMDB rejected a request or returned an unusable response.
    #[error(transparent)]
    Tmdb(#[from] TmdbError),
    /// Filename generation or parsing failed.
    #[error(transparent)]
    Naming(#[from] NamingError),
    /// The selections and verified metadata cannot form one safe operation plan.
    #[error(transparent)]
    Planning(#[from] PlanningError),
    /// Filesystem discovery or destination validation failed.
    #[error(transparent)]
    Filesystem(#[from] FilesystemError),
    /// The normal wizard requires an interactive terminal.
    #[error("This command requires an interactive terminal with stdin and stderr attached.")]
    NonInteractive,
    /// The UI boundary failed while collecting or rendering interaction state.
    #[error(transparent)]
    Ui(#[from] UiError),
}

impl AppError {
    /// Maps the typed application error to the documented CLI exit-code contract.
    pub const fn exit_code(&self) -> i32 {
        match self {
            Self::Configuration(error) => error.exit_code(),
            Self::Domain(_) => 2,
            Self::Tmdb(error) => error.exit_code(),
            Self::Naming(_) => 2,
            Self::Planning(_) => 2,
            Self::Filesystem(error) => error.exit_code(),
            Self::NonInteractive => 2,
            Self::Ui(_) => 1,
        }
    }
}

/// Result alias for application operations.
pub type AppResult<T> = Result<T, AppError>;

/// Result alias for operations performed by the terminal UI boundary.
pub type UiResult<T> = Result<T, UiError>;
