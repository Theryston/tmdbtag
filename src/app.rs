use std::{
    collections::HashSet,
    path::{Component, Path},
};

use crate::{
    config::{ConfigPromptMode, ConfigStore, StartupConfig, configure_interactively},
    domain::{
        EpisodeRef, FileOperation, FilesystemSelection, IdentificationMethod, OperationPlan,
        PlannedOperation, RunOutcome, SelectedSource, SourceFolder, SourceRoot, TmdbId, TmdbItem,
        TmdbSearchCandidate,
    },
    error::{AppError, AppResult, PlanningError, TmdbError, UiError},
    filesystem::{self, DiscoveryWarning},
    naming::{generate_movie_filename, generate_series_filename},
    tmdb::client::TmdbClient,
    ui::{InteractiveUi, MessageLevel, TmdbInteraction, display_relative_path},
};

/// The metadata operations required by the organization workflow.
pub trait TmdbProvider {
    /// Searches the movie namespace.
    fn search_movies(&self, query: &str) -> Result<Vec<TmdbSearchCandidate>, TmdbError>;

    /// Searches the TV-series namespace.
    fn search_series(&self, query: &str) -> Result<Vec<TmdbSearchCandidate>, TmdbError>;

    /// Fetches verified details for one typed TMDB ID.
    fn get_item(
        &self,
        media_type: crate::domain::MediaType,
        id: TmdbId,
    ) -> Result<TmdbItem, TmdbError>;

    /// Validates one series episode against TMDB.
    fn get_episode_details(
        &self,
        series_id: TmdbId,
        episode: EpisodeRef,
    ) -> Result<crate::domain::TmdbEpisode, TmdbError>;
}

impl TmdbProvider for TmdbClient {
    fn search_movies(&self, query: &str) -> Result<Vec<TmdbSearchCandidate>, TmdbError> {
        TmdbClient::search_movies(self, query)
    }

    fn search_series(&self, query: &str) -> Result<Vec<TmdbSearchCandidate>, TmdbError> {
        TmdbClient::search_series(self, query)
    }

    fn get_item(
        &self,
        media_type: crate::domain::MediaType,
        id: TmdbId,
    ) -> Result<TmdbItem, TmdbError> {
        TmdbClient::get_item(self, media_type, id)
    }

    fn get_episode_details(
        &self,
        series_id: TmdbId,
        episode: EpisodeRef,
    ) -> Result<crate::domain::TmdbEpisode, TmdbError> {
        TmdbClient::get_episode_details(self, series_id, episode)
    }
}

/// Runs the default interactive workflow using the current user's configuration file.
pub fn run<U: InteractiveUi + TmdbInteraction>(ui: &mut U, version: &str) -> AppResult<RunOutcome> {
    let store = ConfigStore::for_current_user()?;
    run_with_store(ui, version, &store)
}

/// Runs the default workflow with an explicit configuration store.
///
/// The explicit-store boundary keeps orchestration tests isolated from the real home directory
/// while the production entry point continues to use the documented per-user location.
pub fn run_with_store<U: InteractiveUi + TmdbInteraction>(
    ui: &mut U,
    version: &str,
    store: &ConfigStore,
) -> AppResult<RunOutcome> {
    run_with_store_and_validator(ui, version, store, |config| {
        TmdbClient::new(config)?.validate_credentials()
    })
}

fn run_with_store_and_validator<U, F>(
    ui: &mut U,
    version: &str,
    store: &ConfigStore,
    validate: F,
) -> AppResult<RunOutcome>
where
    U: InteractiveUi + TmdbInteraction,
    F: FnMut(&StartupConfig) -> Result<(), TmdbError>,
{
    ui.show_welcome(version)?;

    let Some(startup_config) =
        configure_and_validate(ui, store, ConfigPromptMode::MissingOnly, validate)?
    else {
        ui.show_message(MessageLevel::Info, "Startup configuration canceled.")?;
        return Ok(RunOutcome::Cancelled);
    };

    ui.show_message(MessageLevel::Success, "TMDB configuration is ready.")?;
    let client = TmdbClient::new(&startup_config)?;

    loop {
        let Some(filesystem_selection) = collect_filesystem_selection(ui)? else {
            ui.show_message(MessageLevel::Info, "Filesystem selection canceled.")?;
            return Ok(RunOutcome::Cancelled);
        };
        ui.show_message(MessageLevel::Success, "Filesystem selection is ready.")?;

        match organize_selection(ui, &filesystem_selection, &client) {
            Ok(outcome) => return Ok(outcome),
            Err(error) if is_recoverable_organization_error(&error) => {
                ui.show_message(MessageLevel::Error, &error.to_string())?;
                let Some(retry) =
                    ui.confirm("Restart source selection and rebuild the plan?", true)?
                else {
                    return Ok(RunOutcome::Cancelled);
                };
                if retry {
                    continue;
                }
                ui.show_message(
                    MessageLevel::Info,
                    "Organization canceled. No files were changed.",
                )?;
                return Ok(RunOutcome::Cancelled);
            }
            Err(error) => return Err(error),
        }
    }
}

/// Runs the `config` command, which deliberately reopens both shared configuration prompts.
pub fn run_config<U: InteractiveUi>(ui: &mut U, version: &str) -> AppResult<RunOutcome> {
    let store = ConfigStore::for_current_user()?;
    run_config_with_store(ui, version, &store)
}

/// Runs the `config` command with an explicit configuration store for isolated tests.
pub fn run_config_with_store<U: InteractiveUi>(
    ui: &mut U,
    version: &str,
    store: &ConfigStore,
) -> AppResult<RunOutcome> {
    run_config_with_store_and_validator(ui, version, store, |config| {
        TmdbClient::new(config)?.validate_credentials()
    })
}

fn run_config_with_store_and_validator<U, F>(
    ui: &mut U,
    version: &str,
    store: &ConfigStore,
    validate: F,
) -> AppResult<RunOutcome>
where
    U: InteractiveUi,
    F: FnMut(&StartupConfig) -> Result<(), TmdbError>,
{
    ui.show_welcome(version)?;

    let Some(_startup_config) =
        configure_and_validate(ui, store, ConfigPromptMode::ReplaceAll, validate)?
    else {
        ui.show_message(MessageLevel::Info, "Configuration update canceled.")?;
        return Ok(RunOutcome::Cancelled);
    };

    ui.show_message(MessageLevel::Success, "TMDB configuration saved.")?;
    ui.show_message(
        MessageLevel::Info,
        &format!("Configuration file: {}", store.path().display()),
    )?;

    Ok(RunOutcome::ConfigurationUpdated)
}

fn configure_and_validate<U, F>(
    ui: &mut U,
    store: &ConfigStore,
    initial_mode: ConfigPromptMode,
    mut validate: F,
) -> AppResult<Option<StartupConfig>>
where
    U: InteractiveUi,
    F: FnMut(&StartupConfig) -> Result<(), TmdbError>,
{
    let mut mode = initial_mode;

    loop {
        let Some(config) = configure_interactively(ui, store, mode)? else {
            return Ok(None);
        };

        let activity = ui.start_activity("Validating TMDB configuration...")?;
        match validate(&config) {
            Ok(()) => {
                activity.finish_success("TMDB configuration verified.");
                return Ok(Some(config));
            }
            Err(error) => {
                activity.finish_error("TMDB configuration verification failed.");
                ui.show_message(MessageLevel::Error, &error.to_string())?;

                let retry_mode = if error.is_authentication() {
                    ConfigPromptMode::RepairApiKey
                } else {
                    mode
                };
                let Some(should_retry) =
                    ui.confirm("Retry TMDB configuration validation?", true)?
                else {
                    return Ok(None);
                };
                if !should_retry {
                    return Ok(None);
                }
                mode = retry_mode;
            }
        }
    }
}

/// Collects a complete, non-mutating filesystem selection from the process current directory.
pub fn collect_filesystem_selection<U: InteractiveUi>(
    ui: &mut U,
) -> AppResult<Option<FilesystemSelection>> {
    let source_root = filesystem::current_source_root()?;
    collect_filesystem_selection_from_root(ui, source_root)
}

