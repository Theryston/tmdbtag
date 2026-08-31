use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet, btree_map::Entry},
    io,
    path::{Component, Path, PathBuf},
    time::Duration,
};

use clap::{Parser, Subcommand};
use dialoguer::{
    Confirm, Input, MultiSelect, Password, Select, console::Key, theme::ColorfulTheme,
};
use indicatif::{ProgressBar, ProgressStyle};

use crate::{
    app,
    domain::{
        EpisodeRef, IdentificationMethod, MediaType, OperationStatus, RunOutcome, SourceRoot,
        TmdbItem, TmdbSearchCandidate, VideoFile,
    },
    error::{AppError, AppResult, UiError, UiResult},
    ui::{InteractiveUi, MessageLevel, ProgressOutput, TmdbInteraction},
};

const MULTI_SELECT_SEARCH_THRESHOLD: usize = 10;
const FILE_CONTEXT_INNER_WIDTH: usize = 72;
const MEDIA_EXPLORER_RESERVED_LINES: usize = 6;

/// Command-line arguments for the default interactive workflow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Parser)]
#[command(
    name = "title-tmdb-file",
    version,
    about = "Organize video files with verified TMDB metadata.",
    long_about = "A polished interactive CLI for selecting video files, identifying movies or TV series in TMDB, and preparing safe metadata-bearing filenames.",
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
    file_context: Option<FileContext>,
}

#[derive(Debug, Clone, Copy)]
struct FileContext;

#[derive(Debug)]
struct MediaExplorer {
    root: ExplorerDirectory,
}

#[derive(Debug)]
struct ExplorerDirectory {
    path: PathBuf,
    children: BTreeMap<std::ffi::OsString, ExplorerNode>,
}

#[derive(Debug)]
enum ExplorerNode {
    Directory(ExplorerDirectory),
    Video { file_index: usize, path: PathBuf },
}

#[derive(Debug, Clone)]
struct VisibleExplorerEntry {
    path: PathBuf,
    file_index: Option<usize>,
    parent: Option<PathBuf>,
    depth: usize,
    is_directory: bool,
    expanded: bool,
}

impl MediaExplorer {
    fn from_files(source_root: &Path, files: &[VideoFile]) -> UiResult<Self> {
        let mut explorer = Self {
            root: ExplorerDirectory::new(source_root.to_owned()),
        };

        for (file_index, file) in files.iter().enumerate() {
            let relative =
                file.path()
                    .strip_prefix(source_root)
                    .map_err(|_| UiError::InvalidSelection {
                        context: "media explorer path",
                    })?;
            let mut components = Vec::new();
            for component in relative.components() {
                match component {
                    Component::Normal(name) => components.push(name.to_owned()),
                    Component::CurDir => {}
                    Component::Prefix(_) | Component::RootDir | Component::ParentDir => {
                        return Err(UiError::InvalidSelection {
                            context: "media explorer path",
                        });
                    }
                }
            }
            if components.is_empty() {
                return Err(UiError::InvalidSelection {
                    context: "media explorer path",
                });
            }
            explorer
                .root
                .insert_video(&components, file_index, file.path().to_owned())?;
        }

        Ok(explorer)
    }

    fn visible_entries(&self, expanded: &BTreeSet<PathBuf>) -> Vec<VisibleExplorerEntry> {
        let mut visible = Vec::new();
        self.root.collect_visible(expanded, 0, None, &mut visible);
        visible
    }
}

