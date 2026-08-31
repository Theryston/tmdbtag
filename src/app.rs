use crate::{
    config::{StartupConfig, tmdb_api_key_default, tmdb_language_default},
    domain::RunOutcome,
    error::AppResult,
    ui::{InteractiveUi, MessageLevel},
};

/// Runs the currently implemented portion of the interactive workflow.
///
/// Task 01 intentionally stops after collecting local startup values. TMDB validation, current
/// directory discovery, and media organization are owned by later tasks. Keeping this boundary
/// explicit prevents the foundation from pretending that an unimplemented move workflow exists.
pub fn run<U: InteractiveUi>(ui: &mut U, version: &str) -> AppResult<RunOutcome> {
    ui.show_welcome(version)?;

    let api_key_default = tmdb_api_key_default();
    let language_default = tmdb_language_default();

    let startup_config = loop {
        ui.show_step(1, 2, "TMDB API key")?;
        let Some(api_key) = ui.ask_masked_secret("TMDB API key", api_key_default.as_deref())?
        else {
            ui.show_message(MessageLevel::Info, "Startup configuration canceled.")?;
            return Ok(RunOutcome::Cancelled);
        };

        ui.show_step(2, 2, "TMDB metadata language")?;
        let Some(language) =
            ui.ask_text("TMDB metadata language", Some(language_default.as_str()))?
        else {
            ui.show_message(MessageLevel::Info, "Startup configuration canceled.")?;
            return Ok(RunOutcome::Cancelled);
        };

        match StartupConfig::new(api_key, language) {
            Ok(config) => break config,
            Err(error) => {
                ui.show_message(MessageLevel::Error, &error.to_string())?;
            }
        }
    };

    // Keep the value alive until the startup stage is complete. Later tasks will pass it to the
    // TMDB client, while the secret remains outside UI messages, plans, and debug output.
    let _startup_config = startup_config;
    ui.show_message(MessageLevel::Success, "Startup configuration captured.")?;
    ui.show_message(
        MessageLevel::Info,
        "TMDB verification and media organization will be connected in the next tasks.",
    )?;

    Ok(RunOutcome::StartupConfigured)
}

#[cfg(test)]
mod tests {
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
        let mut ui = RecordingUi::default();

        let outcome = run(&mut ui, "0.1.0").unwrap();

        assert_eq!(outcome, RunOutcome::StartupConfigured);
        assert_eq!(ui.events[1], "step:1/2:TMDB API key");
        assert_eq!(ui.events[2], "secret:TMDB API key");
        assert_eq!(ui.events[3], "step:2/2:TMDB metadata language");
        assert_eq!(ui.events[4], "text:TMDB metadata language");
    }

    #[test]
    fn canceling_the_api_key_prompt_performs_no_later_work() {
        let mut ui = RecordingUi {
            cancel_on_secret: true,
            ..RecordingUi::default()
        };

        let outcome = run(&mut ui, "0.1.0").unwrap();

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
        let mut ui = RecordingUi {
            cancel_on_language: true,
            ..RecordingUi::default()
        };

        let outcome = run(&mut ui, "0.1.0").unwrap();

        assert_eq!(outcome, RunOutcome::Cancelled);
        assert!(
            ui.events
                .iter()
                .any(|event| event == "message:Info:Startup configuration canceled.")
        );
        assert!(
            !ui.events
                .iter()
                .any(|event| { event == "message:Success:Startup configuration captured." })
        );
    }
}