fn collect_filesystem_selection_from_root<U: InteractiveUi>(
    ui: &mut U,
    source_root: SourceRoot,
) -> AppResult<Option<FilesystemSelection>> {
    ui.show_message(
        MessageLevel::Info,
        &format!(
            "Source root: current directory ({})",
            display_relative_path(source_root.path(), source_root.path())
        ),
    )?;

    ui.show_step(1, 6, "Choose the file operation")?;
    let Some(operation) = ui.choose_file_operation()? else {
        return Ok(None);
    };
    ui.show_message(
        MessageLevel::Info,
        &format!(
            "Operation: {} ({}).",
            operation.label(),
            if operation.preserves_source() {
                "original files will be kept"
            } else {
                "original files will be removed after successful publication"
            }
        ),
    )?;

    'destination: loop {
        ui.show_step(2, 6, "Choose the destination folder")?;
        let Some(raw_destination) = ui.ask_destination_path()? else {
            return Ok(None);
        };
        let destination = match filesystem::resolve_destination(&source_root, &raw_destination) {
            Ok(destination) => destination,
            Err(error) => {
                ui.show_message(MessageLevel::Error, &error.to_string())?;
                continue 'destination;
            }
        };

        if !destination.exists() {
            let Some(allow_creation) =
                ui.confirm_destination_creation(&source_root, &destination)?
            else {
                return Ok(None);
            };
            if !allow_creation {
                ui.show_message(
                    MessageLevel::Info,
                    "The destination will not be created. Choose another destination.",
                )?;
                continue 'destination;
            }
        }

        let destination_status = if destination.exists() {
            "existing directory"
        } else {
            "will be created after final confirmation"
        };
        ui.show_message(
            MessageLevel::Info,
            &format!(
                "Destination: {} ({destination_status})",
                display_relative_path(destination.path(), source_root.path())
            ),
        )?;

        let videos = filesystem::discover_video_files_in_source_root(&source_root, &destination)?;
        show_discovery_warnings(ui, videos.warnings(), source_root.path())?;
        if videos.items().is_empty() {
            ui.show_message(
                MessageLevel::Warning,
                "No eligible video files were found in the current directory or its subfolders.",
            )?;
            let Some(choose_again) = ui.confirm("Choose a different destination?", true)? else {
                return Ok(None);
            };
            if choose_again {
                continue 'destination;
            }
            return Ok(None);
        }

        loop {
            ui.show_step(3, 6, "Select video files")?;
            let Some(file_indices) = ui.select_video_files(&source_root, videos.items())? else {
                return Ok(None);
            };
            if file_indices.is_empty() {
                ui.show_message(
                    MessageLevel::Error,
                    "Select at least one video file to continue.",
                )?;
                continue;
            }
            validate_selection_indices(&file_indices, videos.items().len(), "video file")?;

            let mut sorted_file_indices = file_indices;
            sorted_file_indices.sort_unstable();
            let selected_files = sorted_file_indices
                .into_iter()
                .map(|file_index| videos.items()[file_index].path().to_owned())
                .collect::<Vec<_>>();
            let selected_sources = build_selected_sources(&source_root, selected_files);

            let selected_folders = selected_sources
                .iter()
                .filter(|source| source.folder() != source_root.path())
                .map(|source| SourceFolder::new(source.folder().to_owned()))
                .collect::<Vec<_>>();
            if let Err(error) =
                filesystem::validate_destination_against_sources(&destination, &selected_folders)
            {
                ui.show_message(MessageLevel::Error, &error.to_string())?;
                let Some(try_again) = ui.confirm("Choose video files again?", true)? else {
                    return Ok(None);
                };
                if try_again {
                    continue;
                }
                return Ok(None);
            }

            return Ok(Some(FilesystemSelection::new(
                source_root,
                destination,
                operation,
                selected_sources,
            )));
        }
    }
}

fn build_selected_sources(
    source_root: &SourceRoot,
    selected_files: Vec<std::path::PathBuf>,
) -> Vec<SelectedSource> {
    selected_files
        .into_iter()
        .map(|file| {
            let source_folder = source_container_for_file(source_root, &file);
            SelectedSource::new(source_folder, vec![file])
        })
        .collect()
}

fn source_container_for_file(source_root: &SourceRoot, file: &Path) -> std::path::PathBuf {
    let Some(relative) = file.strip_prefix(source_root.path()).ok() else {
        return source_root.path().to_owned();
    };

    let mut components = relative.components();
    let Some(first) = components.next() else {
        return source_root.path().to_owned();
    };
    if components.next().is_none() {
        return source_root.path().to_owned();
    }

    match first {
        Component::Normal(name) => source_root.path().join(name),
        _ => source_root.path().to_owned(),
    }
}

fn validate_selection_indices(
    indices: &[usize],
    item_count: usize,
    context: &'static str,
) -> AppResult<()> {
    let has_out_of_range = indices.iter().any(|&index| index >= item_count);
    let has_duplicate = indices
        .iter()
        .enumerate()
        .any(|(position, index)| indices[..position].contains(index));
    if has_out_of_range || has_duplicate {
        return Err(AppError::Ui(UiError::InvalidSelection { context }));
    }

    Ok(())
}

fn show_discovery_warnings<U: InteractiveUi>(
    ui: &mut U,
    warnings: &[DiscoveryWarning],
    display_base: &std::path::Path,
) -> AppResult<()> {
    for warning in warnings {
        ui.show_message(
            MessageLevel::Warning,
            &format!(
                "Skipped {}: {}",
                display_relative_path(warning.path(), display_base),
                warning.reason()
            ),
        )?;
    }
    Ok(())
}

/// Identifies every selected video, builds the complete plan, previews it, and executes it only
/// after explicit confirmation.
pub fn organize_selection<U, C>(
    ui: &mut U,
    selection: &FilesystemSelection,
    client: &C,
) -> AppResult<RunOutcome>
where
    U: InteractiveUi + TmdbInteraction,
    C: TmdbProvider,
{
    ui.show_step(4, 6, "Identify media and validate episodes")?;
    let Some(plan) = build_operation_plan(ui, selection, client)? else {
        ui.show_message(
            MessageLevel::Info,
            "Organization canceled. No files were changed.",
        )?;
        return Ok(RunOutcome::Cancelled);
    };

    ui.show_step(5, 6, "Review and validate the operation plan")?;
    filesystem::validate_operation_plan(&plan)?;
    ui.show_plan_preview(&plan)?;

    let Some(confirmed) = ui.confirm(
        &format!(
            "{} and rename {} files?",
            plan.operation().label(),
            plan.operation_count()
        ),
        false,
    )?
    else {
        ui.show_message(
            MessageLevel::Info,
            "Operation canceled. No files were changed.",
        )?;
        return Ok(RunOutcome::Cancelled);
    };
    if !confirmed {
        ui.show_message(
            MessageLevel::Info,
            "Operation canceled. No files were changed.",
        )?;
        return Ok(RunOutcome::Cancelled);
    }

    let operation = plan.operation();
    ui.show_step(6, 6, &format!("{} and rename files", operation.label()))?;
    let activity = ui.start_activity(&format!(
        "{} and renaming files...",
        match operation {
            FileOperation::Copy => "Copying",
            FileOperation::Move => "Moving",
        }
    ))?;
    activity.set_progress(0, plan.total_size_bytes());
    let total = plan.operation_count();
    let source_root = plan.source_root().path();
    let execution = filesystem::execute_operation_plan_with_progress(&plan, |progress| {
        let index = progress.operation_index();
        let operation = &plan.operations()[index];
        activity.set_message(&format!(
            "{} file {}/{}: {}",
            match plan.operation() {
                FileOperation::Copy => "Copying",
                FileOperation::Move => "Moving",
            },
            index + 1,
            total,
            display_relative_path(operation.source_path(), source_root)
        ));
        activity.set_progress(progress.completed_bytes(), progress.total_bytes());
    });
    let report = match execution {
        Ok(report) => report,
        Err(error) => {
            activity.finish_error("The file operation could not start safely.");
            return Err(error.into());
        }
    };
    if report.is_success() {
        activity.finish_success(&format!(
            "All files {} successfully.",
            match operation {
                FileOperation::Copy => "copied",
                FileOperation::Move => "moved",
            }
        ));
    } else {
        activity.finish_error(&format!(
            "File {} stopped before every file was completed.",
            match operation {
                FileOperation::Copy => "copying",
                FileOperation::Move => "movement",
            }
        ));
    }
    ui.show_execution_report(&report)?;
    if report.is_success() {
        Ok(RunOutcome::Completed)
    } else {
        Ok(RunOutcome::PartiallyCompleted)
    }
}