impl ExplorerDirectory {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            children: BTreeMap::new(),
        }
    }

    fn insert_video(
        &mut self,
        components: &[std::ffi::OsString],
        file_index: usize,
        path: PathBuf,
    ) -> UiResult<()> {
        let Some(name) = components.first() else {
            return Err(UiError::InvalidSelection {
                context: "media explorer path",
            });
        };

        if components.len() == 1 {
            return match self.children.entry(name.clone()) {
                Entry::Vacant(entry) => {
                    entry.insert(ExplorerNode::Video { file_index, path });
                    Ok(())
                }
                Entry::Occupied(_) => Err(UiError::InvalidSelection {
                    context: "media explorer path",
                }),
            };
        }

        let directory_path = self.path.join(name);
        match self.children.entry(name.clone()) {
            Entry::Vacant(entry) => {
                let mut directory = ExplorerDirectory::new(directory_path);
                directory.insert_video(&components[1..], file_index, path)?;
                entry.insert(ExplorerNode::Directory(directory));
                Ok(())
            }
            Entry::Occupied(mut entry) => match entry.get_mut() {
                ExplorerNode::Directory(directory) => {
                    directory.insert_video(&components[1..], file_index, path)
                }
                ExplorerNode::Video { .. } => Err(UiError::InvalidSelection {
                    context: "media explorer path",
                }),
            },
        }
    }

    fn collect_visible(
        &self,
        expanded: &BTreeSet<PathBuf>,
        depth: usize,
        parent: Option<&Path>,
        visible: &mut Vec<VisibleExplorerEntry>,
    ) {
        let mut children = self.children.values().collect::<Vec<_>>();
        children.sort_by(|left, right| compare_explorer_paths(left.path(), right.path()));

        for child in children {
            match child {
                ExplorerNode::Directory(directory) => {
                    let is_expanded = expanded.contains(&directory.path);
                    visible.push(VisibleExplorerEntry {
                        path: directory.path.clone(),
                        file_index: None,
                        parent: parent.map(Path::to_owned),
                        depth,
                        is_directory: true,
                        expanded: is_expanded,
                    });
                    if is_expanded {
                        directory.collect_visible(
                            expanded,
                            depth + 1,
                            Some(&directory.path),
                            visible,
                        );
                    }
                }
                ExplorerNode::Video { file_index, path } => {
                    visible.push(VisibleExplorerEntry {
                        path: path.clone(),
                        file_index: Some(*file_index),
                        parent: parent.map(Path::to_owned),
                        depth,
                        is_directory: false,
                        expanded: false,
                    });
                }
            }
        }
    }
}

impl ExplorerNode {
    fn path(&self) -> &Path {
        match self {
            Self::Directory(directory) => &directory.path,
            Self::Video { path, .. } => path,
        }
    }
}

fn compare_explorer_paths(left: &Path, right: &Path) -> Ordering {
    let left_key = left.to_string_lossy().to_lowercase();
    let right_key = right.to_string_lossy().to_lowercase();
    left_key
        .cmp(&right_key)
        .then_with(|| left.to_string_lossy().cmp(&right.to_string_lossy()))
}

impl TerminalUi {
    /// Creates a terminal adapter with the colorful, keyboard-oriented dialoguer theme.
    pub fn new() -> Self {
        Self {
            terminal: dialoguer::console::Term::stderr(),
            theme: ColorfulTheme::default(),
            file_context: None,
        }
    }

    /// Returns whether both prompt input and rendered output are attached to terminals.
    pub fn is_interactive(&self) -> bool {
        std::io::IsTerminal::is_terminal(&io::stdin()) && self.terminal.is_term()
    }

    fn write_line(&self, line: &str) -> UiResult<()> {
        self.terminal.write_line(line).map_err(UiError::Prompt)
    }

    fn contextual_text(&self, value: &str) -> String {
        match self.file_context {
            Some(_) => format!("│ {value}"),
            None => value.to_owned(),
        }
    }

    fn file_context_border(top: bool) -> String {
        let (left, right) = if top { ('╭', '╮') } else { ('╰', '╯') };
        format!("{left}{}{right}", "─".repeat(FILE_CONTEXT_INNER_WIDTH + 2))
    }

    fn file_context_line(value: &str) -> String {
        let value = truncate_terminal_text(value, FILE_CONTEXT_INNER_WIDTH);
        let padding = " ".repeat(FILE_CONTEXT_INNER_WIDTH - value.chars().count());
        format!("│ {value}{padding} │")
    }

