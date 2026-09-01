use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet, btree_map::Entry},
    io,
    path::{Component, Path, PathBuf},
    time::{Duration, Instant},
};

use clap::{Parser, Subcommand};
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    terminal::{disable_raw_mode, enable_raw_mode},
};
use dialoguer::{
    Confirm, Input, MultiSelect, Password, Select, console::Key, theme::ColorfulTheme,
};
use indicatif::{ProgressBar, ProgressStyle};

use crate::{
    app,
    domain::{
        EpisodeRef, FileOperation, IdentificationMethod, MediaType, OperationStatus, RunOutcome,
        SourceRoot, StorageKind, StorageRole, TmdbItem, TmdbSearchCandidate, VideoFile,
    },
    error::{AppError, AppResult, TmdbError, UiError, UiResult},
    storage::{StorageDestination, StorageExecutionReport, StoragePlan, StorageVideoFile},
    ui::{InteractiveUi, MessageLevel, ProgressOutput, TmdbInteraction, format_transfer_speed},
};

const MULTI_SELECT_SEARCH_THRESHOLD: usize = 10;
const MEDIA_EXPLORER_RESERVED_LINES: usize = 6;
const TMDB_LIVE_SEARCH_DEBOUNCE_MS: u64 = 500;
const TMDB_LIVE_SEARCH_MIN_QUERY_CHARS: usize = 2;
const TMDB_LIVE_SEARCH_IDLE_POLL: Duration = Duration::from_secs(60 * 60);
const TMDB_LIVE_SEARCH_RESERVED_LINES: usize = 6;

/// Command-line arguments for the default interactive workflow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Parser)]
#[command(
    name = "tmdbtag",
    version,
    about = "Organize video files with verified TMDB metadata.",
    long_about = "A polished interactive CLI for selecting video files, identifying movies or TV series in TMDB, and preparing safe metadata-bearing filenames.",
    after_help = "Examples:\n  tmdbtag\n  tmdbtag config\n  tmdbtag storage add\n  tmdbtag storage remove\n  tmdbtag --help\n  tmdbtag --version"
)]
pub struct Cli {
    /// Optional explicit command. Omitting it starts the organization wizard.
    #[command(subcommand)]
    pub command: Option<CliCommand>,
}

/// Explicit commands exposed by the clap boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Subcommand)]
pub enum CliCommand {
    /// Create or update the saved TMDB configuration.
    #[command(about = "Create or update the saved TMDB configuration.")]
    Config,
    /// Manage saved S3 buckets.
    #[command(subcommand)]
    Storage(StorageCommand),
}

/// Commands for managing the saved S3 bucket catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Subcommand)]
pub enum StorageCommand {
    /// Add a named S3 bucket to the local catalog.
    #[command(about = "Add a named S3 bucket to the local catalog.")]
    Add,
    /// Remove one named S3 bucket from the local catalog.
    #[command(about = "Remove a named S3 bucket from the local catalog.")]
    Remove,
}

/// Executes the selected command after `clap` has parsed it.
pub fn execute(cli: Cli) -> AppResult<RunOutcome> {
    let mut ui = TerminalUi::new();
    if !ui.is_interactive() {
        return Err(AppError::NonInteractive);
    }

    match cli.command {
        Some(CliCommand::Config) => app::run_config(&mut ui, env!("CARGO_PKG_VERSION")),
        Some(CliCommand::Storage(StorageCommand::Add)) => {
            app::run_storage_add(&mut ui, env!("CARGO_PKG_VERSION"))
        }
        Some(CliCommand::Storage(StorageCommand::Remove)) => {
            app::run_storage_remove(&mut ui, env!("CARGO_PKG_VERSION"))
        }
        None => app::run(&mut ui, env!("CARGO_PKG_VERSION")),
    }
}

/// The concrete terminal adapter used by the interactive wizard.
pub struct TerminalUi {
    terminal: dialoguer::console::Term,
    theme: ColorfulTheme,
}

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

struct MediaExplorerState {
    expanded: BTreeSet<PathBuf>,
    selected: Vec<bool>,
    cursor: usize,
    rendered_lines: usize,
}

impl MediaExplorerState {
    fn new(file_count: usize) -> Self {
        Self {
            expanded: BTreeSet::new(),
            selected: vec![false; file_count],
            cursor: 0,
            rendered_lines: 0,
        }
    }
}