fn build_operation_plan<U, C>(
    ui: &mut U,
    selection: &FilesystemSelection,
    client: &C,
) -> AppResult<Option<OperationPlan>>
where
    U: InteractiveUi + TmdbInteraction,
    C: TmdbProvider,
{
    let mut operations = Vec::new();
    let mut episode_keys = HashSet::new();
    let total_files = selection
        .sources()
        .iter()
        .map(|source| source.files().len())
        .sum::<usize>();
    let mut current_file = 0;

    for source in selection.sources() {
        if source.files().is_empty() {
            return Err(PlanningError::NoSelectedFiles {
                folder: source.folder().to_owned(),
            }
            .into());
        }

        for source_path in source.files() {
            current_file += 1;
            ui.show_file_context(
                current_file,
                total_files,
                source_path,
                selection.source_root().path(),
            )?;
            let operation = build_selected_file_operation(
                ui,
                source,
                source_path,
                selection.destination().path(),
                selection.source_root().path(),
                client,
                &mut episode_keys,
            );
            ui.finish_file_context()?;
            let Some(operation) = operation? else {
                return Ok(None);
            };
            operations.push(operation);
        }
    }

    if operations.is_empty() {
        return Err(PlanningError::EmptyPlan.into());
    }

    Ok(Some(OperationPlan::new(
        selection.source_root().clone(),
        selection.destination().clone(),
        selection.operation(),
        operations,
    )))
}

fn build_selected_file_operation<U, C>(
    ui: &mut U,
    source: &SelectedSource,
    source_path: &std::path::Path,
    destination: &std::path::Path,
    source_root: &std::path::Path,
    client: &C,
    episode_keys: &mut HashSet<(TmdbId, EpisodeRef)>,
) -> AppResult<Option<PlannedOperation>>
where
    U: InteractiveUi + TmdbInteraction,
    C: TmdbProvider,
{
    let Some(item) = identify_tmdb_item(ui, client)? else {
        return Ok(None);
    };

    let episode = match item.media_type {
        crate::domain::MediaType::Movie => None,
        crate::domain::MediaType::Series => {
            let file_label = display_relative_path(source_path, source_root);
            let episode = loop {
                let Some(episode) = collect_series_episode(ui, client, item.id, &file_label)?
                else {
                    return Ok(None);
                };
                let key = (item.id, episode);
                if episode_keys.insert(key) {
                    break episode;
                }

                ui.show_message(
                    MessageLevel::Error,
                    &format!(
                        "Series {} episode S{:02}E{:02} was already assigned. Enter a different episode.",
                        item.id,
                        episode.season(),
                        episode.episode()
                    ),
                )?;
            };
            Some(episode)
        }
    };

    Ok(Some(build_planned_operation(
        source,
        source_path,
        destination,
        item,
        episode,
    )?))
}

fn build_planned_operation(
    source: &SelectedSource,
    source_path: &std::path::Path,
    destination: &std::path::Path,
    item: TmdbItem,
    episode: Option<EpisodeRef>,
) -> AppResult<PlannedOperation> {
    let source_extension = filesystem::source_video_extension(source_path)?;
    let normalized_filename = match episode {
        Some(episode) => generate_series_filename(&item, episode, source_extension.as_str())?,
        None => generate_movie_filename(&item, source_extension.as_str())?,
    };
    let source_snapshot = filesystem::snapshot_source_file(source_path)?;
    let destination_path = destination.join(&normalized_filename);

    Ok(PlannedOperation::new(
        source.folder().to_owned(),
        source_path.to_owned(),
        destination_path,
        normalized_filename,
        item,
        episode,
        source_extension,
        source_snapshot,
    ))
}

fn is_recoverable_organization_error(error: &AppError) -> bool {
    matches!(
        error,
        AppError::Planning(_) | AppError::Naming(_) | AppError::Filesystem(_)
    )
}

/// Identifies one movie or TV series through the shared interactive TMDB workflow.
pub fn identify_tmdb_item<U, C>(ui: &mut U, client: &C) -> AppResult<Option<TmdbItem>>
where
    U: InteractiveUi + TmdbInteraction,
    C: TmdbProvider,
{
    let Some(method) = ui.choose_identification_method()? else {
        return Ok(None);
    };

    match method {
        IdentificationMethod::Search => identify_by_search(ui, client),
        IdentificationMethod::ManualId => identify_by_manual_id(ui, client),
    }
}

/// Collects and validates one episode reference for a selected series file.
pub fn collect_series_episode<U, C>(
    ui: &mut U,
    client: &C,
    series_id: TmdbId,
    file_label: &str,
) -> AppResult<Option<EpisodeRef>>
where
    U: InteractiveUi + TmdbInteraction,
    C: TmdbProvider,
{
    loop {
        let Some((season, episode)) = ui.ask_episode_numbers(file_label)? else {
            return Ok(None);
        };
        let episode = match EpisodeRef::parse(&season, &episode) {
            Ok(episode) => episode,
            Err(error) => {
                ui.show_message(MessageLevel::Error, &error.to_string())?;
                continue;
            }
        };

        let activity = ui.start_activity("Validating episode through TMDB...")?;
        match client.get_episode_details(series_id, episode) {
            Ok(_) => {
                activity.finish_success("Episode verified through TMDB.");
                ui.show_verified_episode(&episode)?;
                return Ok(Some(episode));
            }
            Err(error) => {
                activity.finish_error("Episode verification failed.");
                ui.show_message(MessageLevel::Error, &error.to_string())?;
                let Some(should_retry) = ui.confirm("Retry this episode?", true)? else {
                    return Ok(None);
                };
                if !should_retry {
                    return Ok(None);
                }
            }
        }
    }
}

fn identify_by_search<U, C>(ui: &mut U, client: &C) -> AppResult<Option<TmdbItem>>
where
    U: InteractiveUi + TmdbInteraction,
    C: TmdbProvider,
{
    loop {
        let search_result = ui.select_tmdb_result_live(&mut |query| search_tmdb(client, query))?;
        let candidate = match search_result {
            Ok(Some(candidate)) => candidate,
            Ok(None) => return Ok(None),
            Err(error) => {
                let Some(should_retry) = retry_after_tmdb_error(ui, error)? else {
                    return Ok(None);
                };
                if should_retry {
                    continue;
                }
                return Ok(None);
            }
        };

        let activity = ui.start_activity("Fetching confirmed TMDB details...")?;
        let item = match client.get_item(candidate.media_type, candidate.id) {
            Ok(item) => {
                activity.finish_success("TMDB details received.");
                item
            }
            Err(error) => {
                activity.finish_error("TMDB detail lookup failed.");
                let Some(should_retry) = retry_after_tmdb_error(ui, error)? else {
                    return Ok(None);
                };
                if should_retry {
                    continue;
                }
                return Ok(None);
            }
        };

        match ui.confirm_tmdb_item(&item)? {
            Some(true) => return Ok(Some(item)),
            Some(false) => continue,
            None => return Ok(None),
        }
    }
}

fn search_tmdb<C: TmdbProvider>(
    client: &C,
    query: &str,
) -> Result<Vec<TmdbSearchCandidate>, TmdbError> {
    let movie_results = client.search_movies(query)?;
    let series_results = client.search_series(query)?;
    Ok(combine_candidates(movie_results, series_results))
}