    fn filter_items(&mut self, prompt: &str, items: &[String]) -> UiResult<Option<Vec<usize>>> {
        loop {
            let filter_prompt =
                self.contextual_text(&format!("Filter {prompt} (leave empty to show all)"));
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

    fn run_media_explorer(
        &mut self,
        source_root: &Path,
        files: &[VideoFile],
    ) -> UiResult<Option<Vec<usize>>> {
        let explorer = MediaExplorer::from_files(source_root, files)?;
        let mut expanded = BTreeSet::new();
        let mut selected = vec![false; files.len()];
        let mut cursor = 0;
        let mut rendered_lines = 0;

        self.terminal.hide_cursor().map_err(UiError::Prompt)?;
        let interaction = self.run_media_explorer_loop(
            source_root,
            &explorer,
            &mut expanded,
            &mut selected,
            &mut cursor,
            &mut rendered_lines,
        );
        if interaction.is_err() {
            let _ = self.clear_media_explorer(&mut rendered_lines);
        }
        let restore_cursor = self.terminal.show_cursor().map_err(UiError::Prompt);

        restore_cursor?;
        interaction
    }

    fn run_media_explorer_loop(
        &mut self,
        source_root: &Path,
        explorer: &MediaExplorer,
        expanded: &mut BTreeSet<PathBuf>,
        selected: &mut [bool],
        cursor: &mut usize,
        rendered_lines: &mut usize,
    ) -> UiResult<Option<Vec<usize>>> {
        let mut notice = None;

        loop {
            let visible = explorer.visible_entries(expanded);
            if visible.is_empty() {
                self.clear_media_explorer(rendered_lines)?;
                return Err(UiError::EmptySelection {
                    context: "media explorer",
                });
            }
            *cursor = (*cursor).min(visible.len() - 1);
            self.clear_media_explorer(rendered_lines)?;
            *rendered_lines = self.render_media_explorer(
                source_root,
                &visible,
                selected,
                *cursor,
                notice.as_deref(),
            )?;

            let key = self.terminal.read_key().map_err(UiError::Prompt)?;
            let current = &visible[*cursor];
            match key {
                Key::ArrowDown | Key::Char('j') => {
                    *cursor = (*cursor + 1) % visible.len();
                    notice = None;
                }
                Key::ArrowUp | Key::Char('k') => {
                    *cursor = if *cursor == 0 {
                        visible.len() - 1
                    } else {
                        *cursor - 1
                    };
                    notice = None;
                }
                Key::Home => {
                    *cursor = 0;
                    notice = None;
                }
                Key::End => {
                    *cursor = visible.len() - 1;
                    notice = None;
                }
                Key::Tab => {
                    if current.is_directory && !expanded.remove(&current.path) {
                        expanded.insert(current.path.clone());
                    }
                    notice = None;
                }
                Key::BackTab => {
                    if current.is_directory {
                        expanded.remove(&current.path);
                    } else if let Some(parent) = &current.parent
                        && let Some(parent_position) =
                            visible.iter().position(|entry| entry.path == *parent)
                    {
                        *cursor = parent_position;
                    }
                    notice = None;
                }
                Key::ArrowRight => {
                    if current.is_directory {
                        expanded.insert(current.path.clone());
                    }
                    notice = None;
                }
                Key::ArrowLeft => {
                    if current.is_directory && expanded.remove(&current.path) {
                        notice = None;
                    } else if let Some(parent) = &current.parent {
                        if let Some(parent_position) =
                            visible.iter().position(|entry| entry.path == *parent)
                        {
                            *cursor = parent_position;
                        }
                        notice = None;
                    }
                }
                Key::Char(' ') => {
                    if let Some(file_index) = current.file_index {
                        selected[file_index] = !selected[file_index];
                    }
                    notice = None;
                }
                Key::Enter => {
                    let selected_indices = selected
                        .iter()
                        .enumerate()
                        .filter_map(|(index, is_selected)| is_selected.then_some(index))
                        .collect::<Vec<_>>();
                    if selected_indices.is_empty() {
                        notice =
                            Some("Select at least one video file before continuing.".to_owned());
                        continue;
                    }

                    self.clear_media_explorer(rendered_lines)?;
                    self.write_line(&format!(
                        "✔ Selected {} video file(s).",
                        selected_indices.len()
                    ))?;
                    return Ok(Some(selected_indices));
                }
                Key::Escape | Key::Char('q') | Key::CtrlC => {
                    self.clear_media_explorer(rendered_lines)?;
                    return Ok(None);
                }
                _ => {}
            }
        }
    }

    fn render_media_explorer(
        &self,
        source_root: &Path,
        visible: &[VisibleExplorerEntry],
        selected: &[bool],
        cursor: usize,
        notice: Option<&str>,
    ) -> UiResult<usize> {
        let terminal_height = usize::from(self.terminal.size().0);
        let row_capacity = terminal_height
            .saturating_sub(MEDIA_EXPLORER_RESERVED_LINES)
            .max(5);
        let start = cursor
            .saturating_sub(row_capacity / 2)
            .min(visible.len().saturating_sub(row_capacity));
        let end = (start + row_capacity).min(visible.len());
        let terminal_width = usize::from(self.terminal.size().1).max(40);
        let mut lines = Vec::new();

        lines.push(format!(
            "{}  {}",
            dialoguer::console::style("Video explorer").cyan().bold(),
            dialoguer::console::style("Select files from the current directory").dim()
        ));
        lines.push(format!(
            "  Current directory: {}",
            terminal_text(&crate::ui::display_relative_path(source_root, source_root))
        ));

        for (visible_index, entry) in visible.iter().enumerate().skip(start).take(end - start) {
            let path = crate::ui::display_relative_path(&entry.path, source_root);
            let label_width = terminal_width
                .saturating_sub(8 + entry.depth.saturating_mul(2))
                .max(12);
            let label = truncate_terminal_text(&terminal_text(&path), label_width);
            let indent = "  ".repeat(entry.depth);
            let cursor_marker = if visible_index == cursor {
                dialoguer::console::style("›").cyan().bold().to_string()
            } else {
                " ".to_owned()
            };
            let line = if entry.is_directory {
                let folder_marker = if entry.expanded { "▾" } else { "▸" };
                format!(
                    "{cursor_marker} {} {indent}{label}",
                    dialoguer::console::style(folder_marker).yellow()
                )
            } else {
                let file_index = entry.file_index.ok_or(UiError::InvalidSelection {
                    context: "media explorer file",
                })?;
                let selection_marker = if selected[file_index] {
                    dialoguer::console::style("✔").green().to_string()
                } else {
                    dialoguer::console::style("○").dim().to_string()
                };
                format!("{cursor_marker} {selection_marker} {indent}{label}")
            };
            lines.push(line);
        }

        lines.push(format!(
            "  Showing {}-{} of {} entries · Selected: {}",
            start + 1,
            end,
            visible.len(),
            selected.iter().filter(|is_selected| **is_selected).count()
        ));
        lines.push(
            "  ↑/↓ move · Space select · Tab expand/collapse · Enter confirm · Esc cancel"
                .to_owned(),
        );
        if let Some(notice) = notice {
            lines.push(format!(
                "  {} {}",
                dialoguer::console::style("!").yellow().bold(),
                terminal_text(notice)
            ));
        }

        let output = format!("{}\n", lines.join("\n"));
        self.terminal.write_str(&output).map_err(UiError::Prompt)?;
        Ok(lines.len())
    }

    fn clear_media_explorer(&self, rendered_lines: &mut usize) -> UiResult<()> {
        if *rendered_lines == 0 {
            return Ok(());
        }
        self.terminal
            .clear_last_lines(*rendered_lines)
            .map_err(UiError::Prompt)?;
        *rendered_lines = 0;
        Ok(())
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

    fn show_file_context(
        &mut self,
        current_file: usize,
        total_files: usize,
        file_path: &Path,
        source_root: &Path,
    ) -> UiResult<()> {
        self.file_context = Some(FileContext);
        let relative_path = crate::ui::display_relative_path(file_path, source_root);

        self.write_line("")?;
        self.write_line(&Self::file_context_border(true))?;
        self.write_line(&Self::file_context_line(&format!(
            "File {} of {}",
            current_file, total_files
        )))?;
        self.write_line(&Self::file_context_line("TMDB choices for this file"))?;
        self.write_line(&Self::file_context_line(&format!(
            "Source: {}",
            terminal_text(&relative_path)
        )))
    }

    fn finish_file_context(&mut self) -> UiResult<()> {
        let result = if self.file_context.is_some() {
            self.write_line(&Self::file_context_border(false))
        } else {
            Ok(())
        };
        self.file_context = None;
        result
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
        let prompt = self.contextual_text(prompt);
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
                    .with_prompt(self.contextual_text(&format!("{prompt} (type a filter above)")))
                    .items(&visible_items)
                    .max_length(12)
                    .interact_opt(),
            )?;

            Ok(selection.map(|position| visible_indices[position]))
        } else {
            map_optional_prompt(
                Select::with_theme(&self.theme)
                    .with_prompt(self.contextual_text(prompt))
                    .items(items)
                    .max_length(12)
                    .interact_opt(),
            )
        }
    }

    fn select_video_files(
        &mut self,
        source_root: &SourceRoot,
        files: &[VideoFile],
    ) -> UiResult<Option<Vec<usize>>> {
        if files.is_empty() {
            return Err(UiError::EmptySelection {
                context: "video file",
            });
        }

        self.run_media_explorer(source_root.path(), files)
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
                .with_prompt(
                    self.contextual_text(&format!("{prompt} (Space to toggle, Enter to confirm)")),
                )
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
        let prompt = self.contextual_text(prompt);
        map_optional_prompt(
            Confirm::with_theme(&self.theme)
                .with_prompt(prompt)
                .default(default)
                .interact_opt(),
        )
    }

    fn show_message(&mut self, level: MessageLevel, message: &str) -> UiResult<()> {
        let message = self.contextual_text(message);
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
        progress.set_message(self.contextual_text(message));

        Ok(Box::new(IndicatifProgress {
            progress,
            contextual: self.file_context.is_some(),
        }))
    }

    fn show_plan_preview(&mut self, plan: &crate::domain::OperationPlan) -> UiResult<()> {
        let source_root = plan.source_root().path();
        let destination_status = if plan.destination().exists() {
            "existing directory"
        } else {
            "will be created after final confirmation"
        };

        self.write_line("")?;
        self.write_line(&format!(
            "{}",
            dialoguer::console::style("Operation preview").cyan().bold()
        ))?;
        self.write_line(&format!(
            "Destination: {} ({destination_status})",
            crate::ui::display_relative_path(plan.destination().path(), source_root)
        ))?;
        self.write_line(&format!("Total files: {}", plan.operation_count()))?;
        self.write_line("")?;

        for operation in plan.operations() {
            let source = crate::ui::display_relative_path(operation.source_path(), source_root);
            let destination =
                crate::ui::display_relative_path(operation.destination_path(), source_root);
            let item = operation.tmdb_item();
            let episode = operation
                .episode()
                .map(|episode| format!(" · S{:02}E{:02}", episode.season(), episode.episode()))
                .unwrap_or_default();

            self.write_line(&format!("  {source} -> {destination}"))?;
            self.write_line(&format!(
                "    TMDB: {} [{}] {}{episode}",
                item.id,
                item.media_type,
                terminal_text(&item.title)
            ))?;
            self.write_line(&format!(
                "    Filename: {}",
                terminal_text(operation.normalized_filename())
            ))?;
        }

        self.write_line("")
    }

    fn show_execution_report(&mut self, report: &crate::domain::ExecutionReport) -> UiResult<()> {
        let source_root = report.source_root().path();
        self.write_line("")?;
        self.write_line(&format!(
            "{}",
            dialoguer::console::style("Execution report").cyan().bold()
        ))?;
        self.write_line(&format!(
            "Completed: {} · Failed: {} · Pending: {}",
            report.completed_count(),
            report.failed_count(),
            report.pending_count()
        ))?;

        for result in report.results() {
            let source = crate::ui::display_relative_path(result.source_path(), source_root);
            let destination =
                crate::ui::display_relative_path(result.destination_path(), source_root);
            match result.status() {
                OperationStatus::Completed => {
                    self.write_line(&format!("  ✔ Completed: {source} -> {destination}"))?;
                }
                OperationStatus::Failed { reason } => {
                    self.write_line(&format!(
                        "  ✘ Failed: {source} -> {destination} ({})",
                        terminal_text(reason)
                    ))?;
                }
                OperationStatus::Pending => {
                    self.write_line(&format!("  · Pending: {source} -> {destination}"))?;
                }
            }
        }

        self.write_line("")
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
    contextual: bool,
}

impl IndicatifProgress {
    fn contextual_message(&self, message: &str) -> String {
        if self.contextual {
            format!("│ {message}")
        } else {
            message.to_owned()
        }
    }
}

impl ProgressOutput for IndicatifProgress {
    fn set_message(&self, message: &str) {
        self.progress.set_message(self.contextual_message(message));
    }

    fn finish_success(&self, message: &str) {
        self.progress
            .finish_with_message(self.contextual_message(&format!("✔ {message}")));
    }

    fn finish_error(&self, message: &str) {
        self.progress
            .finish_with_message(self.contextual_message(&format!("✘ {message}")));
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

fn truncate_terminal_text(value: &str, max_chars: usize) -> String {
    let characters: Vec<char> = value.chars().collect();
    if characters.len() <= max_chars {
        return value.to_owned();
    }
    if max_chars == 0 {
        return String::new();
    }
    if max_chars == 1 {
        return "…".to_owned();
    }

    let suffix_length = max_chars - 1;
    let suffix: String = characters[characters.len() - suffix_length..]
        .iter()
        .collect();
    format!("…{suffix}")
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

    #[test]
    fn file_context_truncation_preserves_the_filename_suffix_and_unicode_boundaries() {
        assert_eq!(
            truncate_terminal_text("a very long path/episode.mkv", 12),
            "…episode.mkv"
        );
        assert_eq!(truncate_terminal_text("áéíóú", 4), "…íóú");
        assert_eq!(truncate_terminal_text("anything", 0), "");
        assert_eq!(truncate_terminal_text("anything", 1), "…");
    }

    #[test]
    fn media_explorer_starts_collapsed_and_reveals_nested_entries_explicitly() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let files = vec![
            VideoFile::new(root.join("movie.mp4"), Some(1)),
            VideoFile::new(root.join("shows").join("episode.mkv"), Some(2)),
            VideoFile::new(
                root.join("shows").join("season-02").join("episode-02.webm"),
                Some(3),
            ),
            VideoFile::new(
                root.join("shows")
                    .join("season-02")
                    .join("bonus")
                    .join("episode-03.mp4"),
                Some(4),
            ),
        ];
        let explorer = MediaExplorer::from_files(root, &files).unwrap();

        let collapsed = explorer.visible_entries(&BTreeSet::new());
        assert_eq!(
            collapsed
                .iter()
                .map(|entry| crate::ui::display_relative_path(&entry.path, root))
                .collect::<Vec<_>>(),
            vec!["movie.mp4", "shows"]
        );
        assert!(collapsed[1].is_directory);
        assert!(!collapsed[1].expanded);

        let mut expanded = BTreeSet::new();
        expanded.insert(root.join("shows"));
        let shows_expanded = explorer.visible_entries(&expanded);
        assert_eq!(
            shows_expanded
                .iter()
                .map(|entry| crate::ui::display_relative_path(&entry.path, root))
                .collect::<Vec<_>>(),
            vec!["movie.mp4", "shows", "shows/episode.mkv", "shows/season-02"]
        );
        assert!(shows_expanded[3].is_directory);
        assert!(!shows_expanded[3].expanded);

        expanded.insert(root.join("shows").join("season-02"));
        let fully_expanded = explorer.visible_entries(&expanded);
        let fully_expanded_paths = fully_expanded
            .iter()
            .map(|entry| crate::ui::display_relative_path(&entry.path, root))
            .collect::<Vec<_>>();
        assert_eq!(
            fully_expanded_paths,
            vec![
                "movie.mp4",
                "shows",
                "shows/episode.mkv",
                "shows/season-02",
                "shows/season-02/bonus",
                "shows/season-02/episode-02.webm"
            ]
        );
        assert!(!fully_expanded[4].expanded);
        assert_eq!(
            crate::ui::display_relative_path(&fully_expanded[5].path, root),
            "shows/season-02/episode-02.webm"
        );

        expanded.insert(root.join("shows").join("season-02").join("bonus"));
        let all_folders_expanded = explorer.visible_entries(&expanded);
        assert_eq!(all_folders_expanded.len(), 7);
        assert_eq!(
            crate::ui::display_relative_path(&all_folders_expanded[5].path, root),
            "shows/season-02/bonus/episode-03.mp4"
        );
    }
}
