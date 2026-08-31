use std::{io, time::Duration};

use clap::{Parser, Subcommand};
use dialoguer::{Confirm, Input, MultiSelect, Password, Select, theme::ColorfulTheme};
use indicatif::{ProgressBar, ProgressStyle};

use crate::{
    app,
    domain::{
        EpisodeRef, IdentificationMethod, MediaType, RunOutcome, TmdbItem, TmdbSearchCandidate,
    },
    error::{AppError, AppResult, UiError, UiResult},
    ui::{InteractiveUi, MessageLevel, ProgressOutput, TmdbInteraction},
};

const MULTI_SELECT_SEARCH_THRESHOLD: usize = 10;

/// Command-line arguments for the default interactive workflow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Parser)]
#[command(
    name = "title-tmdb-file",
    version,
    about = "Organize MKV videos with verified TMDB metadata.",
    long_about = "A polished interactive CLI for selecting MKV videos, identifying movies or TV series in TMDB, and preparing safe metadata-bearing filenames.",
    after_help = "Examples:\n  title-tmdb-file\n  title-tmdb-file config\n  title-tmdb-file --help\n  title-tmdb-file --version"
)]
pub struct Cli {
    /// Optional explicit command. Omitting it starts the organization wizard.
    #[command(subcommand)]
    pub command: Option<CliCommand>,
}

/// Explicit commands exposed by the clap boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Subcommand)]
pub enum CliCommand {
    /// Create or update the saved TMDB API key and metadata language.
    #[command(about = "Create or update the saved TMDB configuration.")]
    Config,
}

/// Executes the selected command after `clap` has parsed it.
pub fn execute(cli: Cli) -> AppResult<RunOutcome> {
    let mut ui = TerminalUi::new();
    if !ui.is_interactive() {
        return Err(AppError::NonInteractive);
    }

    match cli.command {
        Some(CliCommand::Config) => app::run_config(&mut ui, env!("CARGO_PKG_VERSION")),
        None => app::run(&mut ui, env!("CARGO_PKG_VERSION")),
    }
}

/// The concrete terminal adapter used by the interactive wizard.
pub struct TerminalUi {
    terminal: dialoguer::console::Term,
    theme: ColorfulTheme,
}

impl TerminalUi {
    /// Creates a terminal adapter with the colorful, keyboard-oriented dialoguer theme.
    pub fn new() -> Self {
        Self {
            terminal: dialoguer::console::Term::stderr(),
            theme: ColorfulTheme::default(),
        }
    }

    /// Returns whether both prompt input and rendered output are attached to terminals.
    pub fn is_interactive(&self) -> bool {
        std::io::IsTerminal::is_terminal(&io::stdin()) && self.terminal.is_term()
    }

    fn write_line(&self, line: &str) -> UiResult<()> {
        self.terminal.write_line(line).map_err(UiError::Prompt)
    }

    fn filter_items(&mut self, prompt: &str, items: &[String]) -> UiResult<Option<Vec<usize>>> {
        loop {
            let filter_prompt = format!("Filter {prompt} (leave empty to show all)");
            let result = Input::<String>::with_theme(&self.theme)
                .with_prompt(filter_prompt)
                .allow_empty(true)
                .interact();
            let Some(query) = map_prompt_result(result)? else {
                return Ok(None);
            };

            let indices = filter_item_indices(items, &query);
            if !indices.is_empty() {
                return Ok(Some(indices));
            }

            self.show_message(
                MessageLevel::Warning,
                "No items match that filter. Try again.",
            )?;
        }
    }
}

impl Default for TerminalUi {
    fn default() -> Self {
        Self::new()
    }
}

impl InteractiveUi for TerminalUi {
    fn show_welcome(&mut self, version: &str) -> UiResult<()> {
        self.write_line("")?;
        self.write_line(&format!(
            "{} {}",
            dialoguer::console::style("title-tmdb-file").cyan().bold(),
            dialoguer::console::style(format!("v{version}")).dim()
        ))?;
        self.write_line(&format!(
            "{}",
            dialoguer::console::style("Interactive TMDB media organizer").dim()
        ))?;
        self.write_line("")
    }

