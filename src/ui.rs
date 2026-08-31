use std::fmt;

use crate::{
    domain::{EpisodeRef, IdentificationMethod, MediaType, TmdbItem, TmdbSearchCandidate},
    error::UiResult,
};

/// The severity of an application-owned terminal message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageLevel {
    /// Neutral information about the current workflow state.
    Info,
    /// A completed or accepted action.
    Success,
    /// A recoverable condition that deserves attention.
    Warning,
    /// An error that prevents the current action from continuing.
    Error,
}

/// Renderer-neutral progress contract for slow network or filesystem work.
pub trait ProgressOutput: fmt::Debug {
    /// Changes the message associated with the activity.
    fn set_message(&self, message: &str);
    /// Finishes the activity with a success message.
    fn finish_success(&self, message: &str);
    /// Finishes the activity with an error message.
    fn finish_error(&self, message: &str);
}

/// Terminal interaction contract used by the application workflow.
///
/// The domain and orchestration layers depend on this trait rather than on dialoguer, indicatif,
/// terminal escape sequences, or a particular terminal implementation. `Option<T>` represents a
/// user cancellation from a prompt; an `Err` represents an actual UI failure.
pub trait InteractiveUi {
    /// Renders the application header and initial context.
    fn show_welcome(&mut self, version: &str) -> UiResult<()>;

    /// Renders a stable wizard step indicator.
    fn show_step(&mut self, current: usize, total: usize, label: &str) -> UiResult<()>;

    /// Asks for a secret without echoing it to the terminal.
    ///
    /// An empty answer may mean "use the masked default" when `default` is present.
    fn ask_masked_secret(
        &mut self,
        prompt: &str,
        default: Option<&str>,
    ) -> UiResult<Option<String>>;

    /// Asks for editable text with an optional visible default.
    fn ask_text(&mut self, prompt: &str, default: Option<&str>) -> UiResult<Option<String>>;

    /// Selects one item, optionally using a searchable selector.
    fn select_one(
        &mut self,
        prompt: &str,
        items: &[String],
        searchable: bool,
    ) -> UiResult<Option<usize>>;

    /// Selects zero or more item positions, optionally filtering long lists first.
    fn select_many(
        &mut self,
        prompt: &str,
        items: &[String],
        searchable: bool,
    ) -> UiResult<Option<Vec<usize>>>;

    /// Asks for confirmation. The caller supplies the default, which should be `false` for file
    /// mutations.
    fn confirm(&mut self, prompt: &str, default: bool) -> UiResult<Option<bool>>;

    /// Renders an application-owned status message.
    fn show_message(&mut self, level: MessageLevel, message: &str) -> UiResult<()>;

    /// Starts a spinner/progress activity for a potentially slow operation.
    fn start_activity(&mut self, message: &str) -> UiResult<Box<dyn ProgressOutput>>;
}

/// TMDB-specific interaction contract used by the identification workflow.
///
/// Keeping these operations separate from the generic selection contract lets the application
/// orchestrate TMDB without importing dialoguer types, while the concrete terminal adapter owns
/// result formatting, numeric text collection, and confirmation wording.
pub trait TmdbInteraction {
    /// Asks whether the user wants text search or manual ID identification.
    fn choose_identification_method(&mut self) -> UiResult<Option<IdentificationMethod>>;

    /// Asks which TMDB namespace a manually entered ID belongs to.
    fn choose_media_type(&mut self) -> UiResult<Option<MediaType>>;

    /// Collects a non-empty search query.
    fn ask_search_query(&mut self) -> UiResult<Option<String>>;

    /// Displays typed TMDB candidates and returns the selected candidate position.
    fn select_tmdb_result(&mut self, candidates: &[TmdbSearchCandidate])
    -> UiResult<Option<usize>>;

    /// Collects a raw ID string so the application can validate it at the domain boundary.
    fn ask_tmdb_id(&mut self) -> UiResult<Option<String>>;

    /// Shows verified details and asks for explicit identification confirmation.
    fn confirm_tmdb_item(&mut self, item: &TmdbItem) -> UiResult<Option<bool>>;

    /// Collects raw season and episode strings for a selected series file.
    fn ask_episode_numbers(&mut self, file_label: &str) -> UiResult<Option<(String, String)>>;

    /// Displays a verified episode result without using its title for naming.
    fn show_verified_episode(&mut self, episode: &EpisodeRef) -> UiResult<()>;
}
