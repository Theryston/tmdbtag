use std::{
    fmt,
    path::{Component, Path, PathBuf},
};

use crate::{
    domain::{
        DestinationSelection, EpisodeRef, ExecutionReport, IdentificationMethod, MediaType,
        OperationPlan, SourceRoot, TmdbItem, TmdbSearchCandidate, VideoFile,
    },
    error::{TmdbError, UiResult},
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

    /// Shows the active file line before its metadata prompts.
    ///
    /// Every TMDB and episode prompt for that file is rendered after this call and before
    /// `finish_file_context`, so a user can always tell which file is being processed. Previous
    /// file contexts and their logs remain visible as an execution history.
    fn show_file_context(
        &mut self,
        current_file: usize,
        total_files: usize,
        file_path: &Path,
        source_root: &Path,
    ) -> UiResult<()> {
        self.show_message(
            MessageLevel::Info,
            &format!(
                "File {current_file}/{total_files}: {}",
                display_relative_path(file_path, source_root)
            ),
        )
    }

    /// Ends the active file context without clearing its output.
    ///
    /// The next `show_file_context` call appends a new context while preserving the previous
    /// file's identification output and logs.
    fn finish_file_context(&mut self) -> UiResult<()> {
        Ok(())
    }

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

    /// Displays the complete immutable plan before the final mutation confirmation.
    fn show_plan_preview(&mut self, plan: &OperationPlan) -> UiResult<()> {
        self.show_message(
            MessageLevel::Info,
            &format!(
                "Prepared an operation plan for {} file(s).",
                plan.operation_count()
            ),
        )
    }

    /// Displays one final result for every operation after execution.
    fn show_execution_report(&mut self, report: &ExecutionReport) -> UiResult<()> {
        self.show_message(
            MessageLevel::Info,
            &format!(
                "Completed: {} · Failed: {} · Pending: {}",
                report.completed_count(),
                report.failed_count(),
                report.pending_count()
            ),
        )
    }

    /// Collects the destination path before source-root media discovery begins.
    fn ask_destination_path(&mut self) -> UiResult<Option<String>> {
        self.ask_text("Destination folder path", None)
    }

    /// Confirms that a missing destination may be created later at the mutation commit point.
    fn confirm_destination_creation(
        &mut self,
        source_root: &SourceRoot,
        destination: &DestinationSelection,
    ) -> UiResult<Option<bool>> {
        self.confirm(
            &format!(
                "Allow creation of destination {} after final confirmation?",
                display_relative_path(destination.path(), source_root.path())
            ),
            false,
        )
    }

    /// Presents all recursively discovered videos in one expandable file explorer and returns
    /// positions into the flat `files` slice.
    fn select_video_files(
        &mut self,
        source_root: &SourceRoot,
        files: &[VideoFile],
    ) -> UiResult<Option<Vec<usize>>> {
        let items = files
            .iter()
            .map(|file| {
                let size = file
                    .size_bytes()
                    .map(|size| format!(" · {}", format_file_size(size)))
                    .unwrap_or_default();
                format!(
                    "{}{}",
                    display_relative_path(file.path(), source_root.path()),
                    size
                )
            })
            .collect::<Vec<_>>();
        self.select_many("Select video files", &items, true)
    }
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

    /// Runs a debounced, keyboard-driven TMDB search and returns the selected candidate.
    ///
    /// The UI owns the query editor, debounce timing, result rendering, and keyboard selection.
    /// The application supplies the search callback so HTTP transport and TMDB policy remain
    /// outside the terminal adapter. The nested result distinguishes UI failures from typed TMDB
    /// request failures, allowing the application to offer its normal retry policy.
    fn select_tmdb_result_live(
        &mut self,
        search: &mut dyn FnMut(&str) -> Result<Vec<TmdbSearchCandidate>, TmdbError>,
    ) -> UiResult<Result<Option<TmdbSearchCandidate>, TmdbError>>;

    /// Collects a raw ID string so the application can validate it at the domain boundary.
    fn ask_tmdb_id(&mut self) -> UiResult<Option<String>>;

    /// Shows verified details and asks for explicit identification confirmation.
    fn confirm_tmdb_item(&mut self, item: &TmdbItem) -> UiResult<Option<bool>>;

    /// Collects raw season and episode strings for a selected series file.
    fn ask_episode_numbers(&mut self, file_label: &str) -> UiResult<Option<(String, String)>>;

    /// Displays a verified episode result without using its title for naming.
    fn show_verified_episode(&mut self, episode: &EpisodeRef) -> UiResult<()>;
}

/// Converts a filesystem path into terminal-safe display text without changing the path value.
fn safe_display_path(path: &Path) -> String {
    safe_display_text(&path.to_string_lossy())
}

/// Converts a path into display text relative to a known base path.
pub fn display_relative_path(path: &Path, base: &Path) -> String {
    safe_display_path(&relative_path(path, base))
}

fn relative_path(path: &Path, base: &Path) -> PathBuf {
    let path_components: Vec<_> = path.components().collect();
    let base_components: Vec<_> = base.components().collect();
    let common_length = path_components
        .iter()
        .zip(base_components.iter())
        .take_while(|(path_component, base_component)| path_component == base_component)
        .count();

    if common_length == 0
        && matches!(
            (path_components.first(), base_components.first()),
            (Some(Component::Prefix(_)), Some(Component::Prefix(_)))
        )
    {
        // Different Windows drive prefixes do not have a meaningful `..` representation.
        return path.to_owned();
    }

    let mut relative = PathBuf::new();
    for component in base_components.iter().skip(common_length) {
        if matches!(component, Component::Prefix(_) | Component::RootDir) {
            return path.to_owned();
        }
        relative.push("..");
    }
    for component in path_components.iter().skip(common_length) {
        match component {
            Component::Prefix(_) | Component::RootDir | Component::CurDir => {}
            Component::ParentDir | Component::Normal(_) => relative.push(component.as_os_str()),
        }
    }

    if relative.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        relative
    }
}

fn safe_display_text(value: &str) -> String {
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

fn format_file_size(size_bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut value = size_bytes as f64;
    let mut unit_index = 0;

    while value >= 1024.0 && unit_index < UNITS.len() - 1 {
        value /= 1024.0;
        unit_index += 1;
    }

    if unit_index == 0 {
        format!("{size_bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit_index])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_display_paths_use_dot_for_the_base_and_parent_segments_outside_it() {
        assert_eq!(
            display_relative_path(Path::new("root"), Path::new("root")),
            "."
        );
        assert_eq!(
            display_relative_path(Path::new("root/series/episode.mp4"), Path::new("root")),
            "series/episode.mp4"
        );
        assert_eq!(
            display_relative_path(Path::new("organized"), Path::new("root")),
            "../organized"
        );
    }
}