#[derive(Clone, Copy)]
struct MediaExplorerContext<'a> {
    subtitle: &'a str,
    root_description: &'a str,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum LiveSearchStatus {
    #[default]
    Empty,
    TooShort,
    Waiting,
    Searching,
    Ready,
    NoResults,
}

#[derive(Debug, Default)]
struct LiveSearchState {
    query: Vec<char>,
    cursor: usize,
    selected_result: usize,
    candidates: Vec<TmdbSearchCandidate>,
    searched_query: Option<String>,
    search_due_at: Option<Instant>,
    status: LiveSearchStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LiveSearchAction {
    Continue,
    Cancel,
    Accept,
}

#[derive(Debug)]
struct RawModeGuard;

impl RawModeGuard {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        Ok(Self)
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
    }
}

impl LiveSearchState {
    fn query(&self) -> String {
        self.query.iter().collect()
    }

    fn trimmed_query(&self) -> String {
        self.query().trim().to_owned()
    }

    fn query_changed(&mut self) {
        self.candidates.clear();
        self.searched_query = None;
        self.selected_result = 0;
        let query_length = self.trimmed_query().chars().count();
        self.status = match query_length {
            0 => LiveSearchStatus::Empty,
            1..TMDB_LIVE_SEARCH_MIN_QUERY_CHARS => LiveSearchStatus::TooShort,
            _ => LiveSearchStatus::Waiting,
        };
        self.search_due_at = (query_length >= TMDB_LIVE_SEARCH_MIN_QUERY_CHARS)
            .then(|| Instant::now() + Duration::from_millis(TMDB_LIVE_SEARCH_DEBOUNCE_MS));
    }

    fn search_is_due(&self) -> bool {
        self.search_due_at
            .is_some_and(|deadline| Instant::now() >= deadline)
    }

    fn search_wait(&self) -> Duration {
        self.search_due_at
            .map(|deadline| deadline.saturating_duration_since(Instant::now()))
            .unwrap_or(TMDB_LIVE_SEARCH_IDLE_POLL)
    }

    fn begin_search(&mut self) {
        self.status = LiveSearchStatus::Searching;
        self.search_due_at = None;
    }

    fn complete_search(&mut self, query: String, candidates: Vec<TmdbSearchCandidate>) {
        self.searched_query = Some(query);
        self.selected_result = 0;
        self.status = if candidates.is_empty() {
            LiveSearchStatus::NoResults
        } else {
            LiveSearchStatus::Ready
        };
        self.candidates = candidates;
    }

    fn selected_candidate(&self) -> Option<TmdbSearchCandidate> {
        let query = self.trimmed_query();
        if self.searched_query.as_deref() != Some(query.as_str()) {
            return None;
        }
        self.candidates.get(self.selected_result).cloned()
    }
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

    fn run_live_tmdb_search(
        &mut self,
        search: &mut dyn FnMut(&str) -> Result<Vec<TmdbSearchCandidate>, TmdbError>,
    ) -> UiResult<Result<Option<TmdbSearchCandidate>, TmdbError>> {
        let _raw_mode = RawModeGuard::enter().map_err(UiError::Prompt)?;
        self.terminal.hide_cursor().map_err(UiError::Prompt)?;

        let mut rendered_lines = 0;
        let interaction = self.run_live_tmdb_search_loop(search, &mut rendered_lines);
        let clear_result = self.clear_rendered_lines(&mut rendered_lines);
        let show_cursor_result = self.terminal.show_cursor().map_err(UiError::Prompt);

        show_cursor_result?;
        clear_result?;
        interaction
    }

    fn run_live_tmdb_search_loop(
        &mut self,
        search: &mut dyn FnMut(&str) -> Result<Vec<TmdbSearchCandidate>, TmdbError>,
        rendered_lines: &mut usize,
    ) -> UiResult<Result<Option<TmdbSearchCandidate>, TmdbError>> {
        let mut state = LiveSearchState::default();

        loop {
            if state.search_is_due() {
                state.begin_search();
                self.render_live_tmdb_search(&state, rendered_lines)?;
                let query = state.trimmed_query();
                match search(&query) {
                    Ok(candidates) => state.complete_search(query, candidates),
                    Err(error) => return Ok(Err(error)),
                }
                continue;
            }

            self.render_live_tmdb_search(&state, rendered_lines)?;
            if !event::poll(state.search_wait()).map_err(UiError::Prompt)? {
                continue;
            }

            let Event::Key(key) = event::read().map_err(UiError::Prompt)? else {
                continue;
            };
            match apply_live_search_key(&mut state, key) {
                LiveSearchAction::Continue => {}
                LiveSearchAction::Cancel => return Ok(Ok(None)),
                LiveSearchAction::Accept => return Ok(Ok(state.selected_candidate())),
            }
        }
    }

