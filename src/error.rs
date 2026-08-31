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
            Self::NonInteractive => 2,
            Self::Ui(_) => 1,
        }
    }
}

/// Result alias for application operations.
pub type AppResult<T> = Result<T, AppError>;

/// Result alias for operations performed by the terminal UI boundary.
pub type UiResult<T> = Result<T, UiError>;