fn identify_by_manual_id<U, C>(ui: &mut U, client: &C) -> AppResult<Option<TmdbItem>>
where
    U: InteractiveUi + TmdbInteraction,
    C: TmdbProvider,
{
    loop {
        let Some(media_type) = ui.choose_media_type()? else {
            return Ok(None);
        };

        loop {
            let Some(raw_id) = ui.ask_tmdb_id()? else {
                return Ok(None);
            };
            let id = match crate::domain::parse_tmdb_id(&raw_id) {
                Ok(id) => id,
                Err(error) => {
                    ui.show_message(MessageLevel::Error, &error.to_string())?;
                    continue;
                }
            };

            let activity = ui.start_activity("Fetching confirmed TMDB details...")?;
            let item = match client.get_item(media_type, id) {
                Ok(item) => {
                    activity.finish_success("TMDB details received.");
                    item
                }
                Err(error) => {
                    activity.finish_error("TMDB detail lookup failed.");
                    let Some(should_retry) = retry_after_tmdb_error(ui, error)? else {
                        return Ok(None);
                    };
                    if should_retry {
                        continue;
                    }
                    return Ok(None);
                }
            };

            match ui.confirm_tmdb_item(&item)? {
                Some(true) => return Ok(Some(item)),
                Some(false) => break,
                None => return Ok(None),
            }
        }
    }
}

fn combine_candidates(
    mut movie_results: Vec<TmdbSearchCandidate>,
    mut series_results: Vec<TmdbSearchCandidate>,
) -> Vec<TmdbSearchCandidate> {
    movie_results.append(&mut series_results);
    movie_results
}