    fn render_live_tmdb_search(
        &self,
        state: &LiveSearchState,
        rendered_lines: &mut usize,
    ) -> UiResult<()> {
        self.clear_rendered_lines(rendered_lines)?;

        let terminal_height = usize::from(self.terminal.size().0);
        let terminal_width = usize::from(self.terminal.size().1).max(40);
        let query = terminal_text(&state.query());
        let query = truncate_terminal_text_end(&query, terminal_width.saturating_sub(10).max(12));
        let mut lines = vec![
            format!(
                "{} {}",
                dialoguer::console::style("TMDB live search").cyan().bold(),
                dialoguer::console::style("Search movies and TV series as you type").dim()
            ),
            format!("  Query: {query}"),
            format!("  {}", live_search_status_message(state)),
        ];

        if !state.candidates.is_empty() {
            let result_capacity = terminal_height
                .saturating_sub(lines.len() + TMDB_LIVE_SEARCH_RESERVED_LINES)
                .max(3);
            let start = state
                .selected_result
                .saturating_sub(result_capacity / 2)
                .min(state.candidates.len().saturating_sub(result_capacity));
            let end = (start + result_capacity).min(state.candidates.len());
            let label_width = terminal_width.saturating_sub(6).max(20);

            lines.push(format!(
                "  Results: {} · ↑/↓ choose · Enter select",
                state.candidates.len()
            ));
            for (index, candidate) in state
                .candidates
                .iter()
                .enumerate()
                .skip(start)
                .take(end - start)
            {
                let marker = if index == state.selected_result {
                    dialoguer::console::style("›").cyan().bold().to_string()
                } else {
                    " ".to_owned()
                };
                let label =
                    truncate_terminal_text_end(&format_tmdb_candidate(candidate), label_width);
                lines.push(format!("  {marker} {label}"));
            }
            if start > 0 || end < state.candidates.len() {
                lines.push(format!(
                    "  Showing {}-{} of {} results",
                    start + 1,
                    end,
                    state.candidates.len()
                ));
            }
        }

        lines.push("  Type to search · ↑/↓ choose · Enter select · Esc cancel".to_owned());
        self.terminal
            .write_str(&raw_terminal_frame(&lines))
            .map_err(UiError::Prompt)?;
        self.terminal.flush().map_err(UiError::Prompt)?;
        *rendered_lines = lines.len();
        Ok(())
    }

    fn run_media_explorer(
        &mut self,
        source_root: &Path,
        files: &[VideoFile],
    ) -> UiResult<Option<Vec<usize>>> {
        let root_description = format!(
            "Current directory: {}",
            crate::ui::display_relative_path(source_root, source_root)
        );
        self.run_media_explorer_with_context(
            source_root,
            files,
            "Select files from the current directory",
            &root_description,
        )
    }

    fn run_media_explorer_with_context(
        &mut self,
        source_root: &Path,
        files: &[VideoFile],
        subtitle: &str,
        root_description: &str,
    ) -> UiResult<Option<Vec<usize>>> {
        let explorer = MediaExplorer::from_files(source_root, files)?;
        let mut state = MediaExplorerState::new(files.len());
        let context = MediaExplorerContext {
            subtitle,
            root_description,
        };

        self.terminal.hide_cursor().map_err(UiError::Prompt)?;
        let interaction = self.run_media_explorer_loop(source_root, &explorer, &mut state, context);
        if interaction.is_err() {
            let _ = self.clear_rendered_lines(&mut state.rendered_lines);
        }
        let restore_cursor = self.terminal.show_cursor().map_err(UiError::Prompt);

        restore_cursor?;
        interaction
    }

