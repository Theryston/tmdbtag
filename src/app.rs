use crate::{
    config::{ConfigPromptMode, ConfigStore, configure_interactively},
    domain::RunOutcome,
    error::AppResult,
    ui::{InteractiveUi, MessageLevel},
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
    ui.show_welcome(version)?;

    let Some(_startup_config) = configure_interactively(ui, store, ConfigPromptMode::MissingOnly)?
    else {
        ui.show_message(MessageLevel::Info, "Startup configuration canceled.")?;
        return Ok(RunOutcome::Cancelled);
    };

    ui.show_message(MessageLevel::Success, "TMDB configuration is ready.")?;
    ui.show_message(
        MessageLevel::Info,
        "TMDB verification and media organization will be connected in the next tasks.",
    )?;

    Ok(RunOutcome::StartupConfigured)
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
    ui.show_welcome(version)?;

    let Some(_startup_config) = configure_interactively(ui, store, ConfigPromptMode::ReplaceAll)?
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
            _prompt: &str,
            _items: &[String],
            _searchable: bool,
        ) -> UiResult<Option<Vec<usize>>> {
            Ok(None)
        }

        fn confirm(&mut self, _prompt: &str, _default: bool) -> UiResult<Option<bool>> {
            Ok(None)
        }

        fn show_message(&mut self, level: MessageLevel, message: &str) -> UiResult<()> {
            self.events.push(format!("message:{level:?}:{message}"));
            Ok(())
        }

        fn start_activity(&mut self, _message: &str) -> UiResult<Box<dyn ProgressOutput>> {
            Ok(Box::new(NoopProgress))
        }
    }

    #[test]
    fn startup_questions_are_asked_in_the_required_order() {
        let directory = tempdir().unwrap();
        let store = ConfigStore::from_path(directory.path().join("config.json"));
        let mut ui = RecordingUi::default();

        let outcome = run_with_store(&mut ui, "0.1.0", &store).unwrap();

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

        let outcome = run_with_store(&mut ui, "0.1.0", &store).unwrap();

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

        let outcome = run_with_store(&mut ui, "0.1.0", &store).unwrap();

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
        run_with_store(&mut first_ui, "0.1.0", &store).unwrap();

        let mut second_ui = RecordingUi::default();
        let outcome = run_with_store(&mut second_ui, "0.1.0", &store).unwrap();

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
    fn a_partial_saved_configuration_prompts_only_for_the_missing_language() {
        let directory = tempdir().unwrap();
        let store = ConfigStore::from_path(directory.path().join("config.json"));
        fs::write(store.path(), r#"{"tmdb_api_key":"stored-api-key"}"#).unwrap();
        let mut ui = RecordingUi::default();

        let outcome = run_with_store(&mut ui, "0.1.0", &store).unwrap();

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
        run_with_store(&mut first_ui, "0.1.0", &store).unwrap();

        let mut ui = RecordingUi::default();
        let outcome = run_config_with_store(&mut ui, "0.1.0", &store).unwrap();

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
        run_with_store(&mut first_ui, "0.1.0", &store).unwrap();
        let original = fs::read_to_string(store.path()).unwrap();

        let mut ui = RecordingUi {
            cancel_on_secret: true,
            ..RecordingUi::default()
        };
        let outcome = run_config_with_store(&mut ui, "0.1.0", &store).unwrap();

        assert_eq!(outcome, RunOutcome::Cancelled);
        assert_eq!(fs::read_to_string(store.path()).unwrap(), original);
    }
}
