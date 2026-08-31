use std::io;

use thiserror::Error;

/// Errors raised while collecting or validating startup configuration.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ConfigError {
    /// The user did not provide an API key.
    #[error("A TMDB API key is required.")]
    MissingApiKey,
    /// The selected language is not a plausible TMDB locale tag.
    #[error("The TMDB metadata language must be a locale such as pt-BR or en-US.")]
    InvalidLanguage,
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
            Self::Configuration(_) | Self::NonInteractive => 2,
            Self::Ui(_) => 1,
        }
    }
}

/// Result alias for application operations.
pub type AppResult<T> = Result<T, AppError>;

/// Result alias for operations performed by the terminal UI boundary.
pub type UiResult<T> = Result<T, UiError>;