    fn run_media_explorer_loop(
        &mut self,
        source_root: &Path,
        explorer: &MediaExplorer,
        state: &mut MediaExplorerState,
        context: MediaExplorerContext<'_>,
    ) -> UiResult<Option<Vec<usize>>> {
        let mut notice = None;

        loop {
            let visible = explorer.visible_entries(&state.expanded);
            if visible.is_empty() {
                self.clear_rendered_lines(&mut state.rendered_lines)?;
                return Err(UiError::EmptySelection {
                    context: "media explorer",
                });
            }
            state.cursor = state.cursor.min(visible.len() - 1);
            self.clear_rendered_lines(&mut state.rendered_lines)?;
            state.rendered_lines = self.render_media_explorer(
                source_root,
                &visible,
                &state.selected,
                state.cursor,
                notice.as_deref(),
                context,
            )?;

            let key = self.terminal.read_key().map_err(UiError::Prompt)?;
            let current = &visible[state.cursor];
            match key {
                Key::ArrowDown | Key::Char('j') => {
                    state.cursor = (state.cursor + 1) % visible.len();
                    notice = None;
                }
                Key::ArrowUp | Key::Char('k') => {
                    state.cursor = if state.cursor == 0 {
                        visible.len() - 1
                    } else {
                        state.cursor - 1
                    };
                    notice = None;
                }
                Key::Home => {
                    state.cursor = 0;
                    notice = None;
                }
                Key::End => {
                    state.cursor = visible.len() - 1;
                    notice = None;
                }
                Key::Tab => {
                    if current.is_directory && !state.expanded.remove(&current.path) {
                        state.expanded.insert(current.path.clone());
                    }
                    notice = None;
                }
                Key::BackTab => {
                    if current.is_directory {
                        state.expanded.remove(&current.path);
                    } else if let Some(parent) = &current.parent
                        && let Some(parent_position) =
                            visible.iter().position(|entry| entry.path == *parent)
                    {
                        state.cursor = parent_position;
                    }
                    notice = None;
                }
                Key::ArrowRight => {
                    if current.is_directory {
                        state.expanded.insert(current.path.clone());
                    }
                    notice = None;
                }
                Key::ArrowLeft => {
                    if current.is_directory && state.expanded.remove(&current.path) {
                        notice = None;
                    } else if let Some(parent) = &current.parent {
                        if let Some(parent_position) =
                            visible.iter().position(|entry| entry.path == *parent)
                        {
                            state.cursor = parent_position;
                        }
                        notice = None;
                    }
                }
                Key::Char(' ') => {
                    if let Some(file_index) = current.file_index {
                        state.selected[file_index] = !state.selected[file_index];
                    }
                    notice = None;
                }
                Key::Enter => {
                    let selected_indices = state
                        .selected
                        .iter()
                        .enumerate()
                        .filter_map(|(index, is_selected)| is_selected.then_some(index))
                        .collect::<Vec<_>>();
                    if selected_indices.is_empty() {
                        notice =
                            Some("Select at least one video file before continuing.".to_owned());
                        continue;
                    }

                    self.clear_rendered_lines(&mut state.rendered_lines)?;
                    self.write_line(&format!(
                        "✔ Selected {} video file(s).",
                        selected_indices.len()
                    ))?;
                    return Ok(Some(selected_indices));
                }
                Key::Escape | Key::Char('q') | Key::CtrlC => {
                    self.clear_rendered_lines(&mut state.rendered_lines)?;
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
        context: MediaExplorerContext<'_>,
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
            dialoguer::console::style(context.subtitle).dim()
        ));
        lines.push(format!("  {}", terminal_text(context.root_description)));

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

    fn clear_rendered_lines(&self, rendered_lines: &mut usize) -> UiResult<()> {
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
            dialoguer::console::style("tmdbtag").cyan().bold(),
            dialoguer::console::style(format!("v{version}")).dim()
        ))?;
        self.write_line(&format!(
            "{}",
            dialoguer::console::style("Interactive TMDB media organizer").dim()
        ))?;
        self.write_line("")
    }

    fn show_step(&mut self, current: usize, total: usize, label: &str) -> UiResult<()> {
        let line = format!(
            "{} Step {current}/{total} · {label}",
            dialoguer::console::style("›").cyan().bold()
        );
        self.write_line(&line)
    }

    fn choose_file_operation(&mut self) -> UiResult<Option<FileOperation>> {
        let items = vec![
            "Copy selected videos (keep originals)".to_owned(),
            "Move selected videos (remove originals after success)".to_owned(),
        ];
        let selection = self.select_one(
            "How should the selected videos be processed?",
            &items,
            false,
        )?;
        match selection {
            None => Ok(None),
            Some(0) => Ok(Some(FileOperation::Copy)),
            Some(1) => Ok(Some(FileOperation::Move)),
            Some(_) => Err(UiError::InvalidSelection {
                context: "file operation",
            }),
        }
    }