    fn show_step(&mut self, current: usize, total: usize, label: &str) -> UiResult<()> {
        self.write_line(&format!(
            "{} Step {current}/{total} · {label}",
            dialoguer::console::style("›").cyan().bold()
        ))
    }

    fn ask_masked_secret(
        &mut self,
        prompt: &str,
        default: Option<&str>,
    ) -> UiResult<Option<String>> {
        let prompt = if default.is_some() {
            format!("{prompt} [saved key available; press Enter to reuse]")
        } else {
            prompt.to_owned()
        };

        let result = Password::with_theme(&self.theme)
            .with_prompt(prompt)
            .allow_empty_password(true)
            .report(false)
            .interact();

        let Some(value) = map_prompt_result(result)? else {
            return Ok(None);
        };

        if value.is_empty() {
            Ok(default.map(ToOwned::to_owned))
        } else {
            Ok(Some(value))
        }
    }

    fn ask_text(&mut self, prompt: &str, default: Option<&str>) -> UiResult<Option<String>> {
        let mut input = Input::<String>::with_theme(&self.theme)
            .with_prompt(prompt)
            .allow_empty(true);
        if let Some(default) = default {
            input = input.default(default.to_owned());
        }

        map_prompt_result(input.interact())
    }

    fn select_one(
        &mut self,
        prompt: &str,
        items: &[String],
        searchable: bool,
    ) -> UiResult<Option<usize>> {
        if items.is_empty() {
            return Err(UiError::EmptySelection { context: "single" });
        }

        if searchable {
            let Some(visible_indices) = self.filter_items(prompt, items)? else {
                return Ok(None);
            };
            let visible_items: Vec<&str> = visible_indices
                .iter()
                .map(|&index| items[index].as_str())
                .collect();
            let selection = map_optional_prompt(
                Select::with_theme(&self.theme)
                    .with_prompt(format!("{prompt} (type a filter above)"))
                    .items(&visible_items)
                    .max_length(12)
                    .interact_opt(),
            )?;

            Ok(selection.map(|position| visible_indices[position]))
        } else {
            map_optional_prompt(
                Select::with_theme(&self.theme)
                    .with_prompt(prompt)
                    .items(items)
                    .max_length(12)
                    .interact_opt(),
            )
        }
    }

    fn select_many(
        &mut self,
        prompt: &str,
        items: &[String],
        searchable: bool,
    ) -> UiResult<Option<Vec<usize>>> {
        if items.is_empty() {
            return Err(UiError::EmptySelection {
                context: "multiple",
            });
        }

        let visible_indices = if searchable && items.len() > MULTI_SELECT_SEARCH_THRESHOLD {
            self.filter_items(prompt, items)?.unwrap_or_default()
        } else {
            (0..items.len()).collect()
        };

        if visible_indices.is_empty() {
            return Ok(None);
        }

        let visible_items: Vec<&str> = visible_indices
            .iter()
            .map(|&index| items[index].as_str())
            .collect();
        let selection = map_optional_prompt(
            MultiSelect::with_theme(&self.theme)
                .with_prompt(format!("{prompt} (Space to toggle, Enter to confirm)"))
                .items(&visible_items)
                .max_length(12)
                .interact_opt(),
        )?;

        Ok(selection.map(|positions| {
            positions
                .into_iter()
                .map(|position| visible_indices[position])
                .collect()
        }))
    }

    fn confirm(&mut self, prompt: &str, default: bool) -> UiResult<Option<bool>> {
        map_optional_prompt(
            Confirm::with_theme(&self.theme)
                .with_prompt(prompt)
                .default(default)
                .interact_opt(),
        )
    }

