use std::{io, path::PathBuf};

use serde_json::Error as JsonError;
use thiserror::Error;

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
            Self::NonInteractive => 2,
            Self::Ui(_) => 1,
        }
    }
}

/// Result alias for application operations.
pub type AppResult<T> = Result<T, AppError>;

/// Result alias for operations performed by the terminal UI boundary.
pub type UiResult<T> = Result<T, UiError>;