    fn choose_storage(&mut self, role: StorageRole) -> UiResult<Option<StorageKind>> {
        let items = vec![
            "Local filesystem".to_owned(),
            "S3-compatible object storage".to_owned(),
        ];
        let prompt = match role {
            StorageRole::Source => "Where are the source files stored?",
            StorageRole::Destination => "Where should organized files be written?",
        };
        match self.select_one(prompt, &items, false)? {
            None => Ok(None),
            Some(0) => Ok(Some(StorageKind::Local)),
            Some(1) => Ok(Some(StorageKind::S3)),
            Some(_) => Err(UiError::InvalidSelection {
                context: "storage backend",
            }),
        }
    }

    fn ask_storage_destination_path(&mut self, kind: StorageKind) -> UiResult<Option<String>> {
        let prompt = match kind {
            StorageKind::Local => "Local destination folder path",
            StorageKind::S3 => "S3 destination prefix (optional; empty = bucket root)",
        };
        self.ask_text(prompt, None)
    }

    fn confirm_storage_destination_creation(
        &mut self,
        _destination: &StorageDestination,
        display_path: &str,
    ) -> UiResult<Option<bool>> {
        self.confirm(
            &format!("Allow creation of destination {display_path} after final confirmation?"),
            false,
        )
    }

    fn select_storage_video_files(
        &mut self,
        source_description: &str,
        files: &[StorageVideoFile],
    ) -> UiResult<Option<Vec<usize>>> {
        if files.is_empty() {
            return Err(UiError::EmptySelection {
                context: "video file",
            });
        }

        // The existing explorer is intentionally reused for S3 objects by projecting their
        // relative object keys onto a virtual local root. The selected positions still refer to
        // the original backend-owned file array, so no remote path is ever converted into a
        // filesystem mutation target.
        let virtual_root = PathBuf::from(".tmdbtag-storage-root");
        let virtual_files = files
            .iter()
            .map(|file| VideoFile::new(virtual_root.join(file.relative_path()), file.size_bytes()))
            .collect::<Vec<_>>();
        let root_description = format!("Storage root: {source_description}");
        self.run_media_explorer_with_context(
            &virtual_root,
            &virtual_files,
            "Select files from the selected storage",
            &root_description,
        )
    }

    fn show_storage_file_context(
        &mut self,
        current_file: usize,
        total_files: usize,
        file: &StorageVideoFile,
    ) -> UiResult<()> {
        self.write_line("")?;
        self.write_line(&format!(
            "File {current_file} of {total_files} · {}",
            terminal_text(file.relative_path())
        ))
    }