    fn show_message(&mut self, level: MessageLevel, message: &str) -> UiResult<()> {
        let (prefix, style_message) = match level {
            MessageLevel::Info => ("·", dialoguer::console::style(message).cyan()),
            MessageLevel::Success => ("✔", dialoguer::console::style(message).green()),
            MessageLevel::Warning => ("!", dialoguer::console::style(message).yellow()),
            MessageLevel::Error => ("✘", dialoguer::console::style(message).red()),
        };

        self.write_line(&format!(
            "{} {style_message}",
            dialoguer::console::style(prefix).bold()
        ))
    }

    fn start_activity(&mut self, message: &str) -> UiResult<Box<dyn ProgressOutput>> {
        let style = ProgressStyle::with_template("{spinner:.cyan} {msg}")
            .map_err(|error| UiError::ProgressStyle(error.to_string()))?;
        let progress = ProgressBar::new_spinner();
        progress.set_style(style);
        progress.enable_steady_tick(Duration::from_millis(80));
        progress.set_message(message.to_owned());

        Ok(Box::new(IndicatifProgress { progress }))
    }
}

impl TmdbInteraction for TerminalUi {
    fn choose_identification_method(&mut self) -> UiResult<Option<IdentificationMethod>> {
        let items = vec![
            "Search TMDB by title".to_owned(),
            "Enter a TMDB ID manually".to_owned(),
        ];
        let selection = self.select_one("How should this item be identified?", &items, false)?;
        match selection {
            None => Ok(None),
            Some(0) => Ok(Some(IdentificationMethod::Search)),
            Some(1) => Ok(Some(IdentificationMethod::ManualId)),
            Some(_) => Err(UiError::InvalidSelection {
                context: "identification method",
            }),
        }
    }

    fn choose_media_type(&mut self) -> UiResult<Option<MediaType>> {
        let items = vec!["Movie".to_owned(), "TV series".to_owned()];
        let selection = self.select_one("What type of TMDB item is this ID?", &items, false)?;
        match selection {
            None => Ok(None),
            Some(0) => Ok(Some(MediaType::Movie)),
            Some(1) => Ok(Some(MediaType::Series)),
            Some(_) => Err(UiError::InvalidSelection {
                context: "media type",
            }),
        }
    }

    fn ask_search_query(&mut self) -> UiResult<Option<String>> {
        self.ask_text("Search TMDB by title", None)
    }

    fn select_tmdb_result(
        &mut self,
        candidates: &[TmdbSearchCandidate],
    ) -> UiResult<Option<usize>> {
        if candidates.is_empty() {
            return Err(UiError::EmptySelection {
                context: "TMDB result",
            });
        }

        let items: Vec<String> = candidates
            .iter()
            .map(|candidate| {
                let year = candidate
                    .year
                    .map(|year| format!(" ({year})"))
                    .unwrap_or_default();
                format!(
                    "[{}] {} {}{}",
                    candidate.media_type,
                    candidate.id,
                    terminal_text(&candidate.title),
                    year
                )
            })
            .collect();
        let selection = self.select_one("Select a TMDB result", &items, true)?;
        match selection {
            None => Ok(None),
            Some(index) if index < candidates.len() => Ok(Some(index)),
            Some(_) => Err(UiError::InvalidSelection {
                context: "TMDB result",
            }),
        }
    }

    fn ask_tmdb_id(&mut self) -> UiResult<Option<String>> {
        self.ask_text("TMDB ID", None)
    }

    fn confirm_tmdb_item(&mut self, item: &TmdbItem) -> UiResult<Option<bool>> {
        let year = item
            .year
            .map(|year| format!(" ({year})"))
            .unwrap_or_default();
        let prompt = format!(
            "Use [{}] {} {}{}?",
            item.media_type,
            item.id,
            terminal_text(&item.title),
            year
        );
        self.confirm(&prompt, false)
    }