fn retry_after_tmdb_error<U: InteractiveUi>(
    ui: &mut U,
    error: TmdbError,
) -> AppResult<Option<bool>> {
    ui.show_message(MessageLevel::Error, &error.to_string())?;
    Ok(ui.confirm("Retry the TMDB request?", true)?)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;
    use crate::{
        domain::{
            DestinationSelection, ExecutionReport, FileOperation, MediaType, OperationPlan,
            TmdbEpisode,
        },
        error::{TmdbError, UiResult},
        ui::{MessageLevel, ProgressOutput},
    };

    #[derive(Debug, Default)]
    struct NoopProgress;

    impl ProgressOutput for NoopProgress {
        fn set_message(&self, _message: &str) {}

        fn set_progress(&self, _completed_bytes: u64, _total_bytes: u64) {}

        fn finish_success(&self, _message: &str) {}

        fn finish_error(&self, _message: &str) {}
    }

    #[derive(Debug, Default)]
    struct RecordingUi {
        events: Vec<String>,
        cancel_on_secret: bool,
        cancel_on_language: bool,
        destination_input: Option<String>,
        file_operations: Vec<Option<FileOperation>>,
        select_many_responses: Vec<Option<Vec<usize>>>,
        confirm_responses: Vec<Option<bool>>,
        identification_methods: Vec<Option<IdentificationMethod>>,
        media_types: Vec<Option<MediaType>>,
        search_queries: Vec<Option<String>>,
        tmdb_result_indices: Vec<Option<usize>>,
        tmdb_ids: Vec<Option<String>>,
        tmdb_confirmations: Vec<Option<bool>>,
        episode_numbers: Vec<Option<(String, String)>>,
    }

    impl InteractiveUi for RecordingUi {
        fn show_welcome(&mut self, version: &str) -> UiResult<()> {
            self.events.push(format!("welcome:{version}"));
            Ok(())
        }

        fn show_step(&mut self, current: usize, total: usize, label: &str) -> UiResult<()> {
            self.events.push(format!("step:{current}/{total}:{label}"));
            Ok(())
        }

        fn choose_file_operation(&mut self) -> UiResult<Option<FileOperation>> {
            Ok(if self.file_operations.is_empty() {
                Some(FileOperation::Move)
            } else {
                self.file_operations.remove(0)
            })
        }

        fn show_file_context(
            &mut self,
            current_file: usize,
            total_files: usize,
            file_path: &std::path::Path,
            source_root: &std::path::Path,
        ) -> UiResult<()> {
            self.events.push(format!(
                "file:{current_file}/{total_files}:{}",
                display_relative_path(file_path, source_root)
            ));
            Ok(())
        }

        fn finish_file_context(&mut self) -> UiResult<()> {
            self.events.push("file-end".to_owned());
            Ok(())
        }

        fn ask_masked_secret(
            &mut self,
            prompt: &str,
            _default: Option<&str>,
        ) -> UiResult<Option<String>> {
            self.events.push(format!("secret:{prompt}"));
            if self.cancel_on_secret {
                Ok(None)
            } else {
                Ok(Some("test-api-key".to_owned()))
            }
        }

        fn ask_text(&mut self, prompt: &str, _default: Option<&str>) -> UiResult<Option<String>> {
            self.events.push(format!("text:{prompt}"));
            if prompt == "Destination folder path" {
                return Ok(self.destination_input.clone());
            }
            if self.cancel_on_language {
                Ok(None)
            } else {
                Ok(Some("pt-BR".to_owned()))
            }
        }

        fn select_one(
            &mut self,
            _prompt: &str,
            _items: &[String],
            _searchable: bool,
        ) -> UiResult<Option<usize>> {
            Ok(None)
        }

        fn select_many(
            &mut self,
            prompt: &str,
            items: &[String],
            _searchable: bool,
        ) -> UiResult<Option<Vec<usize>>> {
            self.events
                .push(format!("many:{prompt}:{}", items.join("|")));
            if self.select_many_responses.is_empty() {
                Ok(None)
            } else {
                Ok(self.select_many_responses.remove(0))
            }
        }

        fn confirm(&mut self, prompt: &str, _default: bool) -> UiResult<Option<bool>> {
            self.events.push(format!("confirm:{prompt}"));
            if self.confirm_responses.is_empty() {
                Ok(None)
            } else {
                Ok(self.confirm_responses.remove(0))
            }
        }

        fn show_message(&mut self, level: MessageLevel, message: &str) -> UiResult<()> {
            self.events.push(format!("message:{level:?}:{message}"));
            Ok(())
        }

        fn start_activity(&mut self, _message: &str) -> UiResult<Box<dyn ProgressOutput>> {
            Ok(Box::new(NoopProgress))
        }

        fn show_plan_preview(&mut self, plan: &OperationPlan) -> UiResult<()> {
            self.events
                .push(format!("preview:{}", plan.operation_count()));
            Ok(())
        }

        fn show_execution_report(&mut self, report: &ExecutionReport) -> UiResult<()> {
            self.events.push(format!(
                "report:{}:{}:{}",
                report.completed_count(),
                report.failed_count(),
                report.pending_count()
            ));
            Ok(())
        }
    }

    impl TmdbInteraction for RecordingUi {
        fn choose_identification_method(&mut self) -> UiResult<Option<IdentificationMethod>> {
            Ok(if self.identification_methods.is_empty() {
                None
            } else {
                self.identification_methods.remove(0)
            })
        }

        fn choose_media_type(&mut self) -> UiResult<Option<MediaType>> {
            Ok(if self.media_types.is_empty() {
                None
            } else {
                self.media_types.remove(0)
            })
        }

        fn select_tmdb_result_live(
            &mut self,
            search: &mut dyn FnMut(
                &str,
            )
                -> Result<Vec<crate::domain::TmdbSearchCandidate>, TmdbError>,
        ) -> UiResult<Result<Option<crate::domain::TmdbSearchCandidate>, TmdbError>> {
            let Some(query) = (if self.search_queries.is_empty() {
                None
            } else {
                self.search_queries.remove(0)
            }) else {
                return Ok(Ok(None));
            };
            let candidates = match search(query.trim()) {
                Ok(candidates) => candidates,
                Err(error) => return Ok(Err(error)),
            };
            let Some(index) = (if self.tmdb_result_indices.is_empty() {
                None
            } else {
                self.tmdb_result_indices.remove(0)
            }) else {
                return Ok(Ok(None));
            };
            Ok(Ok(candidates.get(index).cloned()))
        }

        fn ask_tmdb_id(&mut self) -> UiResult<Option<String>> {
            Ok(if self.tmdb_ids.is_empty() {
                None
            } else {
                self.tmdb_ids.remove(0)
            })
        }

        fn confirm_tmdb_item(&mut self, _item: &TmdbItem) -> UiResult<Option<bool>> {
            Ok(if self.tmdb_confirmations.is_empty() {
                None
            } else {
                self.tmdb_confirmations.remove(0)
            })
        }

        fn ask_episode_numbers(&mut self, _file_label: &str) -> UiResult<Option<(String, String)>> {
            Ok(if self.episode_numbers.is_empty() {
                None
            } else {
                self.episode_numbers.remove(0)
            })
        }

        fn show_verified_episode(&mut self, episode: &EpisodeRef) -> UiResult<()> {
            self.events.push(format!(
                "verified:S{:02}E{:02}",
                episode.season(),
                episode.episode()
            ));
            Ok(())
        }
    }

    #[derive(Debug, Default)]
    struct FakeTmdbProvider {
        items: Vec<TmdbItem>,
        episodes: std::collections::HashSet<(TmdbId, EpisodeRef)>,
    }

    impl TmdbProvider for FakeTmdbProvider {
        fn search_movies(&self, query: &str) -> Result<Vec<TmdbSearchCandidate>, TmdbError> {
            Ok(search_fake_items(self, query, MediaType::Movie))
        }

        fn search_series(&self, query: &str) -> Result<Vec<TmdbSearchCandidate>, TmdbError> {
            Ok(search_fake_items(self, query, MediaType::Series))
        }

        fn get_item(&self, media_type: MediaType, id: TmdbId) -> Result<TmdbItem, TmdbError> {
            self.items
                .iter()
                .find(|item| item.media_type == media_type && item.id == id)
                .cloned()
                .ok_or_else(|| TmdbError::NotFound {
                    resource: format!("{media_type} {id}"),
                })
        }

        fn get_episode_details(
            &self,
            series_id: TmdbId,
            episode: EpisodeRef,
        ) -> Result<TmdbEpisode, TmdbError> {
            if self.episodes.contains(&(series_id, episode)) {
                Ok(TmdbEpisode {
                    series_id,
                    episode,
                    title: None,
                })
            } else {
                Err(TmdbError::EpisodeNotFound {
                    series_id: series_id.value(),
                    season: episode.season(),
                    episode: episode.episode(),
                })
            }
        }
    }

    fn search_fake_items(
        client: &FakeTmdbProvider,
        query: &str,
        media_type: MediaType,
    ) -> Vec<TmdbSearchCandidate> {
        let query = query.to_lowercase();
        client
            .items
            .iter()
            .filter(|item| {
                item.media_type == media_type && item.title.to_lowercase().contains(&query)
            })
            .map(|item| TmdbSearchCandidate {
                id: item.id,
                media_type: item.media_type,
                title: item.title.clone(),
                original_title: item.original_title.clone(),
                year: item.year,
            })
            .collect()
    }

    fn run_default_for_test(ui: &mut RecordingUi, store: &ConfigStore) -> AppResult<RunOutcome> {
        run_configuration_stage_for_test(ui, store, |_| Ok(()))
    }

    fn run_configuration_stage_for_test<F>(
        ui: &mut RecordingUi,
        store: &ConfigStore,
        validate: F,
    ) -> AppResult<RunOutcome>
    where
        F: FnMut(&StartupConfig) -> Result<(), TmdbError>,
    {
        ui.show_welcome("0.1.0")?;
        let Some(_config) =
            configure_and_validate(ui, store, ConfigPromptMode::MissingOnly, validate)?
        else {
            ui.show_message(MessageLevel::Info, "Startup configuration canceled.")?;
            return Ok(RunOutcome::Cancelled);
        };
        ui.show_message(MessageLevel::Success, "TMDB configuration is ready.")?;
        Ok(RunOutcome::StartupConfigured)
    }

    fn run_config_for_test(ui: &mut RecordingUi, store: &ConfigStore) -> AppResult<RunOutcome> {
        run_config_with_store_and_validator(ui, "0.1.0", store, |_| Ok(()))
    }

    fn movie_item(id: u64, title: &str) -> TmdbItem {
        TmdbItem {
            id: TmdbId::new(id).unwrap(),
            media_type: MediaType::Movie,
            title: title.to_owned(),
            original_title: Some(title.to_owned()),
            year: Some(1999),
        }
    }

    fn series_item(id: u64, title: &str) -> TmdbItem {
        TmdbItem {
            id: TmdbId::new(id).unwrap(),
            media_type: MediaType::Series,
            title: title.to_owned(),
            original_title: Some(title.to_owned()),
            year: Some(2011),
        }
    }

    fn filesystem_selection(
        source_root: &std::path::Path,
        destination: &std::path::Path,
        destination_exists: bool,
        sources: Vec<SelectedSource>,
    ) -> FilesystemSelection {
        filesystem_selection_with_operation(
            source_root,
            destination,
            destination_exists,
            FileOperation::Move,
            sources,
        )
    }

    fn filesystem_selection_with_operation(
        source_root: &std::path::Path,
        destination: &std::path::Path,
        destination_exists: bool,
        operation: FileOperation,
        sources: Vec<SelectedSource>,
    ) -> FilesystemSelection {
        FilesystemSelection::new(
            SourceRoot::new(source_root.to_owned()),
            DestinationSelection::new(
                destination.to_owned(),
                destination_exists,
                !destination_exists,
            ),
            operation,
            sources,
        )
    }

    fn manual_identification_ui(
        media_type: MediaType,
        id: u64,
        final_confirmation: Option<bool>,
    ) -> RecordingUi {
        RecordingUi {
            identification_methods: vec![Some(IdentificationMethod::ManualId)],
            media_types: vec![Some(media_type)],
            tmdb_ids: vec![Some(id.to_string())],
            tmdb_confirmations: vec![Some(true)],
            confirm_responses: vec![final_confirmation],
            ..RecordingUi::default()
        }
    }

    #[test]
    fn search_identification_uses_the_live_search_callback_before_fetching_details() {
        let client = FakeTmdbProvider {
            items: vec![movie_item(550, "Fight Club")],
            ..FakeTmdbProvider::default()
        };
        let mut ui = RecordingUi {
            identification_methods: vec![Some(IdentificationMethod::Search)],
            search_queries: vec![Some("fight".to_owned())],
            tmdb_result_indices: vec![Some(0)],
            tmdb_confirmations: vec![Some(true)],
            ..RecordingUi::default()
        };

        let item = identify_tmdb_item(&mut ui, &client).unwrap().unwrap();

        assert_eq!(item.id, TmdbId::new(550).unwrap());
        assert_eq!(item.media_type, MediaType::Movie);
        assert_eq!(item.title, "Fight Club");
    }

    #[test]
    fn multiple_source_folders_get_independent_confirmed_items_in_one_plan() {
        let directory = tempdir().unwrap();
        let first_folder = directory.path().join("first-movie");
        let second_folder = directory.path().join("second-movie");
        let destination = directory.path().join("organized");
        let first_file = first_folder.join("first.mkv");
        let second_file = second_folder.join("second.mp4");
        fs::create_dir(&first_folder).unwrap();
        fs::create_dir(&second_folder).unwrap();
        fs::create_dir(&destination).unwrap();
        fs::write(&first_file, "first movie").unwrap();
        fs::write(&second_file, "second movie").unwrap();

        let client = FakeTmdbProvider {
            items: vec![
                movie_item(550, "Fight Club"),
                movie_item(680, "Pulp Fiction"),
            ],
            ..FakeTmdbProvider::default()
        };
        let mut ui = RecordingUi {
            identification_methods: vec![
                Some(IdentificationMethod::ManualId),
                Some(IdentificationMethod::ManualId),
            ],
            media_types: vec![Some(MediaType::Movie), Some(MediaType::Movie)],
            tmdb_ids: vec![Some("550".to_owned()), Some("680".to_owned())],
            tmdb_confirmations: vec![Some(true), Some(true)],
            confirm_responses: vec![Some(true)],
            ..RecordingUi::default()
        };
        let selection = filesystem_selection(
            directory.path(),
            &destination,
            true,
            vec![
                SelectedSource::new(first_folder, vec![first_file.clone()]),
                SelectedSource::new(second_folder, vec![second_file.clone()]),
            ],
        );

        let outcome = organize_selection(&mut ui, &selection, &client).unwrap();

        assert_eq!(outcome, RunOutcome::Completed);
        assert!(!first_file.exists());
        assert!(!second_file.exists());
        assert_eq!(
            fs::read_to_string(destination.join("550__S__MOVIE__S__Fight Club.mkv")).unwrap(),
            "first movie"
        );
        assert_eq!(
            fs::read_to_string(destination.join("680__S__MOVIE__S__Pulp Fiction.mp4")).unwrap(),
            "second movie"
        );
        assert!(ui.events.iter().any(|event| event == "preview:2"));
        assert!(ui.events.iter().any(|event| event == "report:2:0:0"));
    }

    #[test]
    fn a_confirmed_movie_is_planned_previewed_and_moved_with_its_original_extension() {
        let directory = tempdir().unwrap();
        let source_folder = directory.path().join("movies");
        let destination = directory.path().join("organized");
        let source_file = source_folder.join("movie.MP4");
        fs::create_dir(&source_folder).unwrap();
        fs::write(&source_file, "movie contents").unwrap();

        let item = movie_item(550, "Mission: Impossible");
        let client = FakeTmdbProvider {
            items: vec![item],
            ..FakeTmdbProvider::default()
        };
        let mut ui = manual_identification_ui(MediaType::Movie, 550, Some(true));
        let selection = filesystem_selection(
            directory.path(),
            &destination,
            false,
            vec![SelectedSource::new(
                source_folder.clone(),
                vec![source_file.clone()],
            )],
        );

        let outcome = organize_selection(&mut ui, &selection, &client).unwrap();

        assert_eq!(outcome, RunOutcome::Completed);
        assert!(!source_file.exists());
        assert_eq!(
            fs::read_to_string(destination.join("550__S__MOVIE__S__Mission - Impossible.mp4"),)
                .unwrap(),
            "movie contents"
        );
        assert!(ui.events.iter().any(|event| event == "preview:1"));
        assert!(ui.events.iter().any(|event| event == "report:1:0:0"));
    }

    #[test]
    fn a_confirmed_movie_can_be_copied_without_removing_the_source() {
        let directory = tempdir().unwrap();
        let source_folder = directory.path().join("movies");
        let destination = directory.path().join("organized");
        let source_file = source_folder.join("movie.MP4");
        fs::create_dir(&source_folder).unwrap();
        fs::write(&source_file, "movie contents").unwrap();

        let client = FakeTmdbProvider {
            items: vec![movie_item(550, "Mission: Impossible")],
            ..FakeTmdbProvider::default()
        };
        let mut ui = manual_identification_ui(MediaType::Movie, 550, Some(true));
        let selection = filesystem_selection_with_operation(
            directory.path(),
            &destination,
            false,
            FileOperation::Copy,
            vec![SelectedSource::new(
                source_folder.clone(),
                vec![source_file.clone()],
            )],
        );

        let outcome = organize_selection(&mut ui, &selection, &client).unwrap();

        assert_eq!(outcome, RunOutcome::Completed);
        assert!(source_file.exists());
        assert_eq!(
            fs::read_to_string(destination.join("550__S__MOVIE__S__Mission - Impossible.mp4"),)
                .unwrap(),
            "movie contents"
        );
        assert!(
            ui.events
                .iter()
                .any(|event| event == "confirm:Copy and rename 1 files?")
        );
        assert!(ui.events.iter().any(|event| event == "report:1:0:0"));
    }

    #[test]
    fn a_series_builds_one_operation_per_file_and_validates_each_episode() {
        let directory = tempdir().unwrap();
        let source_folder = directory.path().join("series");
        let destination = directory.path().join("organized");
        let first_file = source_folder.join("episode-01.MKV");
        let second_file = source_folder.join("season-01").join("episode-02.mp4");
        fs::create_dir(&source_folder).unwrap();
        fs::create_dir(second_file.parent().unwrap()).unwrap();
        fs::write(&first_file, "episode one").unwrap();
        fs::write(&second_file, "episode two").unwrap();
        fs::create_dir(&destination).unwrap();

        let id = TmdbId::new(1399).unwrap();
        let client = FakeTmdbProvider {
            items: vec![series_item(1399, "Game: Of Thrones")],
            episodes: [(id, EpisodeRef::new(1, 1)), (id, EpisodeRef::new(1, 2))]
                .into_iter()
                .collect(),
        };
        let mut ui = manual_identification_ui(MediaType::Series, 1399, Some(true));
        ui.identification_methods = vec![
            Some(IdentificationMethod::ManualId),
            Some(IdentificationMethod::ManualId),
        ];
        ui.media_types = vec![Some(MediaType::Series), Some(MediaType::Series)];
        ui.tmdb_ids = vec![Some("1399".to_owned()), Some("1399".to_owned())];
        ui.tmdb_confirmations = vec![Some(true), Some(true)];
        ui.episode_numbers = vec![
            Some(("1".to_owned(), "1".to_owned())),
            Some(("1".to_owned(), "2".to_owned())),
        ];
        let selection = filesystem_selection(
            directory.path(),
            &destination,
            true,
            vec![SelectedSource::new(
                source_folder,
                vec![first_file.clone(), second_file.clone()],
            )],
        );

        let outcome = organize_selection(&mut ui, &selection, &client).unwrap();

        assert_eq!(outcome, RunOutcome::Completed);
        assert!(!first_file.exists());
        assert!(!second_file.exists());
        assert_eq!(
            fs::read_to_string(
                destination.join("1399__S__SERIES__S__S01E01__S__Game - Of Thrones.mkv",),
            )
            .unwrap(),
            "episode one"
        );
        assert_eq!(
            fs::read_to_string(
                destination.join("1399__S__SERIES__S__S01E02__S__Game - Of Thrones.mp4",),
            )
            .unwrap(),
            "episode two"
        );
        assert!(ui.events.iter().any(|event| event == "preview:2"));
        assert!(ui.events.iter().any(|event| event == "report:2:0:0"));
        assert!(ui.events.iter().any(|event| event == "verified:S01E01"));
        assert!(ui.events.iter().any(|event| event == "verified:S01E02"));
    }

    #[test]
    fn each_selected_video_runs_its_own_movie_identification_loop() {
        let directory = tempdir().unwrap();
        let source_folder = directory.path().join("movies");
        let destination = directory.path().join("organized");
        let first_file = source_folder.join("first.mkv");
        let second_file = source_folder.join("second.mkv");
        fs::create_dir(&source_folder).unwrap();
        fs::write(&first_file, "first").unwrap();
        fs::write(&second_file, "second").unwrap();

        let client = FakeTmdbProvider {
            items: vec![
                movie_item(550, "Fight Club"),
                movie_item(680, "Pulp Fiction"),
            ],
            ..FakeTmdbProvider::default()
        };
        let mut ui = RecordingUi {
            identification_methods: vec![
                Some(IdentificationMethod::ManualId),
                Some(IdentificationMethod::ManualId),
            ],
            media_types: vec![Some(MediaType::Movie), Some(MediaType::Movie)],
            tmdb_ids: vec![Some("550".to_owned()), Some("680".to_owned())],
            tmdb_confirmations: vec![Some(true), Some(true)],
            confirm_responses: vec![Some(true)],
            ..RecordingUi::default()
        };
        let selection = filesystem_selection(
            directory.path(),
            &destination,
            false,
            vec![SelectedSource::new(
                source_folder,
                vec![first_file.clone(), second_file.clone()],
            )],
        );

        let outcome = organize_selection(&mut ui, &selection, &client).unwrap();

        assert_eq!(outcome, RunOutcome::Completed);
        assert!(!first_file.exists());
        assert!(!second_file.exists());
        assert_eq!(
            fs::read_to_string(destination.join("550__S__MOVIE__S__Fight Club.mkv")).unwrap(),
            "first"
        );
        assert_eq!(
            fs::read_to_string(destination.join("680__S__MOVIE__S__Pulp Fiction.mkv")).unwrap(),
            "second"
        );
        assert!(ui.events.iter().any(|event| event == "preview:2"));
        assert!(ui.events.iter().any(|event| event == "report:2:0:0"));
        let first_context = ui
            .events
            .iter()
            .position(|event| event == "file:1/2:movies/first.mkv")
            .unwrap();
        let first_context_end = ui
            .events
            .iter()
            .position(|event| event == "file-end")
            .unwrap();
        let second_context = ui
            .events
            .iter()
            .position(|event| event == "file:2/2:movies/second.mkv")
            .unwrap();
        assert!(first_context < first_context_end);
        assert!(first_context_end < second_context);
    }

    #[test]
    fn duplicate_series_episode_values_are_rejected_and_can_be_corrected() {
        let directory = tempdir().unwrap();
        let source_folder = directory.path().join("series");
        let destination = directory.path().join("organized");
        let first_file = source_folder.join("first.mkv");
        let second_file = source_folder.join("second.mkv");
        fs::create_dir(&source_folder).unwrap();
        fs::create_dir(&destination).unwrap();
        fs::write(&first_file, "first").unwrap();
        fs::write(&second_file, "second").unwrap();

        let id = TmdbId::new(1399).unwrap();
        let client = FakeTmdbProvider {
            items: vec![series_item(1399, "Game of Thrones")],
            episodes: [(id, EpisodeRef::new(1, 1)), (id, EpisodeRef::new(1, 2))]
                .into_iter()
                .collect(),
        };
        let mut ui = manual_identification_ui(MediaType::Series, 1399, Some(true));
        ui.identification_methods = vec![
            Some(IdentificationMethod::ManualId),
            Some(IdentificationMethod::ManualId),
        ];
        ui.media_types = vec![Some(MediaType::Series), Some(MediaType::Series)];
        ui.tmdb_ids = vec![Some("1399".to_owned()), Some("1399".to_owned())];
        ui.tmdb_confirmations = vec![Some(true), Some(true)];
        ui.episode_numbers = vec![
            Some(("1".to_owned(), "1".to_owned())),
            Some(("1".to_owned(), "1".to_owned())),
            Some(("1".to_owned(), "2".to_owned())),
        ];
        let selection = filesystem_selection(
            directory.path(),
            &destination,
            true,
            vec![SelectedSource::new(
                source_folder,
                vec![first_file.clone(), second_file.clone()],
            )],
        );

        let outcome = organize_selection(&mut ui, &selection, &client).unwrap();

        assert_eq!(outcome, RunOutcome::Completed);
        assert!(
            ui.events
                .iter()
                .any(|event| event.contains("was already assigned"))
        );
        assert_eq!(
            ui.events
                .iter()
                .filter(|event| event.as_str() == "verified:S01E01")
                .count(),
            2
        );
        assert!(ui.events.iter().any(|event| event == "verified:S01E02"));
    }

    #[test]
    fn declining_the_final_confirmation_does_not_create_or_change_anything() {
        let directory = tempdir().unwrap();
        let source_folder = directory.path().join("movies");
        let destination = directory.path().join("new-organized");
        let source_file = source_folder.join("movie.mkv");
        fs::create_dir(&source_folder).unwrap();
        fs::write(&source_file, "movie").unwrap();

        let client = FakeTmdbProvider {
            items: vec![movie_item(550, "Fight Club")],
            ..FakeTmdbProvider::default()
        };
        let mut ui = manual_identification_ui(MediaType::Movie, 550, Some(false));
        let selection = filesystem_selection(
            directory.path(),
            &destination,
            false,
            vec![SelectedSource::new(
                source_folder,
                vec![source_file.clone()],
            )],
        );

        let outcome = organize_selection(&mut ui, &selection, &client).unwrap();

        assert_eq!(outcome, RunOutcome::Cancelled);
        assert!(source_file.exists());
        assert!(!destination.exists());
        assert!(!ui.events.iter().any(|event| event.starts_with("report:")));
    }

    #[test]
    fn an_existing_destination_conflict_blocks_confirmation_and_preserves_the_source() {
        let directory = tempdir().unwrap();
        let source_folder = directory.path().join("movies");
        let destination = directory.path().join("organized");
        let source_file = source_folder.join("movie.mkv");
        let conflicting_file = destination.join("550__S__MOVIE__S__Fight Club.mkv");
        fs::create_dir(&source_folder).unwrap();
        fs::create_dir(&destination).unwrap();
        fs::write(&source_file, "source").unwrap();
        fs::write(&conflicting_file, "existing").unwrap();

        let client = FakeTmdbProvider {
            items: vec![movie_item(550, "Fight Club")],
            ..FakeTmdbProvider::default()
        };
        let mut ui = manual_identification_ui(MediaType::Movie, 550, Some(true));
        let selection = filesystem_selection(
            directory.path(),
            &destination,
            true,
            vec![SelectedSource::new(
                source_folder,
                vec![source_file.clone()],
            )],
        );

        let error = organize_selection(&mut ui, &selection, &client).unwrap_err();

        assert!(matches!(
            error,
            AppError::Filesystem(crate::error::FilesystemError::DestinationAlreadyExists { .. })
        ));
        assert!(source_file.exists());
        assert_eq!(fs::read_to_string(conflicting_file).unwrap(), "existing");
        assert!(!ui.events.iter().any(|event| event.starts_with("preview:")));
        assert!(
            !ui.events
                .iter()
                .any(|event| event.starts_with("confirm:Move"))
        );
    }

    #[test]
    fn startup_questions_are_asked_in_the_required_order() {
        let directory = tempdir().unwrap();
        let store = ConfigStore::from_path(directory.path().join("config.json"));
        let mut ui = RecordingUi::default();

        let outcome = run_default_for_test(&mut ui, &store).unwrap();

        assert_eq!(outcome, RunOutcome::StartupConfigured);
        assert!(
            ui.events
                .iter()
                .position(|event| event == "step:1/2:TMDB API key")
                .is_some()
        );
        assert!(
            ui.events
                .iter()
                .position(|event| event == "secret:TMDB API key")
                .is_some()
        );
        assert!(
            ui.events
                .iter()
                .position(|event| event == "step:2/2:TMDB metadata language")
                .is_some()
        );
        assert!(
            ui.events
                .iter()
                .position(|event| event == "text:TMDB metadata language")
                .is_some()
        );
    }

    #[test]
    fn canceling_the_api_key_prompt_performs_no_later_work() {
        let directory = tempdir().unwrap();
        let store = ConfigStore::from_path(directory.path().join("config.json"));
        let mut ui = RecordingUi {
            cancel_on_secret: true,
            ..RecordingUi::default()
        };

        let outcome = run_default_for_test(&mut ui, &store).unwrap();

        assert_eq!(outcome, RunOutcome::Cancelled);
        assert!(!ui.events.iter().any(|event| event.starts_with("text:")));
        assert!(
            !ui.events
                .iter()
                .any(|event| event.starts_with("message:Success"))
        );
    }

    #[test]
    fn canceling_the_language_prompt_does_not_capture_configuration() {
        let directory = tempdir().unwrap();
        let store = ConfigStore::from_path(directory.path().join("config.json"));
        let mut ui = RecordingUi {
            cancel_on_language: true,
            ..RecordingUi::default()
        };

        let outcome = run_default_for_test(&mut ui, &store).unwrap();

        assert_eq!(outcome, RunOutcome::Cancelled);
        assert!(
            ui.events
                .iter()
                .any(|event| event == "message:Info:Startup configuration canceled.")
        );
        assert!(
            !ui.events
                .iter()
                .any(|event| { event == "message:Success:TMDB configuration is ready." })
        );
    }

    #[test]
    fn a_complete_saved_configuration_skips_startup_prompts() {
        let directory = tempdir().unwrap();
        let store = ConfigStore::from_path(directory.path().join("config.json"));
        let mut first_ui = RecordingUi::default();
        run_default_for_test(&mut first_ui, &store).unwrap();

        let mut second_ui = RecordingUi::default();
        let outcome = run_default_for_test(&mut second_ui, &store).unwrap();

        assert_eq!(outcome, RunOutcome::StartupConfigured);
        assert!(
            !second_ui
                .events
                .iter()
                .any(|event| event.starts_with("step:"))
        );
        assert!(
            !second_ui
                .events
                .iter()
                .any(|event| event.starts_with("secret:") || event.starts_with("text:"))
        );
    }

    #[test]
    fn startup_validates_the_complete_configuration_before_reporting_success() {
        let directory = tempdir().unwrap();
        let store = ConfigStore::from_path(directory.path().join("config.json"));
        let mut ui = RecordingUi::default();
        let mut validation_calls = 0;

        let outcome = run_configuration_stage_for_test(&mut ui, &store, |config| {
            validation_calls += 1;
            assert_eq!(config.tmdb_language(), "pt-BR");
            assert_eq!(config.tmdb_api_key(), "test-api-key");
            Ok(())
        })
        .unwrap();

        assert_eq!(outcome, RunOutcome::StartupConfigured);
        assert_eq!(validation_calls, 1);
        assert!(
            ui.events
                .iter()
                .any(|event| event == "message:Success:TMDB configuration is ready.")
        );
    }

    #[test]
    fn a_partial_saved_configuration_prompts_only_for_the_missing_language() {
        let directory = tempdir().unwrap();
        let store = ConfigStore::from_path(directory.path().join("config.json"));
        fs::write(store.path(), r#"{"tmdb_api_key":"stored-api-key"}"#).unwrap();
        let mut ui = RecordingUi::default();

        let outcome = run_default_for_test(&mut ui, &store).unwrap();

        assert_eq!(outcome, RunOutcome::StartupConfigured);
        assert!(!ui.events.iter().any(|event| event.starts_with("secret:")));
        assert!(
            ui.events
                .iter()
                .any(|event| event == "step:1/1:TMDB metadata language")
        );
        assert!(
            ui.events
                .iter()
                .any(|event| event == "text:TMDB metadata language")
        );
    }

    #[test]
    fn config_command_reuses_the_same_prompts_but_reopens_both_fields() {
        let directory = tempdir().unwrap();
        let store = ConfigStore::from_path(directory.path().join("config.json"));
        let mut first_ui = RecordingUi::default();
        run_default_for_test(&mut first_ui, &store).unwrap();

        let mut ui = RecordingUi::default();
        let outcome = run_config_for_test(&mut ui, &store).unwrap();

        assert_eq!(outcome, RunOutcome::ConfigurationUpdated);
        assert!(ui.events.iter().any(|event| event == "secret:TMDB API key"));
        assert!(
            ui.events
                .iter()
                .any(|event| event == "text:TMDB metadata language")
        );
    }

    #[test]
    fn canceling_config_update_preserves_the_existing_file() {
        let directory = tempdir().unwrap();
        let store = ConfigStore::from_path(directory.path().join("config.json"));
        let mut first_ui = RecordingUi::default();
        run_default_for_test(&mut first_ui, &store).unwrap();
        let original = fs::read_to_string(store.path()).unwrap();

        let mut ui = RecordingUi {
            cancel_on_secret: true,
            ..RecordingUi::default()
        };
        let outcome = run_config_for_test(&mut ui, &store).unwrap();

        assert_eq!(outcome, RunOutcome::Cancelled);
        assert_eq!(fs::read_to_string(store.path()).unwrap(), original);
    }

    #[test]
    fn filesystem_selection_uses_one_explorer_and_preserves_deterministic_file_order() {
        let directory = tempdir().unwrap();
        let alpha = directory.path().join("alpha");
        let beta = directory.path().join("Beta");
        let destination = directory.path().join("organized");
        let root_video = directory.path().join("root-video.mp4");
        fs::create_dir(&alpha).unwrap();
        fs::create_dir(&beta).unwrap();
        fs::create_dir(&destination).unwrap();
        let alpha_first = alpha.join("first.mkv");
        let alpha_second = alpha.join("second.MKV");
        let alpha_nested = alpha.join("season-01").join("episode.mp4");
        let beta_episode = beta.join("episode.mkv");
        fs::write(&alpha_first, "first").unwrap();
        fs::write(&alpha_second, "second").unwrap();
        fs::create_dir(alpha_nested.parent().unwrap()).unwrap();
        fs::write(&alpha_nested, "nested").unwrap();
        fs::write(&beta_episode, "episode").unwrap();
        fs::write(&root_video, "root").unwrap();

        let mut ui = RecordingUi {
            destination_input: Some("organized".to_owned()),
            file_operations: vec![Some(FileOperation::Copy)],
            select_many_responses: vec![Some(vec![4, 3, 2, 1, 0])],
            ..RecordingUi::default()
        };
        let selection = collect_filesystem_selection_from_root(
            &mut ui,
            SourceRoot::new(directory.path().to_path_buf()),
        )
        .unwrap()
        .unwrap();

        assert_eq!(selection.source_root().path(), directory.path());
        assert_eq!(selection.destination().path(), destination);
        assert_eq!(selection.operation(), FileOperation::Copy);
        assert_eq!(selection.sources().len(), 5);
        let selected_files = selection
            .sources()
            .iter()
            .flat_map(|source| source.files().iter())
            .collect::<Vec<_>>();
        assert_eq!(
            selected_files,
            vec![
                &alpha_first,
                &alpha_nested,
                &alpha_second,
                &beta_episode,
                &root_video
            ]
        );
        assert_eq!(selection.sources()[0].folder(), alpha);
        assert_eq!(selection.sources()[3].folder(), beta);
        assert_eq!(selection.sources()[4].folder(), directory.path());
        assert!(destination.is_dir());
        assert!(root_video.is_file());
        assert!(alpha_first.is_file());
        assert!(alpha_nested.is_file());
        assert!(alpha_second.is_file());
        assert!(beta_episode.is_file());
        assert!(ui.events.iter().any(|event| {
            event
                == "many:Select video files:alpha/first.mkv · 5 B|alpha/season-01/episode.mp4 · 6 B|alpha/second.MKV · 6 B|Beta/episode.mkv · 7 B|root-video.mp4 · 4 B"
        }));
        let absolute_root = directory.path().to_string_lossy();
        assert!(
            !ui.events
                .iter()
                .any(|event| event.contains(&*absolute_root))
        );
    }

    #[test]
    fn canceling_filesystem_selection_does_not_create_a_deferred_destination() {
        let directory = tempdir().unwrap();
        let source = directory.path().join("source");
        let destination = directory.path().join("new-destination");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("movie.mkv"), "movie").unwrap();

        let mut ui = RecordingUi {
            destination_input: Some("new-destination".to_owned()),
            confirm_responses: vec![Some(true)],
            ..RecordingUi::default()
        };
        let result = collect_filesystem_selection_from_root(
            &mut ui,
            SourceRoot::new(directory.path().to_path_buf()),
        )
        .unwrap();

        assert!(result.is_none());
        assert!(!destination.exists());
        assert!(source.join("movie.mkv").is_file());
    }

    #[test]
    fn no_video_files_can_be_declined_without_mutation() {
        let directory = tempdir().unwrap();
        let source = directory.path().join("empty-source");
        let destination = directory.path().join("organized");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&destination).unwrap();

        let mut ui = RecordingUi {
            destination_input: Some("organized".to_owned()),
            confirm_responses: vec![Some(false)],
            ..RecordingUi::default()
        };
        let result = collect_filesystem_selection_from_root(
            &mut ui,
            SourceRoot::new(directory.path().to_path_buf()),
        )
        .unwrap();

        assert!(result.is_none());
        assert!(source.is_dir());
        assert!(destination.is_dir());
    }
}