    fn show_storage_plan_preview(&mut self, plan: &StoragePlan) -> UiResult<()> {
        self.write_line("")?;
        self.write_line(
            &dialoguer::console::style("Storage operation preview")
                .cyan()
                .bold()
                .to_string(),
        )?;
        self.write_line(&format!(
            "Source: {}",
            terminal_text(plan.source_description())
        ))?;
        self.write_line(&format!(
            "Destination: {}",
            terminal_text(plan.destination_description())
        ))?;
        self.write_line(&format!(
            "Operation: {}{}",
            plan.operation().label(),
            if plan.operation().preserves_source() {
                " (original files will be kept)"
            } else {
                " (original files will be removed after successful publication)"
            }
        ))?;
        self.write_line(&format!("Total files: {}", plan.operation_count()))?;
        self.write_line(&format!(
            "Total bytes: {}",
            crate::ui::format_file_size(plan.total_size_bytes())
        ))?;
        self.write_line("")?;

        for operation in plan.operations() {
            let item = operation.tmdb_item();
            let episode = operation
                .episode()
                .map(|episode| format!(" · S{:02}E{:02}", episode.season(), episode.episode()))
                .unwrap_or_default();
            self.write_line(&format!(
                "  {} -> {}",
                terminal_text(operation.source_display()),
                terminal_text(operation.destination_display())
            ))?;
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

    fn show_storage_execution_report(&mut self, report: &StorageExecutionReport) -> UiResult<()> {
        self.write_line("")?;
        self.write_line(
            &dialoguer::console::style(format!("{} execution report", report.operation().label()))
                .cyan()
                .bold()
                .to_string(),
        )?;
        self.write_line(&format!(
            "Completed: {} · Failed: {} · Pending: {}",
            report.completed_count(),
            report.failed_count(),
            report.pending_count()
        ))?;
        for result in report.results() {
            match result.status() {
                OperationStatus::Completed => self.write_line(&format!(
                    "  ✔ Completed: {} -> {}",
                    terminal_text(result.source_display()),
                    terminal_text(result.destination_display())
                ))?,
                OperationStatus::Failed { reason } => self.write_line(&format!(
                    "  ✘ Failed: {} -> {} ({})",
                    terminal_text(result.source_display()),
                    terminal_text(result.destination_display()),
                    terminal_text(reason)
                ))?,
                OperationStatus::Pending => self.write_line(&format!(
                    "  · Pending: {} -> {}",
                    terminal_text(result.source_display()),
                    terminal_text(result.destination_display())
                ))?,
            }
        }
        self.write_line("")
    }

    fn show_file_context(
        &mut self,
        current_file: usize,
        total_files: usize,
        file_path: &Path,
        source_root: &Path,
    ) -> UiResult<()> {
        self.write_line("")?;
        let relative_path = crate::ui::display_relative_path(file_path, source_root);

        self.write_line(&format!(
            "File {current_file} of {total_files} · {}",
            terminal_text(&relative_path)
        ))
    }

    fn finish_file_context(&mut self) -> UiResult<()> {
        Ok(())
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

        let (visible_indices, visible_items, selector_prompt) = if searchable {
            let Some(visible_indices) = self.filter_items(prompt, items)? else {
                return Ok(None);
            };
            let visible_items = visible_indices
                .iter()
                .map(|&index| items[index].clone())
                .collect::<Vec<_>>();
            (
                visible_indices,
                visible_items,
                format!("{prompt} (type a filter above)"),
            )
        } else {
            (
                (0..items.len()).collect(),
                items.to_vec(),
                prompt.to_owned(),
            )
        };

        let visible_item_labels = visible_items.iter().map(String::as_str).collect::<Vec<_>>();
        map_optional_prompt(
            Select::with_theme(&self.theme)
                .with_prompt(selector_prompt)
                .items(&visible_item_labels)
                .max_length(12)
                .interact_opt(),
        )
        .map(|selection| selection.map(|position| visible_indices[position]))
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
        let spinner_style = ProgressStyle::with_template("{spinner:.cyan} {msg}")
            .map_err(|error| UiError::ProgressStyle(error.to_string()))?;
        let transfer_style = ProgressStyle::with_template(
            "{spinner:.cyan} {bar:40.cyan/blue} {percent:>3}% {prefix} {msg}",
        )
        .map_err(|error| UiError::ProgressStyle(error.to_string()))?;
        let progress = ProgressBar::new_spinner();
        progress.set_style(spinner_style);
        progress.enable_steady_tick(Duration::from_millis(80));
        progress.set_message(message.to_owned());

        Ok(Box::new(IndicatifProgress {
            progress,
            transfer_style,
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
        self.write_line(&format!(
            "Operation: {}{}",
            plan.operation().label(),
            if plan.operation().preserves_source() {
                " (original files will be kept)"
            } else {
                " (original files will be removed after successful publication)"
            }
        ))?;
        self.write_line(&format!("Total files: {}", plan.operation_count()))?;
        self.write_line(&format!(
            "Total bytes: {}",
            crate::ui::format_file_size(plan.total_size_bytes())
        ))?;
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
            dialoguer::console::style(format!("{} execution report", report.operation().label()))
                .cyan()
                .bold()
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

    fn select_tmdb_result_live(
        &mut self,
        search: &mut dyn FnMut(&str) -> Result<Vec<TmdbSearchCandidate>, TmdbError>,
    ) -> UiResult<Result<Option<TmdbSearchCandidate>, TmdbError>> {
        self.run_live_tmdb_search(search)
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

struct IndicatifProgress {
    progress: ProgressBar,
    transfer_style: ProgressStyle,
}

impl std::fmt::Debug for IndicatifProgress {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("IndicatifProgress")
            .field("progress", &self.progress)
            .finish_non_exhaustive()
    }
}

impl ProgressOutput for IndicatifProgress {
    fn set_message(&self, message: &str) {
        self.progress.set_message(message.to_owned());
    }

    fn set_progress(&self, completed_bytes: u64, total_bytes: u64) {
        // An empty plan is not executable, but zero-byte videos are valid. Give that edge case a
        // one-unit visual scale so the completed operation still renders as 100%.
        self.progress.set_style(self.transfer_style.clone());
        let display_total = total_bytes.max(1);
        let display_position = if total_bytes == 0 {
            1
        } else {
            completed_bytes.min(total_bytes)
        };
        self.progress.set_length(display_total);
        self.progress.set_position(display_position);
        self.progress.set_prefix(format!(
            "Speed: {}",
            format_transfer_speed(self.progress.per_sec())
        ));
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

fn apply_live_search_key(state: &mut LiveSearchState, key: KeyEvent) -> LiveSearchAction {
    if key.kind == KeyEventKind::Release {
        return LiveSearchAction::Continue;
    }

    let control = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Esc => LiveSearchAction::Cancel,
        KeyCode::Char(character) if control && character.eq_ignore_ascii_case(&'c') => {
            LiveSearchAction::Cancel
        }
        KeyCode::Enter => {
            if state.selected_candidate().is_some() {
                LiveSearchAction::Accept
            } else {
                LiveSearchAction::Continue
            }
        }
        KeyCode::Up => {
            if !state.candidates.is_empty() {
                state.selected_result = if state.selected_result == 0 {
                    state.candidates.len() - 1
                } else {
                    state.selected_result - 1
                };
            }
            LiveSearchAction::Continue
        }
        KeyCode::Down => {
            if !state.candidates.is_empty() {
                state.selected_result = (state.selected_result + 1) % state.candidates.len();
            }
            LiveSearchAction::Continue
        }
        KeyCode::PageUp => {
            if !state.candidates.is_empty() {
                state.selected_result = state.selected_result.saturating_sub(5);
            }
            LiveSearchAction::Continue
        }
        KeyCode::PageDown => {
            if !state.candidates.is_empty() {
                state.selected_result =
                    (state.selected_result + 5).min(state.candidates.len().saturating_sub(1));
            }
            LiveSearchAction::Continue
        }
        KeyCode::Left => {
            state.cursor = state.cursor.saturating_sub(1);
            LiveSearchAction::Continue
        }
        KeyCode::Right => {
            state.cursor = (state.cursor + 1).min(state.query.len());
            LiveSearchAction::Continue
        }
        KeyCode::Home => {
            state.cursor = 0;
            LiveSearchAction::Continue
        }
        KeyCode::End => {
            state.cursor = state.query.len();
            LiveSearchAction::Continue
        }
        KeyCode::Backspace => {
            if state.cursor > 0 {
                state.query.remove(state.cursor - 1);
                state.cursor -= 1;
                state.query_changed();
            }
            LiveSearchAction::Continue
        }
        KeyCode::Delete => {
            if state.cursor < state.query.len() {
                state.query.remove(state.cursor);
                state.query_changed();
            }
            LiveSearchAction::Continue
        }
        KeyCode::Char('u') if control => {
            if !state.query.is_empty() {
                state.query.clear();
                state.cursor = 0;
                state.query_changed();
            }
            LiveSearchAction::Continue
        }
        KeyCode::Char('w') if control => {
            let original_cursor = state.cursor;
            while state.cursor > 0 && state.query[state.cursor - 1].is_whitespace() {
                state.query.remove(state.cursor - 1);
                state.cursor -= 1;
            }
            while state.cursor > 0 && !state.query[state.cursor - 1].is_whitespace() {
                state.query.remove(state.cursor - 1);
                state.cursor -= 1;
            }
            if original_cursor != state.cursor {
                state.query_changed();
            }
            LiveSearchAction::Continue
        }
        KeyCode::Char(character)
            if !character.is_control()
                && !key.modifiers.intersects(
                    KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER,
                ) =>
        {
            state.query.insert(state.cursor, character);
            state.cursor += 1;
            state.query_changed();
            LiveSearchAction::Continue
        }
        _ => LiveSearchAction::Continue,
    }
}

fn live_search_status_message(state: &LiveSearchState) -> String {
    match state.status {
        LiveSearchStatus::Empty => "Type a title to start searching.".to_owned(),
        LiveSearchStatus::TooShort => {
            format!("Type at least {TMDB_LIVE_SEARCH_MIN_QUERY_CHARS} characters to search.")
        }
        LiveSearchStatus::Waiting => "Waiting briefly before searching TMDB...".to_owned(),
        LiveSearchStatus::Searching => "Searching TMDB...".to_owned(),
        LiveSearchStatus::Ready => {
            format!("{} result(s) found.", state.candidates.len())
        }
        LiveSearchStatus::NoResults => {
            "No movies or TV series found. Keep typing to refine the search.".to_owned()
        }
    }
}

fn format_tmdb_candidate(candidate: &TmdbSearchCandidate) -> String {
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
}

fn raw_terminal_frame(lines: &[String]) -> String {
    let mut frame = lines.join("\r\n");
    frame.push_str("\r\n");
    frame
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

fn truncate_terminal_text_end(value: &str, max_chars: usize) -> String {
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

    let prefix_length = max_chars - 1;
    let prefix: String = characters[..prefix_length].iter().collect();
    format!("{prefix}…")
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::error::ErrorKind;

    #[test]
    fn default_invocation_parses_without_arguments() {
        let parsed = Cli::try_parse_from(["tmdbtag"]).unwrap();
        assert_eq!(parsed.command, None);
    }

    #[test]
    fn config_subcommand_is_owned_by_clap() {
        let parsed = Cli::try_parse_from(["tmdbtag", "config"]).unwrap();

        assert_eq!(parsed.command, Some(CliCommand::Config));
    }

    #[test]
    fn storage_catalog_commands_are_parsed_as_nested_clap_subcommands() {
        let add = Cli::try_parse_from(["tmdbtag", "storage", "add"]).unwrap();
        let remove = Cli::try_parse_from(["tmdbtag", "storage", "remove"]).unwrap();

        assert_eq!(add.command, Some(CliCommand::Storage(StorageCommand::Add)));
        assert_eq!(
            remove.command,
            Some(CliCommand::Storage(StorageCommand::Remove))
        );
    }

    #[test]
    fn help_is_owned_by_clap() {
        let error = Cli::try_parse_from(["tmdbtag", "--help"]).unwrap_err();

        assert_eq!(error.kind(), ErrorKind::DisplayHelp);
        assert!(error.to_string().contains("A polished interactive CLI"));
        assert!(error.to_string().contains("Examples:"));
        assert!(error.to_string().contains("config"));
    }

    #[test]
    fn version_is_owned_by_clap() {
        let error = Cli::try_parse_from(["tmdbtag", "--version"]).unwrap_err();

        assert_eq!(error.kind(), ErrorKind::DisplayVersion);
        assert!(error.to_string().contains(env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn invalid_arguments_return_a_clap_usage_error() {
        let error = Cli::try_parse_from(["tmdbtag", "--unknown-option"]).unwrap_err();

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
    fn terminal_text_truncation_preserves_the_filename_suffix_and_unicode_boundaries() {
        assert_eq!(
            truncate_terminal_text("a very long path/episode.mkv", 12),
            "…episode.mkv"
        );
        assert_eq!(
            truncate_terminal_text_end("a very long title", 8),
            "a very …"
        );
        assert_eq!(truncate_terminal_text("áéíóú", 4), "…íóú");
        assert_eq!(truncate_terminal_text_end("áéíóú", 4), "áéí…");
        assert_eq!(truncate_terminal_text("anything", 0), "");
        assert_eq!(truncate_terminal_text("anything", 1), "…");
    }

    #[test]
    fn raw_terminal_frames_return_to_column_zero_for_each_line() {
        let lines = vec!["Header".to_owned(), "Query: bat".to_owned()];

        assert_eq!(raw_terminal_frame(&lines), "Header\r\nQuery: bat\r\n");
    }

    #[test]
    fn live_search_state_invalidates_old_results_when_the_query_changes() {
        let mut state = LiveSearchState::default();
        for character in "bat".chars() {
            assert_eq!(
                apply_live_search_key(
                    &mut state,
                    KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
                ),
                LiveSearchAction::Continue
            );
        }

        let first = TmdbSearchCandidate {
            id: crate::domain::TmdbId::new(550).unwrap(),
            media_type: MediaType::Movie,
            title: "Fight Club".to_owned(),
            original_title: Some("Fight Club".to_owned()),
            year: Some(1999),
        };
        let second = TmdbSearchCandidate {
            id: crate::domain::TmdbId::new(414_906).unwrap(),
            media_type: MediaType::Movie,
            title: "The Batman".to_owned(),
            original_title: Some("The Batman".to_owned()),
            year: Some(2022),
        };
        state.complete_search("bat".to_owned(), vec![first, second]);

        assert!(state.selected_candidate().is_some());
        assert_eq!(
            apply_live_search_key(&mut state, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)),
            LiveSearchAction::Continue
        );
        assert_eq!(
            apply_live_search_key(
                &mut state,
                KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)
            ),
            LiveSearchAction::Accept
        );

        apply_live_search_key(
            &mut state,
            KeyEvent::new(KeyCode::Char('m'), KeyModifiers::NONE),
        );
        assert_eq!(state.query(), "batm");
        assert!(state.candidates.is_empty());
        assert!(state.selected_candidate().is_none());
        assert_eq!(state.status, LiveSearchStatus::Waiting);
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
