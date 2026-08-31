use crate::{
    config::{ConfigPromptMode, ConfigStore, StartupConfig, configure_interactively},
    domain::{
        EpisodeRef, FilesystemSelection, IdentificationMethod, RunOutcome, SelectedSource,
        SourceFolder, SourceRoot, TmdbId, TmdbItem, TmdbSearchCandidate,
    },
    error::{AppError, AppResult, TmdbError, UiError},
    filesystem::{self, DiscoveryWarning},
    tmdb::client::TmdbClient,
    ui::{InteractiveUi, MessageLevel, TmdbInteraction, display_relative_path},
};

/// Runs the default interactive workflow using the current user's configuration file.
pub fn run<U: InteractiveUi>(ui: &mut U, version: &str) -> AppResult<RunOutcome> {
    let store = ConfigStore::for_current_user()?;
    run_with_store(ui, version, &store)
}

/// Runs the default workflow with an explicit configuration store.
///
/// The explicit-store boundary keeps orchestration tests isolated from the real home directory
/// while the production entry point continues to use the documented per-user location.
pub fn run_with_store<U: InteractiveUi>(
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
    U: InteractiveUi,
    F: FnMut(&StartupConfig) -> Result<(), TmdbError>,
{
    ui.show_welcome(version)?;

    let Some(_startup_config) =
        configure_and_validate(ui, store, ConfigPromptMode::MissingOnly, validate)?
    else {
        ui.show_message(MessageLevel::Info, "Startup configuration canceled.")?;
        return Ok(RunOutcome::Cancelled);
    };

    ui.show_message(MessageLevel::Success, "TMDB configuration is ready.")?;
    let Some(_filesystem_selection) = collect_filesystem_selection(ui)? else {
        ui.show_message(MessageLevel::Info, "Filesystem selection canceled.")?;
        return Ok(RunOutcome::Cancelled);
    };
    ui.show_message(MessageLevel::Success, "Filesystem selection is ready.")?;
    ui.show_message(
        MessageLevel::Info,
        "TMDB identification and media organization will continue in the next workflow step.",
    )?;

    Ok(RunOutcome::MediaSelectionReady)
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

    'destination: loop {
        ui.show_step(1, 3, "Choose the destination folder")?;
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

        let folders = filesystem::discover_source_folders(&source_root, &destination)?;
        show_discovery_warnings(ui, folders.warnings(), source_root.path())?;
        if folders.items().is_empty() {
            ui.show_message(
                MessageLevel::Warning,
                "No eligible source folders were found in the current directory.",
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
            ui.show_step(2, 3, "Select source folders")?;
            let Some(folder_indices) = ui.select_source_folders(&source_root, folders.items())?
            else {
                return Ok(None);
            };
            if folder_indices.is_empty() {
                ui.show_message(
                    MessageLevel::Error,
                    "Select at least one source folder to continue.",
                )?;
                continue;
            }
            validate_selection_indices(&folder_indices, folders.items().len(), "source folder")?;

            let mut selected_sources = Vec::with_capacity(folder_indices.len());
            let mut choose_folders_again = false;
            let mut sorted_folder_indices = folder_indices;
            sorted_folder_indices.sort_unstable();

            for folder_index in sorted_folder_indices {
                let folder = &folders.items()[folder_index];
                let videos = filesystem::discover_video_files(folder)?;
                show_discovery_warnings(ui, videos.warnings(), folder.path())?;
                if videos.items().is_empty() {
                    ui.show_message(
                        MessageLevel::Warning,
                        &format!(
                            "No eligible video files were found in {} or its subfolders.",
                            display_relative_path(folder.path(), source_root.path())
                        ),
                    )?;
                    let Some(try_other_folder) =
                        ui.confirm("Choose source folders again?", true)?
                    else {
                        return Ok(None);
                    };
                    if try_other_folder {
                        choose_folders_again = true;
                        break;
                    }
                    return Ok(None);
                }

                loop {
                    ui.show_step(
                        3,
                        3,
                        &format!(
                            "Select videos in {}",
                            display_relative_path(folder.path(), source_root.path())
                        ),
                    )?;
                    let Some(file_indices) =
                        ui.select_video_files(&source_root, folder, videos.items())?
                    else {
                        return Ok(None);
                    };
                    if file_indices.is_empty() {
                        ui.show_message(
                            MessageLevel::Error,
                            &format!(
                                "Select at least one video file in {} to continue.",
                                display_relative_path(folder.path(), source_root.path())
                            ),
                        )?;
                        continue;
                    }
                    validate_selection_indices(&file_indices, videos.items().len(), "video file")?;

                    let mut sorted_file_indices = file_indices;
                    sorted_file_indices.sort_unstable();
                    let files = sorted_file_indices
                        .into_iter()
                        .map(|file_index| videos.items()[file_index].path().to_owned())
                        .collect();
                    selected_sources.push(SelectedSource::new(folder.path().to_owned(), files));
                    break;
                }
            }

            if choose_folders_again {
                continue;
            }

            let selected_folders = selected_sources
                .iter()
                .map(|source| SourceFolder::new(source.folder().to_owned()))
                .collect::<Vec<_>>();
            if let Err(error) =
                filesystem::validate_destination_against_sources(&destination, &selected_folders)
            {
                ui.show_message(MessageLevel::Error, &error.to_string())?;
                let Some(try_again) = ui.confirm("Choose source folders again?", true)? else {
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
                selected_sources,
            )));
        }
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

/// Identifies one movie or TV series through the shared interactive TMDB workflow.
pub fn identify_tmdb_item<U: InteractiveUi + TmdbInteraction>(
    ui: &mut U,
    client: &TmdbClient,
) -> AppResult<Option<TmdbItem>> {
    let Some(method) = ui.choose_identification_method()? else {
        return Ok(None);
    };

    match method {
        IdentificationMethod::Search => identify_by_search(ui, client),
        IdentificationMethod::ManualId => identify_by_manual_id(ui, client),
    }
}

/// Collects and validates one episode reference for a selected series file.
pub fn collect_series_episode<U: InteractiveUi + TmdbInteraction>(
    ui: &mut U,
    client: &TmdbClient,
    series_id: TmdbId,
    file_label: &str,
) -> AppResult<Option<EpisodeRef>> {
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

fn identify_by_search<U: InteractiveUi + TmdbInteraction>(
    ui: &mut U,
    client: &TmdbClient,
) -> AppResult<Option<TmdbItem>> {
    loop {
        let Some(query) = ui.ask_search_query()? else {
            return Ok(None);
        };
        let query = query.trim().to_owned();
        if query.is_empty() {
            ui.show_message(
                MessageLevel::Error,
                "The TMDB search query cannot be empty.",
            )?;
            continue;
        }

        let activity = ui.start_activity("Searching TMDB movies and TV series...")?;
        let movie_results = match client.search_movies(&query) {
            Ok(results) => results,
            Err(error) => {
                activity.finish_error("TMDB search failed.");
                let Some(should_retry) = retry_after_tmdb_error(ui, error)? else {
                    return Ok(None);
                };
                if should_retry {
                    continue;
                }
                return Ok(None);
            }
        };
        let series_results = match client.search_series(&query) {
            Ok(results) => results,
            Err(error) => {
                activity.finish_error("TMDB search failed.");
                let Some(should_retry) = retry_after_tmdb_error(ui, error)? else {
                    return Ok(None);
                };
                if should_retry {
                    continue;
                }
                return Ok(None);
            }
        };
        activity.finish_success("TMDB search completed.");

        let candidates = combine_candidates(movie_results, series_results);
        if candidates.is_empty() {
            ui.show_message(
                MessageLevel::Warning,
                "TMDB returned no movies or TV series for that search.",
            )?;
            continue;
        }

        let Some(index) = ui.select_tmdb_result(&candidates)? else {
            return Ok(None);
        };
        let Some(candidate) = candidates.get(index) else {
            return Err(AppError::Ui(UiError::InvalidSelection {
                context: "TMDB result",
            }));
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

fn identify_by_manual_id<U: InteractiveUi + TmdbInteraction>(
    ui: &mut U,
    client: &TmdbClient,
) -> AppResult<Option<TmdbItem>> {
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
        error::UiResult,
        ui::{MessageLevel, ProgressOutput},
    };

    #[derive(Debug, Default)]
    struct NoopProgress;

    impl ProgressOutput for NoopProgress {
        fn set_message(&self, _message: &str) {}

        fn finish_success(&self, _message: &str) {}

        fn finish_error(&self, _message: &str) {}
    }

    #[derive(Debug, Default)]
    struct RecordingUi {
        events: Vec<String>,
        cancel_on_secret: bool,
        cancel_on_language: bool,
        destination_input: Option<String>,
        select_many_responses: Vec<Option<Vec<usize>>>,
        confirm_responses: Vec<Option<bool>>,
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
    fn filesystem_selection_preserves_folder_associations_and_deterministic_file_order() {
        let directory = tempdir().unwrap();
        let alpha = directory.path().join("alpha");
        let beta = directory.path().join("Beta");
        let destination = directory.path().join("organized");
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

        let mut ui = RecordingUi {
            destination_input: Some("organized".to_owned()),
            select_many_responses: vec![Some(vec![1, 0]), Some(vec![2, 1, 0]), Some(vec![0])],
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
        assert_eq!(selection.sources().len(), 2);
        assert_eq!(selection.sources()[0].folder(), alpha);
        assert_eq!(
            selection.sources()[0].files(),
            &[
                alpha_first.clone(),
                alpha_nested.clone(),
                alpha_second.clone(),
            ]
        );
        assert_eq!(selection.sources()[1].folder(), beta);
        assert_eq!(
            selection.sources()[1].files(),
            std::slice::from_ref(&beta_episode)
        );
        assert!(destination.is_dir());
        assert!(alpha_first.is_file());
        assert!(alpha_nested.is_file());
        assert!(alpha_second.is_file());
        assert!(beta_episode.is_file());
        assert!(
            ui.events
                .iter()
                .any(|event| event == "many:Select source folders:alpha|Beta")
        );
        assert!(ui.events.iter().any(|event| event
            == "many:Select video files in alpha:first.mkv · 5 B|season-01/episode.mp4 · 6 B|second.MKV · 6 B"));
        assert!(
            ui.events
                .iter()
                .any(|event| event == "many:Select video files in Beta:episode.mkv · 7 B")
        );
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
    fn a_source_folder_without_videos_can_be_declined_without_mutation() {
        let directory = tempdir().unwrap();
        let source = directory.path().join("empty-source");
        let destination = directory.path().join("organized");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&destination).unwrap();

        let mut ui = RecordingUi {
            destination_input: Some("organized".to_owned()),
            select_many_responses: vec![Some(vec![0])],
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