    fn ask_episode_numbers(&mut self, file_label: &str) -> UiResult<Option<(String, String)>> {
        let file_label = terminal_text(file_label);
        let Some(season) = self.ask_text(&format!("Season number for {file_label}"), None)? else {
            return Ok(None);
        };
        let Some(episode) = self.ask_text(&format!("Episode number for {file_label}"), None)?
        else {
            return Ok(None);
        };
        Ok(Some((season, episode)))
    }

    fn show_verified_episode(&mut self, episode: &EpisodeRef) -> UiResult<()> {
        self.show_message(
            MessageLevel::Success,
            &format!(
                "Verified episode S{:02}E{:02} through TMDB.",
                episode.season(),
                episode.episode()
            ),
        )
    }
}

#[derive(Debug)]
struct IndicatifProgress {
    progress: ProgressBar,
}

impl ProgressOutput for IndicatifProgress {
    fn set_message(&self, message: &str) {
        self.progress.set_message(message.to_owned());
    }

    fn finish_success(&self, message: &str) {
        self.progress.finish_with_message(format!("✔ {message}"));
    }

    fn finish_error(&self, message: &str) {
        self.progress.finish_with_message(format!("✘ {message}"));
    }
}

fn map_prompt_result<T>(result: dialoguer::Result<T>) -> UiResult<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(error) => match UiError::from_dialoguer(error) {
            UiError::Canceled => Ok(None),
            error => Err(error),
        },
    }
}

fn map_optional_prompt<T>(result: dialoguer::Result<Option<T>>) -> UiResult<Option<T>> {
    match result {
        Ok(value) => Ok(value),
        Err(error) => match UiError::from_dialoguer(error) {
            UiError::Canceled => Ok(None),
            error => Err(error),
        },
    }
}

fn filter_item_indices(items: &[String], query: &str) -> Vec<usize> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return (0..items.len()).collect();
    }

    items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| item.to_lowercase().contains(&query).then_some(index))
        .collect()
}

fn terminal_text(value: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;
    use clap::error::ErrorKind;

    #[test]
    fn default_invocation_parses_without_arguments() {
        let parsed = Cli::try_parse_from(["title-tmdb-file"]).unwrap();
        assert_eq!(parsed.command, None);
    }

    #[test]
    fn config_subcommand_is_owned_by_clap() {
        let parsed = Cli::try_parse_from(["title-tmdb-file", "config"]).unwrap();

        assert_eq!(parsed.command, Some(CliCommand::Config));
    }

    #[test]
    fn help_is_owned_by_clap() {
        let error = Cli::try_parse_from(["title-tmdb-file", "--help"]).unwrap_err();

        assert_eq!(error.kind(), ErrorKind::DisplayHelp);
        assert!(error.to_string().contains("A polished interactive CLI"));
        assert!(error.to_string().contains("Examples:"));
        assert!(error.to_string().contains("config"));
    }

    #[test]
    fn version_is_owned_by_clap() {
        let error = Cli::try_parse_from(["title-tmdb-file", "--version"]).unwrap_err();

        assert_eq!(error.kind(), ErrorKind::DisplayVersion);
        assert!(error.to_string().contains(env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn invalid_arguments_return_a_clap_usage_error() {
        let error = Cli::try_parse_from(["title-tmdb-file", "--unknown-option"]).unwrap_err();

        assert_eq!(error.kind(), ErrorKind::UnknownArgument);
    }

    #[test]
    fn multi_select_filter_is_case_insensitive_and_preserves_original_indices() {
        let items = vec![
            "The Office (2001)".to_owned(),
            "Fight Club".to_owned(),
            "The Office (1995)".to_owned(),
        ];

        assert_eq!(filter_item_indices(&items, "OFFICE"), vec![0, 2]);
        assert_eq!(filter_item_indices(&items, ""), vec![0, 1, 2]);
        assert!(filter_item_indices(&items, "unknown").is_empty());
    }
}
