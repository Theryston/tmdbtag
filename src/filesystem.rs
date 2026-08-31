//! Filesystem discovery, plan validation, and safe execution for the interactive workflow.
//!
//! Discovery and plan construction remain non-mutating. Once the application has displayed and
//! confirmed an immutable plan, this module owns the destination commit, no-replace publication,
//! source-preserving copies, cross-volume copy verification, source removal for moves, and
//! per-file execution reports.

use std::{
    cmp::Ordering,
    env,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering as AtomicOrdering},
};

use crate::{
    domain::{
        DestinationSelection, ExecutionReport, FileOperation, FileSnapshot, OperationPlan,
        OperationResult, OperationStatus, PlannedOperation, SourceFolder, SourceRoot,
        VideoExtension, VideoFile,
    },
    error::FilesystemError,
};

/// Backward-compatible access to the shared video-extension policy.
pub use crate::domain::VIDEO_EXTENSIONS;

static NEXT_TEMP_FILE_ID: AtomicU64 = AtomicU64::new(1);

/// The result of a deterministic directory scan and the non-fatal entries skipped during it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discovery<T> {
    items: Vec<T>,
    warnings: Vec<DiscoveryWarning>,
}

impl<T> Discovery<T> {
    /// Creates a discovery result from eligible values and displayable warnings.
    pub fn new(items: Vec<T>, warnings: Vec<DiscoveryWarning>) -> Self {
        Self { items, warnings }
    }

    /// Returns eligible entries in deterministic display order.
    pub fn items(&self) -> &[T] {
        &self.items
    }

    /// Returns entries that were skipped or could not be fully inspected.
    pub fn warnings(&self) -> &[DiscoveryWarning] {
        &self.warnings
    }

    /// Splits the discovery result into owned entries and warnings.
    pub fn into_parts(self) -> (Vec<T>, Vec<DiscoveryWarning>) {
        (self.items, self.warnings)
    }
}

/// A non-fatal discovery condition associated with one filesystem path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryWarning {
    path: PathBuf,
    reason: String,
}

/// Aggregate byte progress for one confirmed file operation plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransferProgress {
    operation_index: usize,
    operation_count: usize,
    completed_bytes: u64,
    total_bytes: u64,
    current_file_bytes: u64,
    current_file_total: u64,
}

impl TransferProgress {
    fn new(
        operation_index: usize,
        operation_count: usize,
        completed_bytes: u64,
        total_bytes: u64,
        current_file_bytes: u64,
        current_file_total: u64,
    ) -> Self {
        Self {
            operation_index,
            operation_count,
            completed_bytes,
            total_bytes,
            current_file_bytes,
            current_file_total,
        }
    }

    /// Returns the zero-based index of the file currently being processed.
    pub const fn operation_index(self) -> usize {
        self.operation_index
    }

    /// Returns the number of files in the plan.
    pub const fn operation_count(self) -> usize {
        self.operation_count
    }

    /// Returns the aggregate bytes copied or logically moved so far.
    pub const fn completed_bytes(self) -> u64 {
        self.completed_bytes
    }

    /// Returns the aggregate bytes represented by the complete plan.
    pub const fn total_bytes(self) -> u64 {
        self.total_bytes
    }

    /// Returns the bytes transferred for the current file.
    pub const fn current_file_bytes(self) -> u64 {
        self.current_file_bytes
    }

    /// Returns the planned size of the current file.
    pub const fn current_file_total(self) -> u64 {
        self.current_file_total
    }
}

impl DiscoveryWarning {
    /// Creates a warning without changing the path that was observed.
    pub fn new(path: PathBuf, reason: impl Into<String>) -> Self {
        Self {
            path,
            reason: reason.into(),
        }
    }

    /// Returns the affected path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns the safe, human-readable reason selected by the discovery layer.
    pub fn reason(&self) -> &str {
        &self.reason
    }
}

/// Resolves and validates the process current working directory as the source root.
pub fn current_source_root() -> Result<SourceRoot, FilesystemError> {
    let path = env::current_dir().map_err(|source| FilesystemError::CurrentDirectory { source })?;
    let metadata = fs::metadata(&path).map_err(|source| FilesystemError::SourceRootMetadata {
        path: path.clone(),
        source,
    })?;
    if !metadata.is_dir() {
        return Err(FilesystemError::SourceRootNotDirectory { path });
    }

    fs::read_dir(&path).map_err(|source| FilesystemError::ReadDirectory {
        path: path.clone(),
        source,
    })?;

    Ok(SourceRoot::new(normalize_path(&path)))
}

/// Resolves a user-entered destination without creating it.
pub fn resolve_destination(
    source_root: &SourceRoot,
    input: &str,
) -> Result<DestinationSelection, FilesystemError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(FilesystemError::EmptyDestination);
    }
    if trimmed.chars().any(|character| character == '\0') {
        return Err(FilesystemError::InvalidDestination {
            input: sanitize_input_for_error(trimmed),
            reason: "it contains a null character".to_owned(),
        });
    }

    let entered_path = Path::new(trimmed);
    let candidate = if entered_path.is_absolute() {
        entered_path.to_owned()
    } else {
        source_root.path().join(entered_path)
    };
    let path = normalize_path(&candidate);

    // Check the lexical root first so `.` is reported as the forbidden source root even when a
    // platform represents the current directory through a link. All other existing components
    // must be real filesystem entries; otherwise a typed path could silently resolve elsewhere.
    if normalize_path(&path) == normalize_path(source_root.path()) {
        return Err(FilesystemError::DestinationIsSourceRoot { path });
    }
    reject_symlink_components(&path)?;

    match fs::symlink_metadata(&path) {
        Ok(metadata) => {
            let file_type = metadata.file_type();
            if file_type.is_symlink() {
                return Err(FilesystemError::DestinationSymlink { path });
            }
            if paths_equivalent(&path, source_root.path()) {
                return Err(FilesystemError::DestinationIsSourceRoot { path });
            }
            if metadata.is_file() {
                return Err(FilesystemError::DestinationIsFile { path });
            }
            if !metadata.is_dir() {
                return Err(FilesystemError::DestinationUnsupportedType { path });
            }

            Ok(DestinationSelection::new(path, true, false))
        }
        Err(source)
            if matches!(
                source.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            validate_missing_destination_parent(&path)?;
            Ok(DestinationSelection::new(path, false, true))
        }
        Err(source) => Err(FilesystemError::DestinationMetadata { path, source }),
    }
}

/// Ensures that a destination does not overlap any source folder selected by the user.
pub fn validate_destination_against_sources(
    destination: &DestinationSelection,
    folders: &[SourceFolder],
) -> Result<(), FilesystemError> {
    for folder in folders {
        if paths_equivalent(destination.path(), folder.path())
            || path_is_same_or_descendant(destination.path(), folder.path())
        {
            return Err(FilesystemError::DestinationIsSelectedSource {
                path: destination.path().to_owned(),
            });
        }
    }

    Ok(())
}

/// Discovers direct, non-symbolic-link child directories of the source root.
pub fn discover_source_folders(
    source_root: &SourceRoot,
    destination: &DestinationSelection,
) -> Result<Discovery<SourceFolder>, FilesystemError> {
    let entries =
        fs::read_dir(source_root.path()).map_err(|source| FilesystemError::ReadDirectory {
            path: source_root.path().to_owned(),
            source,
        })?;

    let mut folders = Vec::new();
    let mut warnings = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                warnings.push(DiscoveryWarning::new(
                    source_root.path().to_owned(),
                    format!("could not read a directory entry: {error}"),
                ));
                continue;
            }
        };

        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(error) => {
                warnings.push(DiscoveryWarning::new(
                    path,
                    format!("could not inspect the directory entry: {error}"),
                ));
                continue;
            }
        };

        if file_type.is_symlink() {
            warnings.push(DiscoveryWarning::new(
                path,
                "symbolic links are not eligible source folders",
            ));
            continue;
        }
        if !file_type.is_dir() {
            continue;
        }

        // Exclude the destination itself and any direct source folder that contains a nested
        // destination. Selecting such a folder would make the source and destination overlap.
        if paths_equivalent(&path, destination.path())
            || path_is_same_or_descendant(destination.path(), &path)
        {
            continue;
        }

        folders.push(SourceFolder::new(path));
    }

    folders.sort_by(|left, right| compare_paths(left.path(), right.path()));
    warnings.sort_by(|left, right| {
        compare_paths(left.path(), right.path()).then_with(|| left.reason().cmp(right.reason()))
    });
    Ok(Discovery::new(folders, warnings))
}

/// Recursively discovers regular video files inside one source folder.
///
/// Real subdirectories are traversed in the MVP, but symbolic links are never followed. A
/// failure to read the selected source folder is fatal; a failure in a nested subfolder is
/// reported as a warning so the user can still select files found elsewhere in the tree.
pub fn discover_video_files(
    source_folder: &SourceFolder,
) -> Result<Discovery<VideoFile>, FilesystemError> {
    let mut files = Vec::new();
    let mut warnings = Vec::new();
    discover_video_files_in_directory(source_folder.path(), true, None, &mut files, &mut warnings)?;

    sort_video_discovery(&mut files, &mut warnings);
    Ok(Discovery::new(files, warnings))
}

/// Recursively discovers every regular video file below the current source root.
///
/// Unlike [`discover_video_files`], this scan includes videos directly in the source root and
/// excludes the chosen destination subtree at any depth. The returned files are flat and sorted;
/// the interactive UI is responsible for presenting them as an expandable tree.
pub fn discover_video_files_in_source_root(
    source_root: &SourceRoot,
    destination: &DestinationSelection,
) -> Result<Discovery<VideoFile>, FilesystemError> {
    let mut files = Vec::new();
    let mut warnings = Vec::new();
    discover_video_files_in_directory(
        source_root.path(),
        true,
        Some(destination.path()),
        &mut files,
        &mut warnings,
    )?;

    sort_video_discovery(&mut files, &mut warnings);
    Ok(Discovery::new(files, warnings))
}

fn discover_video_files_in_directory(
    directory: &Path,
    is_root: bool,
    excluded_directory: Option<&Path>,
    files: &mut Vec<VideoFile>,
    warnings: &mut Vec<DiscoveryWarning>,
) -> Result<(), FilesystemError> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(source) if is_root => {
            return Err(FilesystemError::ReadDirectory {
                path: directory.to_owned(),
                source,
            });
        }
        Err(source) => {
            warnings.push(DiscoveryWarning::new(
                directory.to_owned(),
                format!("could not read nested directory: {source}"),
            ));
            return Ok(());
        }
    };

    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                warnings.push(DiscoveryWarning::new(
                    directory.to_owned(),
                    format!("could not read a directory entry: {error}"),
                ));
                continue;
            }
        };

        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(error) => {
                warnings.push(DiscoveryWarning::new(
                    path,
                    format!("could not inspect the directory entry: {error}"),
                ));
                continue;
            }
        };

        if file_type.is_symlink() {
            warnings.push(DiscoveryWarning::new(
                path,
                "symbolic links are not eligible video files",
            ));
            continue;
        }
        if file_type.is_dir() {
            if excluded_directory.is_some_and(|excluded| {
                paths_equivalent(&path, excluded) || path_is_same_or_descendant(&path, excluded)
            }) {
                continue;
            }
            discover_video_files_in_directory(&path, false, excluded_directory, files, warnings)?;
            continue;
        }
        if !file_type.is_file() || !has_video_extension(&path) {
            continue;
        }

        let size_bytes = match entry.metadata() {
            Ok(metadata) if metadata.is_file() => Some(metadata.len()),
            Ok(_) => {
                warnings.push(DiscoveryWarning::new(
                    path,
                    "the entry changed and is no longer a regular file",
                ));
                continue;
            }
            Err(error) => {
                warnings.push(DiscoveryWarning::new(
                    path.clone(),
                    format!("could not read file metadata: {error}"),
                ));
                None
            }
        };

        files.push(VideoFile::new(path, size_bytes));
    }

    Ok(())
}

fn sort_video_discovery(files: &mut [VideoFile], warnings: &mut [DiscoveryWarning]) {
    files.sort_by(|left, right| compare_paths(left.path(), right.path()));
    warnings.sort_by(|left, right| {
        compare_paths(left.path(), right.path()).then_with(|| left.reason().cmp(right.reason()))
    });
}

/// Returns whether a path has one of the recognized video filename extensions.
pub fn has_video_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| VideoExtension::parse(extension).is_ok())
}

/// Reads and validates the recognized video extension of a selected regular file.
pub fn source_video_extension(path: &Path) -> Result<VideoExtension, FilesystemError> {
    let metadata = read_source_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Err(FilesystemError::SourceSymlink {
            path: path.to_owned(),
        });
    }
    if !metadata.is_file() {
        return Err(FilesystemError::SourceNotRegularFile {
            path: path.to_owned(),
        });
    }

    let Some(extension) = path.extension().and_then(|extension| extension.to_str()) else {
        return Err(FilesystemError::SourceUnsupportedExtension {
            path: path.to_owned(),
        });
    };
    VideoExtension::parse(extension).map_err(|_| FilesystemError::SourceUnsupportedExtension {
        path: path.to_owned(),
    })
}

/// Captures the portable source-file identity used by plan revalidation.
pub fn snapshot_source_file(path: &Path) -> Result<FileSnapshot, FilesystemError> {
    let metadata = read_source_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Err(FilesystemError::SourceSymlink {
            path: path.to_owned(),
        });
    }
    if !metadata.is_file() {
        return Err(FilesystemError::SourceNotRegularFile {
            path: path.to_owned(),
        });
    }

    Ok(FileSnapshot::new(metadata.len(), metadata.modified().ok()))
}

fn read_source_metadata(path: &Path) -> Result<fs::Metadata, FilesystemError> {
    fs::symlink_metadata(path).map_err(|cause| {
        if cause.kind() == io::ErrorKind::NotFound {
            FilesystemError::SourceNotFound {
                path: path.to_owned(),
            }
        } else {
            FilesystemError::SourceMetadata {
                path: path.to_owned(),
                cause,
            }
        }
    })
}

/// Validates every operation without changing the filesystem.
pub fn validate_operation_plan(plan: &OperationPlan) -> Result<(), FilesystemError> {
    if plan.operations().is_empty() {
        return Err(FilesystemError::EmptyOperationPlan);
    }

    validate_plan_destination(plan)?;

    let mut source_paths = Vec::new();
    let mut destination_paths = Vec::new();
    for operation in plan.operations() {
        validate_source_folder(operation.source_folder())?;
        if !path_is_same_or_descendant(operation.source_path(), operation.source_folder())
            || paths_equivalent(operation.source_path(), operation.source_folder())
        {
            return Err(FilesystemError::SourceFolderMismatch {
                source_path: operation.source_path().to_owned(),
                folder: operation.source_folder().to_owned(),
            });
        }

        if source_paths
            .iter()
            .any(|path: &PathBuf| paths_equivalent(path, operation.source_path()))
        {
            return Err(FilesystemError::DuplicateSource {
                path: operation.source_path().to_owned(),
            });
        }
        source_paths.push(operation.source_path().to_owned());

        validate_source_operation(operation)?;
        validate_destination_path(plan, operation)?;

        if destination_paths
            .iter()
            .any(|path: &PathBuf| paths_equivalent(path, operation.destination_path()))
        {
            return Err(FilesystemError::DuplicateDestination {
                path: operation.destination_path().to_owned(),
            });
        }
        destination_paths.push(operation.destination_path().to_owned());
    }

    Ok(())
}

/// Executes a previously validated plan and returns one result for every planned file.
pub fn execute_operation_plan(plan: &OperationPlan) -> Result<ExecutionReport, FilesystemError> {
    execute_operation_plan_with_progress(plan, |_| {})
}

/// Executes a plan while reporting aggregate byte-based transfer progress.
///
/// The callback is presentation-only: it cannot change the plan or authorize an operation. The
/// filesystem layer still performs all validation and owns every mutation. Copy operations always
/// transfer bytes into a destination-side temporary file; move operations report logical byte
/// completion for same-volume no-replace moves and byte progress for cross-volume fallbacks.
pub fn execute_operation_plan_with_progress<F>(
    plan: &OperationPlan,
    on_progress: F,
) -> Result<ExecutionReport, FilesystemError>
where
    F: FnMut(TransferProgress),
{
    validate_operation_plan(plan)?;
    Ok(execute_validated_operation_plan(
        plan,
        MoveStrategy::Automatic,
        on_progress,
    ))
}

fn validate_plan_destination(plan: &OperationPlan) -> Result<(), FilesystemError> {
    let destination = plan.destination();
    let destination_path = destination.path();
    let current_state = fs::symlink_metadata(destination_path);

    match (destination.exists(), current_state) {
        (true, Ok(metadata)) => {
            if metadata.file_type().is_symlink() {
                return Err(FilesystemError::DestinationSymlink {
                    path: destination_path.to_owned(),
                });
            }
            if !metadata.is_dir() {
                return Err(FilesystemError::DestinationUnsupportedType {
                    path: destination_path.to_owned(),
                });
            }
            ensure_destination_writable(destination_path, &metadata)?;
        }
        (true, Err(cause)) if cause.kind() == io::ErrorKind::NotFound => {
            return Err(FilesystemError::DestinationStateChanged {
                path: destination_path.to_owned(),
            });
        }
        (true, Err(cause)) => {
            return Err(FilesystemError::DestinationMetadata {
                path: destination_path.to_owned(),
                source: cause,
            });
        }
        (false, Ok(_)) => {
            return Err(FilesystemError::DestinationStateChanged {
                path: destination_path.to_owned(),
            });
        }
        (false, Err(cause)) if cause.kind() == io::ErrorKind::NotFound => {
            if !destination.may_create_after_confirmation() {
                return Err(FilesystemError::DestinationCreationNotAllowed {
                    path: destination_path.to_owned(),
                });
            }
            validate_missing_destination_parent(destination_path)?;
        }
        (false, Err(cause)) => {
            return Err(FilesystemError::DestinationMetadata {
                path: destination_path.to_owned(),
                source: cause,
            });
        }
    }

    let mut source_folders = Vec::new();
    for operation in plan.operations() {
        if source_folders
            .iter()
            .any(|folder: &PathBuf| paths_equivalent(folder, operation.source_folder()))
        {
            continue;
        }
        let source_folder_is_root =
            paths_equivalent(operation.source_folder(), plan.source_root().path());
        // A root-level source video is grouped under the source root for plan validation. A
        // destination child is safe because discovery excludes the entire destination subtree;
        // nested source containers still reject an overlapping destination.
        if paths_equivalent(destination_path, operation.source_folder())
            || (!source_folder_is_root
                && path_is_same_or_descendant(destination_path, operation.source_folder()))
        {
            return Err(FilesystemError::DestinationIsSelectedSource {
                path: destination_path.to_owned(),
            });
        }
        source_folders.push(operation.source_folder().to_owned());
    }

    if paths_equivalent(destination_path, plan.source_root().path()) {
        return Err(FilesystemError::DestinationIsSourceRoot {
            path: destination_path.to_owned(),
        });
    }

    Ok(())
}

fn validate_source_folder(path: &Path) -> Result<(), FilesystemError> {
    let metadata = fs::symlink_metadata(path).map_err(|cause| {
        if cause.kind() == io::ErrorKind::NotFound {
            FilesystemError::SourceFolderNotFound {
                path: path.to_owned(),
            }
        } else {
            FilesystemError::SourceMetadata {
                path: path.to_owned(),
                cause,
            }
        }
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(FilesystemError::SourceFolderInvalid {
            path: path.to_owned(),
        });
    }
    Ok(())
}

fn validate_source_operation(operation: &PlannedOperation) -> Result<(), FilesystemError> {
    let extension = source_video_extension(operation.source_path())?;
    if extension != *operation.source_extension() {
        return Err(FilesystemError::SourceExtensionChanged {
            path: operation.source_path().to_owned(),
            expected: operation.source_extension().to_string(),
            actual: extension.to_string(),
        });
    }

    let snapshot = snapshot_source_file(operation.source_path())?;
    if snapshot != *operation.source_snapshot() {
        return Err(FilesystemError::SourceChanged {
            path: operation.source_path().to_owned(),
        });
    }

    Ok(())
}

fn validate_destination_path(
    plan: &OperationPlan,
    operation: &PlannedOperation,
) -> Result<(), FilesystemError> {
    let destination = plan.destination().path();
    let destination_path = operation.destination_path();
    let Some(parent) = destination_path.parent() else {
        return Err(FilesystemError::DestinationEscapes {
            path: destination_path.to_owned(),
        });
    };
    if !paths_equivalent(parent, destination)
        || destination_path.file_name().and_then(|name| name.to_str())
            != Some(operation.normalized_filename())
    {
        return Err(FilesystemError::DestinationEscapes {
            path: destination_path.to_owned(),
        });
    }
    if paths_equivalent(operation.source_path(), destination_path) {
        return Err(FilesystemError::SourceDestinationSame {
            path: destination_path.to_owned(),
        });
    }

    match fs::symlink_metadata(destination_path) {
        Ok(_) => Err(FilesystemError::DestinationAlreadyExists {
            path: destination_path.to_owned(),
        }),
        Err(cause) if cause.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(cause) => Err(FilesystemError::DestinationMetadata {
            path: destination_path.to_owned(),
            source: cause,
        }),
    }
}

fn ensure_destination_writable(
    destination: &Path,
    metadata: &fs::Metadata,
) -> Result<(), FilesystemError> {
    if metadata.permissions().readonly() {
        return Err(FilesystemError::DestinationNotWritable {
            path: destination.to_owned(),
        });
    }
    Ok(())
}

fn execute_validated_operation_plan<F>(
    plan: &OperationPlan,
    strategy: MoveStrategy,
    mut on_progress: F,
) -> ExecutionReport
where
    F: FnMut(TransferProgress),
{
    let mut statuses = vec![OperationStatus::Pending; plan.operation_count()];
    if let Err(error) = ensure_destination_at_commit(plan) {
        statuses[0] = OperationStatus::Failed {
            reason: error.to_string(),
        };
        return report_for_statuses(plan, statuses);
    }

    let total_bytes = plan.total_size_bytes();
    let mut completed_bytes = 0_u64;

    for (index, operation) in plan.operations().iter().enumerate() {
        let current_file_total = operation.source_snapshot().size_bytes();
        on_progress(TransferProgress::new(
            index,
            plan.operation_count(),
            completed_bytes,
            total_bytes,
            0,
            current_file_total,
        ));

        if let Err(error) = validate_operation_at_commit(plan, operation) {
            statuses[index] = OperationStatus::Failed {
                reason: error.to_string(),
            };
            break;
        }

        let execution_result = {
            let mut transfer = TransferContext::new(
                index,
                plan.operation_count(),
                completed_bytes,
                total_bytes,
                &mut on_progress,
            );
            execute_operation(operation, plan.operation(), strategy, &mut transfer)
        };

        match execution_result {
            Ok(()) => {
                completed_bytes = completed_bytes.saturating_add(current_file_total);
                on_progress(TransferProgress::new(
                    index,
                    plan.operation_count(),
                    completed_bytes,
                    total_bytes,
                    current_file_total,
                    current_file_total,
                ));
                statuses[index] = OperationStatus::Completed;
            }
            Err(error) => {
                statuses[index] = OperationStatus::Failed {
                    reason: error.to_string(),
                };
                break;
            }
        }
    }

    report_for_statuses(plan, statuses)
}

fn report_for_statuses(plan: &OperationPlan, statuses: Vec<OperationStatus>) -> ExecutionReport {
    let results = plan
        .operations()
        .iter()
        .zip(statuses)
        .map(|(operation, status)| {
            OperationResult::new(
                operation.source_path().to_owned(),
                operation.destination_path().to_owned(),
                status,
            )
        })
        .collect();
    ExecutionReport::new(plan.source_root().clone(), plan.operation(), results)
}

fn ensure_destination_at_commit(plan: &OperationPlan) -> Result<(), FilesystemError> {
    let destination = plan.destination();
    match fs::symlink_metadata(destination.path()) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                return Err(FilesystemError::DestinationSymlink {
                    path: destination.path().to_owned(),
                });
            }
            if !metadata.is_dir() {
                return Err(FilesystemError::DestinationUnsupportedType {
                    path: destination.path().to_owned(),
                });
            }
            ensure_destination_writable(destination.path(), &metadata)
        }
        Err(cause) if cause.kind() == io::ErrorKind::NotFound => {
            if !destination.may_create_after_confirmation() {
                return Err(FilesystemError::DestinationCreationNotAllowed {
                    path: destination.path().to_owned(),
                });
            }
            validate_missing_destination_parent(destination.path())?;
            fs::create_dir(destination.path()).map_err(|cause| {
                FilesystemError::DestinationCreation {
                    path: destination.path().to_owned(),
                    cause,
                }
            })?;
            let metadata = fs::symlink_metadata(destination.path()).map_err(|cause| {
                FilesystemError::DestinationCreation {
                    path: destination.path().to_owned(),
                    cause,
                }
            })?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(FilesystemError::DestinationUnsupportedType {
                    path: destination.path().to_owned(),
                });
            }
            ensure_destination_writable(destination.path(), &metadata)
        }
        Err(cause) => Err(FilesystemError::DestinationMetadata {
            path: destination.path().to_owned(),
            source: cause,
        }),
    }
}

fn validate_operation_at_commit(
    plan: &OperationPlan,
    operation: &PlannedOperation,
) -> Result<(), FilesystemError> {
    validate_source_folder(operation.source_folder())?;
    validate_source_operation(operation)?;
    validate_destination_path(plan, operation)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MoveStrategy {
    Automatic,
    #[cfg(test)]
    SameVolume,
    #[cfg(test)]
    CrossVolume,
}

struct TransferContext<'a> {
    operation_index: usize,
    operation_count: usize,
    completed_bytes: u64,
    total_bytes: u64,
    on_progress: &'a mut dyn FnMut(TransferProgress),
}

impl<'a> TransferContext<'a> {
    fn new(
        operation_index: usize,
        operation_count: usize,
        completed_bytes: u64,
        total_bytes: u64,
        on_progress: &'a mut dyn FnMut(TransferProgress),
    ) -> Self {
        Self {
            operation_index,
            operation_count,
            completed_bytes,
            total_bytes,
            on_progress,
        }
    }

    fn report(&mut self, current_file_bytes: u64, current_file_total: u64) {
        let current_file_bytes = current_file_bytes.min(current_file_total);
        let completed_bytes = self
            .completed_bytes
            .saturating_add(current_file_bytes)
            .min(self.total_bytes);
        (self.on_progress)(TransferProgress::new(
            self.operation_index,
            self.operation_count,
            completed_bytes,
            self.total_bytes,
            current_file_bytes,
            current_file_total,
        ));
    }
}

fn execute_operation(
    operation: &PlannedOperation,
    file_operation: FileOperation,
    strategy: MoveStrategy,
    transfer: &mut TransferContext<'_>,
) -> Result<(), FilesystemError> {
    match file_operation {
        FileOperation::Copy => copy_with_temporary_publish(operation, false, transfer),
        FileOperation::Move => match strategy {
            MoveStrategy::Automatic => match move_same_volume(operation) {
                Ok(()) => Ok(()),
                Err(error) if is_cross_device_error(&error) => {
                    copy_with_temporary_publish(operation, true, transfer)
                }
                Err(error) => Err(error),
            },
            #[cfg(test)]
            MoveStrategy::SameVolume => move_same_volume(operation),
            #[cfg(test)]
            MoveStrategy::CrossVolume => copy_with_temporary_publish(operation, true, transfer),
        },
    }
}

fn move_same_volume(operation: &PlannedOperation) -> Result<(), FilesystemError> {
    match fs::hard_link(operation.source_path(), operation.destination_path()) {
        Ok(()) => {
            validate_source_operation(operation)?;
            fs::remove_file(operation.source_path()).map_err(|cause| {
                FilesystemError::SourceRemoval {
                    path: operation.source_path().to_owned(),
                    cause,
                }
            })
        }
        Err(cause) if cause.kind() == io::ErrorKind::AlreadyExists => {
            Err(FilesystemError::DestinationAlreadyExists {
                path: operation.destination_path().to_owned(),
            })
        }
        Err(cause) => Err(FilesystemError::SameVolumeMove {
            source_path: operation.source_path().to_owned(),
            destination: operation.destination_path().to_owned(),
            cause,
        }),
    }
}

fn copy_with_temporary_publish(
    operation: &PlannedOperation,
    remove_source: bool,
    transfer: &mut TransferContext<'_>,
) -> Result<(), FilesystemError> {
    let destination_directory = operation.destination_path().parent().ok_or_else(|| {
        FilesystemError::DestinationEscapes {
            path: operation.destination_path().to_owned(),
        }
    })?;
    let (temporary_path, temporary_file) =
        create_temporary_file(destination_directory).map_err(|cause| {
            FilesystemError::CrossVolumeCopy {
                source_path: operation.source_path().to_owned(),
                destination: operation.destination_path().to_owned(),
                reason: format!("could not create a destination-side temporary file: {cause}"),
            }
        })?;

    let copy_result = copy_and_publish(
        operation,
        &temporary_path,
        temporary_file,
        remove_source,
        transfer,
    );

    if copy_result.is_err()
        && let Err(cleanup_cause) = fs::remove_file(&temporary_path)
        && cleanup_cause.kind() != io::ErrorKind::NotFound
    {
        return Err(FilesystemError::TemporaryCleanup {
            path: temporary_path,
            cause: cleanup_cause,
        });
    }

    copy_result
}

fn copy_and_publish(
    operation: &PlannedOperation,
    temporary_path: &Path,
    mut temporary_file: File,
    remove_source: bool,
    transfer: &mut TransferContext<'_>,
) -> Result<(), FilesystemError> {
    let mut source_file =
        File::open(operation.source_path()).map_err(|cause| FilesystemError::CrossVolumeCopy {
            source_path: operation.source_path().to_owned(),
            destination: operation.destination_path().to_owned(),
            reason: format!("could not open the source: {cause}"),
        })?;
    let source_size = operation.source_snapshot().size_bytes();
    let mut copied_bytes = 0_u64;
    let mut buffer = [0_u8; 1024 * 1024];
    loop {
        let bytes_read =
            source_file
                .read(&mut buffer)
                .map_err(|cause| FilesystemError::CrossVolumeCopy {
                    source_path: operation.source_path().to_owned(),
                    destination: operation.destination_path().to_owned(),
                    reason: format!("copy failed while reading the source: {cause}"),
                })?;
        if bytes_read == 0 {
            break;
        }

        temporary_file
            .write_all(&buffer[..bytes_read])
            .map_err(|cause| FilesystemError::CrossVolumeCopy {
                source_path: operation.source_path().to_owned(),
                destination: operation.destination_path().to_owned(),
                reason: format!("copy failed while writing the destination: {cause}"),
            })?;
        copied_bytes = copied_bytes.saturating_add(bytes_read as u64);
        transfer.report(copied_bytes, source_size);
    }
    temporary_file
        .flush()
        .map_err(|cause| FilesystemError::CrossVolumeCopy {
            source_path: operation.source_path().to_owned(),
            destination: operation.destination_path().to_owned(),
            reason: format!("could not flush the temporary copy: {cause}"),
        })?;
    temporary_file
        .sync_all()
        .map_err(|cause| FilesystemError::CrossVolumeCopy {
            source_path: operation.source_path().to_owned(),
            destination: operation.destination_path().to_owned(),
            reason: format!("could not synchronize the temporary copy: {cause}"),
        })?;
    drop(source_file);
    drop(temporary_file);

    let temporary_metadata =
        fs::metadata(temporary_path).map_err(|cause| FilesystemError::CopyVerification {
            source_path: operation.source_path().to_owned(),
            destination: operation.destination_path().to_owned(),
            reason: format!("could not inspect the temporary copy: {cause}"),
        })?;
    if copied_bytes != operation.source_snapshot().size_bytes()
        || temporary_metadata.len() != operation.source_snapshot().size_bytes()
    {
        return Err(FilesystemError::CopyVerification {
            source_path: operation.source_path().to_owned(),
            destination: operation.destination_path().to_owned(),
            reason: "the copied byte count does not match the planned source size".to_owned(),
        });
    }
    if let Err(error) = validate_source_operation(operation) {
        return Err(FilesystemError::CopyVerification {
            source_path: operation.source_path().to_owned(),
            destination: operation.destination_path().to_owned(),
            reason: format!("the source changed while it was being copied: {error}"),
        });
    }

    match fs::hard_link(temporary_path, operation.destination_path()) {
        Ok(()) => {}
        Err(cause) if cause.kind() == io::ErrorKind::AlreadyExists => {
            return Err(FilesystemError::DestinationAlreadyExists {
                path: operation.destination_path().to_owned(),
            });
        }
        Err(cause) => {
            return Err(FilesystemError::DestinationPublication {
                path: operation.destination_path().to_owned(),
                cause,
            });
        }
    }

    fs::remove_file(temporary_path).map_err(|cause| FilesystemError::TemporaryCleanup {
        path: temporary_path.to_owned(),
        cause,
    })?;
    if remove_source {
        fs::remove_file(operation.source_path()).map_err(|cause| {
            FilesystemError::SourceRemoval {
                path: operation.source_path().to_owned(),
                cause,
            }
        })?;
    }
    Ok(())
}

fn create_temporary_file(directory: &Path) -> io::Result<(PathBuf, File)> {
    for attempt in 0..64_u32 {
        let id = NEXT_TEMP_FILE_ID.fetch_add(1, AtomicOrdering::Relaxed);
        let filename = format!(".tmdbtag.{}.{}.{}.tmp", std::process::id(), id, attempt);
        let path = directory.join(filename);
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }

    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not allocate a unique temporary filename",
    ))
}

fn is_cross_device_error(error: &FilesystemError) -> bool {
    match error {
        FilesystemError::SameVolumeMove { cause, .. } => {
            cause.kind() == io::ErrorKind::CrossesDevices || {
                #[cfg(unix)]
                {
                    cause.raw_os_error() == Some(18)
                }
                #[cfg(windows)]
                {
                    cause.raw_os_error() == Some(17)
                }
                #[cfg(not(any(unix, windows)))]
                {
                    false
                }
            }
        }
        _ => false,
    }
}

fn validate_missing_destination_parent(path: &Path) -> Result<(), FilesystemError> {
    let mut current = path
        .parent()
        .ok_or_else(|| FilesystemError::InvalidDestination {
            input: sanitize_input_for_error(&path.to_string_lossy()),
            reason: "it has no usable parent directory".to_owned(),
        })?;

    loop {
        match fs::symlink_metadata(current) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() {
                    return Err(FilesystemError::DestinationSymlink {
                        path: current.to_owned(),
                    });
                }
                if !metadata.is_dir() {
                    return Err(FilesystemError::DestinationParentNotDirectory {
                        path: current.to_owned(),
                    });
                }
                return Ok(());
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let Some(parent) = current.parent() else {
                    return Err(FilesystemError::InvalidDestination {
                        input: sanitize_input_for_error(&path.to_string_lossy()),
                        reason: "no existing parent directory could be found".to_owned(),
                    });
                };
                if parent == current {
                    return Err(FilesystemError::InvalidDestination {
                        input: sanitize_input_for_error(&path.to_string_lossy()),
                        reason: "no existing parent directory could be found".to_owned(),
                    });
                }
                current = parent;
            }
            Err(source) => {
                return Err(FilesystemError::DestinationMetadata {
                    path: current.to_owned(),
                    source,
                });
            }
        }
    }
}

fn paths_equivalent(left: &Path, right: &Path) -> bool {
    if normalize_path(left) == normalize_path(right) {
        return true;
    }

    match (fs::canonicalize(left), fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn reject_symlink_components(path: &Path) -> Result<(), FilesystemError> {
    let ancestors: Vec<PathBuf> = path.ancestors().map(Path::to_path_buf).collect();
    for ancestor in ancestors.iter().rev() {
        if ancestor.as_os_str().is_empty() {
            continue;
        }

        match fs::symlink_metadata(ancestor) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(FilesystemError::DestinationSymlink {
                    path: ancestor.clone(),
                });
            }
            Ok(_) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                ) =>
            {
                break;
            }
            Err(source) => {
                return Err(FilesystemError::DestinationMetadata {
                    path: ancestor.clone(),
                    source,
                });
            }
        }
    }

    Ok(())
}

fn path_is_same_or_descendant(path: &Path, ancestor: &Path) -> bool {
    let path = normalize_path(path);
    let ancestor = normalize_path(ancestor);
    let mut path_components = path.components();
    ancestor
        .components()
        .all(|component| path_components.next() == Some(component))
}

/// Removes `.` and safely resolves lexical `..` components without touching the filesystem.
fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();

    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() && !path.is_absolute() {
                    normalized.push(component.as_os_str());
                }
            }
            Component::Normal(value) => normalized.push(value),
        }
    }

    normalized
}

fn compare_paths(left: &Path, right: &Path) -> Ordering {
    path_sort_key(left)
        .cmp(&path_sort_key(right))
        .then_with(|| left.cmp(right))
}

fn path_sort_key(path: &Path) -> String {
    path.to_string_lossy().to_lowercase()
}

fn sanitize_input_for_error(value: &str) -> String {
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
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    fn tmdb_movie(title: &str) -> crate::domain::TmdbItem {
        crate::domain::TmdbItem {
            id: crate::domain::TmdbId::new(550).unwrap(),
            media_type: crate::domain::MediaType::Movie,
            title: title.to_owned(),
            original_title: Some(title.to_owned()),
            year: Some(1999),
        }
    }

    fn planned_operation(
        source_folder: &Path,
        source_path: &Path,
        destination: &Path,
        filename: &str,
    ) -> PlannedOperation {
        PlannedOperation::new(
            source_folder.to_owned(),
            source_path.to_owned(),
            destination.join(filename),
            filename.to_owned(),
            tmdb_movie(filename),
            None,
            source_video_extension(source_path).unwrap(),
            snapshot_source_file(source_path).unwrap(),
        )
    }

    fn operation_plan(
        source_root_path: &Path,
        destination: &Path,
        destination_exists: bool,
        operations: Vec<PlannedOperation>,
    ) -> OperationPlan {
        operation_plan_with_mode(
            source_root_path,
            destination,
            destination_exists,
            FileOperation::Move,
            operations,
        )
    }

    fn operation_plan_with_mode(
        source_root_path: &Path,
        destination: &Path,
        destination_exists: bool,
        mode: FileOperation,
        operations: Vec<PlannedOperation>,
    ) -> OperationPlan {
        OperationPlan::new(
            SourceRoot::new(source_root_path.to_owned()),
            DestinationSelection::new(
                destination.to_owned(),
                destination_exists,
                !destination_exists,
            ),
            mode,
            operations,
        )
    }

    fn source_root(path: &Path) -> SourceRoot {
        SourceRoot::new(path.to_path_buf())
    }

    #[test]
    fn current_source_root_uses_an_absolute_readable_directory_without_changing_cwd() {
        let before = env::current_dir().unwrap();

        let root = current_source_root().unwrap();

        assert!(root.path().is_absolute());
        assert!(root.path().is_dir());
        assert_eq!(env::current_dir().unwrap(), before);
    }

    #[test]
    fn a_regular_file_cannot_be_used_as_a_source_root() {
        let directory = tempdir().unwrap();
        let file = directory.path().join("not-a-directory");
        fs::write(&file, "content").unwrap();

        let error = discover_source_folders(
            &source_root(&file),
            &DestinationSelection::new(directory.path().join("destination"), false, true),
        )
        .unwrap_err();

        assert!(matches!(error, FilesystemError::ReadDirectory { .. }));
    }

    #[test]
    fn destination_paths_resolve_relative_and_absolute_without_creating_missing_directories() {
        let directory = tempdir().unwrap();
        let root = source_root(directory.path());
        let existing = directory.path().join("existing");
        fs::create_dir(&existing).unwrap();

        let relative = resolve_destination(&root, "./existing/../existing").unwrap();
        let absolute = resolve_destination(&root, existing.to_str().unwrap()).unwrap();
        let missing_path = directory.path().join("new/nested/destination");
        let missing = resolve_destination(&root, "new/nested/destination").unwrap();

        assert_eq!(relative.path(), existing);
        assert_eq!(absolute.path(), existing);
        assert!(relative.exists());
        assert!(!relative.may_create_after_confirmation());
        assert_eq!(missing.path(), missing_path);
        assert!(!missing.exists());
        assert!(missing.may_create_after_confirmation());
        assert!(!missing_path.exists());
    }

    #[test]
    fn destination_rejects_the_source_root_files_and_unsupported_links() {
        let directory = tempdir().unwrap();
        let root = source_root(directory.path());
        let file = directory.path().join("destination.txt");
        fs::write(&file, "content").unwrap();

        assert!(matches!(
            resolve_destination(&root, "."),
            Err(FilesystemError::DestinationIsSourceRoot { .. })
        ));
        assert!(matches!(
            resolve_destination(&root, "destination.txt"),
            Err(FilesystemError::DestinationIsFile { .. })
        ));

        #[cfg(unix)]
        {
            let link = directory.path().join("destination-link");
            std::os::unix::fs::symlink(directory.path(), &link).unwrap();
            assert!(matches!(
                resolve_destination(&root, "destination-link"),
                Err(FilesystemError::DestinationSymlink { .. })
            ));
        }
    }

    #[test]
    fn missing_destination_rejects_a_file_parent() {
        let directory = tempdir().unwrap();
        let root = source_root(directory.path());
        let parent = directory.path().join("parent-file");
        fs::write(&parent, "content").unwrap();

        let error = resolve_destination(&root, "parent-file/child").unwrap_err();

        assert!(
            matches!(error, FilesystemError::DestinationParentNotDirectory { .. }),
            "unexpected error: {error:?}"
        );
    }

    #[test]
    fn source_folder_discovery_is_direct_sorted_and_excludes_destination() {
        let directory = tempdir().unwrap();
        for name in ["Beta", "alpha", "organized", "nested-parent"] {
            fs::create_dir(directory.path().join(name)).unwrap();
        }
        fs::create_dir(directory.path().join("nested-parent").join("inside")).unwrap();
        fs::write(directory.path().join("root-file.mkv"), "content").unwrap();

        let root = source_root(directory.path());
        let destination = resolve_destination(&root, "organized").unwrap();
        let result = discover_source_folders(&root, &destination).unwrap();
        let names: Vec<_> = result
            .items()
            .iter()
            .map(|folder| folder.path().file_name().unwrap().to_string_lossy())
            .collect();

        assert_eq!(names, vec!["alpha", "Beta", "nested-parent"]);
        assert!(result.warnings().is_empty());
        assert!(!names.iter().any(|name| *name == "organized"));
    }

    #[test]
    fn a_destination_nested_inside_a_source_folder_excludes_that_folder() {
        let directory = tempdir().unwrap();
        let source = directory.path().join("source");
        fs::create_dir(&source).unwrap();
        let destination_input = source.join("organized");
        let root = source_root(directory.path());
        let destination = resolve_destination(&root, destination_input.to_str().unwrap()).unwrap();

        let result = discover_source_folders(&root, &destination).unwrap();

        assert!(result.items().is_empty());
    }

    #[test]
    fn source_folder_discovery_skips_symbolic_links_without_following_them() {
        #[cfg(unix)]
        {
            let directory = tempdir().unwrap();
            let real = directory.path().join("real");
            let link = directory.path().join("linked");
            fs::create_dir(&real).unwrap();
            std::os::unix::fs::symlink(&real, &link).unwrap();

            let root = source_root(directory.path());
            let destination =
                DestinationSelection::new(directory.path().join("destination"), false, true);
            let result = discover_source_folders(&root, &destination).unwrap();

            assert_eq!(result.items().len(), 1);
            assert_eq!(result.items()[0].path(), real);
            assert!(
                result
                    .warnings()
                    .iter()
                    .any(|warning| warning.path() == link)
            );
        }
    }

    #[test]
    fn video_discovery_is_recursive_regular_supports_common_extensions_and_sorted() {
        let directory = tempdir().unwrap();
        let folder_path = directory.path().join("source");
        fs::create_dir(&folder_path).unwrap();
        fs::write(folder_path.join("zeta.MKV"), "z").unwrap();
        fs::write(folder_path.join("Alpha.mKv"), "alpha").unwrap();
        fs::write(folder_path.join("movie.MP4"), "movie").unwrap();
        fs::write(folder_path.join("trailer.webm"), "trailer").unwrap();
        fs::write(folder_path.join("ignore.txt"), "ignore").unwrap();
        fs::create_dir(folder_path.join("nested.mkv")).unwrap();
        fs::create_dir(folder_path.join("nested")).unwrap();
        fs::write(folder_path.join("nested.mkv").join("inside.avi"), "inside").unwrap();
        fs::write(folder_path.join("nested").join("hidden.mkv"), "hidden").unwrap();

        let result = discover_video_files(&SourceFolder::new(folder_path.clone())).unwrap();
        let names: Vec<_> = result
            .items()
            .iter()
            .map(|file| {
                file.path()
                    .strip_prefix(&folder_path)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();

        assert_eq!(
            names,
            vec![
                "Alpha.mKv",
                "movie.MP4",
                "nested.mkv/inside.avi",
                "nested/hidden.mkv",
                "trailer.webm",
                "zeta.MKV",
            ]
        );
        assert_eq!(result.items()[0].size_bytes(), Some(5));
        assert!(result.warnings().is_empty());
    }

    #[test]
    fn source_root_video_discovery_includes_root_files_and_excludes_destination_subtrees() {
        let directory = tempdir().unwrap();
        let source = directory.path().join("source");
        let nested = source.join("nested");
        let destination = directory.path().join("organized");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&nested).unwrap();
        fs::create_dir(&destination).unwrap();
        fs::write(directory.path().join("root.mp4"), "root").unwrap();
        fs::write(source.join("movie.mkv"), "movie").unwrap();
        fs::write(nested.join("episode.webm"), "episode").unwrap();
        fs::write(destination.join("already-organized.mkv"), "ignored").unwrap();
        fs::create_dir(destination.join("nested")).unwrap();
        fs::write(
            destination.join("nested").join("also-ignored.mp4"),
            "ignored",
        )
        .unwrap();

        let root = source_root(directory.path());
        let destination = resolve_destination(&root, "organized").unwrap();
        let result = discover_video_files_in_source_root(&root, &destination).unwrap();
        let paths: Vec<_> = result
            .items()
            .iter()
            .map(|file| {
                file.path()
                    .strip_prefix(directory.path())
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();

        assert_eq!(
            paths,
            vec!["root.mp4", "source/movie.mkv", "source/nested/episode.webm"]
        );
        assert!(result.warnings().is_empty());
    }

    #[test]
    fn video_discovery_skips_symbolic_links() {
        #[cfg(unix)]
        {
            let directory = tempdir().unwrap();
            let folder = directory.path().join("source");
            let real = folder.join("real.mkv");
            let link = folder.join("linked.mkv");
            fs::create_dir(&folder).unwrap();
            fs::write(&real, "video").unwrap();
            std::os::unix::fs::symlink(&real, &link).unwrap();

            let result = discover_video_files(&SourceFolder::new(folder)).unwrap();

            assert_eq!(result.items().len(), 1);
            assert_eq!(result.items()[0].path(), real);
            assert!(
                result
                    .warnings()
                    .iter()
                    .any(|warning| warning.path() == link)
            );
        }
    }

    #[test]
    fn video_extension_matching_is_case_insensitive_and_rejects_non_video_files() {
        for path in [
            "movie.MKV",
            "movie.mp4",
            "movie.Avi",
            "movie.mov",
            "movie.webm",
            "movie.m2ts",
            "movie.ts",
            "movie.wmv",
        ] {
            assert!(
                has_video_extension(Path::new(path)),
                "expected {path} to match"
            );
        }

        for path in ["subtitle.srt", "poster.png", "README", "movie.mp4.txt"] {
            assert!(
                !has_video_extension(Path::new(path)),
                "expected {path} not to match"
            );
        }
    }

    #[test]
    fn destination_validation_rejects_overlap_with_selected_sources() {
        let directory = tempdir().unwrap();
        let source_path = directory.path().join("source");
        fs::create_dir(&source_path).unwrap();
        let destination = DestinationSelection::new(source_path.join("organized"), false, true);

        let error =
            validate_destination_against_sources(&destination, &[SourceFolder::new(source_path)])
                .unwrap_err();

        assert!(matches!(
            error,
            FilesystemError::DestinationIsSelectedSource { .. }
        ));
    }

    #[test]
    fn same_volume_execution_moves_contents_without_overwriting_an_existing_file() {
        let directory = tempdir().unwrap();
        let source_folder = directory.path().join("source");
        let destination = directory.path().join("organized");
        let source = source_folder.join("movie.MKV");
        let final_name = "550__S__MOVIE__S__Fight Club.mkv";
        fs::create_dir(&source_folder).unwrap();
        fs::create_dir(&destination).unwrap();
        fs::write(&source, "video bytes").unwrap();

        let plan = operation_plan(
            directory.path(),
            &destination,
            true,
            vec![planned_operation(
                &source_folder,
                &source,
                &destination,
                final_name,
            )],
        );

        let mut progress = Vec::new();
        let report = execute_validated_operation_plan(&plan, MoveStrategy::SameVolume, |update| {
            progress.push(update);
        });

        assert!(report.is_success());
        assert!(!source.exists());
        assert_eq!(
            fs::read_to_string(destination.join(final_name)).unwrap(),
            "video bytes"
        );
        assert_eq!(progress.last().unwrap().completed_bytes(), 11);
        assert_eq!(progress.last().unwrap().total_bytes(), 11);
    }

    #[test]
    fn copy_execution_preserves_an_independent_source_and_reports_byte_progress() {
        let directory = tempdir().unwrap();
        let source_folder = directory.path().join("source");
        let destination = directory.path().join("organized");
        let source = source_folder.join("movie.MKV");
        let final_name = "550__S__MOVIE__S__Fight Club.mkv";
        fs::create_dir(&source_folder).unwrap();
        fs::create_dir(&destination).unwrap();
        fs::write(&source, "copy operation bytes").unwrap();

        let plan = operation_plan_with_mode(
            directory.path(),
            &destination,
            true,
            FileOperation::Copy,
            vec![planned_operation(
                &source_folder,
                &source,
                &destination,
                final_name,
            )],
        );
        validate_operation_plan(&plan).unwrap();

        let mut progress = Vec::new();
        let report = execute_operation_plan_with_progress(&plan, |update| {
            progress.push(update);
        })
        .unwrap();

        assert!(report.is_success());
        assert!(source.exists());
        assert_eq!(
            fs::read_to_string(destination.join(final_name)).unwrap(),
            "copy operation bytes"
        );
        fs::write(&source, "changed source").unwrap();
        assert_eq!(
            fs::read_to_string(destination.join(final_name)).unwrap(),
            "copy operation bytes"
        );
        assert!(progress.iter().any(|update| {
            update.current_file_bytes() > 0
                && update.current_file_bytes() == update.current_file_total()
        }));
        assert_eq!(progress.last().unwrap().completed_bytes(), 20);
        assert_eq!(progress.last().unwrap().total_bytes(), 20);
    }

    #[test]
    fn an_existing_destination_is_rejected_before_any_file_is_changed() {
        let directory = tempdir().unwrap();
        let source_folder = directory.path().join("source");
        let destination = directory.path().join("organized");
        let source = source_folder.join("movie.mkv");
        let final_name = "550__S__MOVIE__S__Fight Club.mkv";
        let conflicting_destination = destination.join(final_name);
        fs::create_dir(&source_folder).unwrap();
        fs::create_dir(&destination).unwrap();
        fs::write(&source, "source bytes").unwrap();
        fs::write(&conflicting_destination, "existing bytes").unwrap();

        let plan = operation_plan(
            directory.path(),
            &destination,
            true,
            vec![planned_operation(
                &source_folder,
                &source,
                &destination,
                final_name,
            )],
        );

        let error = validate_operation_plan(&plan).unwrap_err();

        assert!(matches!(
            error,
            FilesystemError::DestinationAlreadyExists { .. }
        ));
        assert!(source.exists());
        assert_eq!(
            fs::read_to_string(conflicting_destination).unwrap(),
            "existing bytes"
        );
    }

    #[test]
    fn a_missing_destination_is_created_only_after_execution_begins() {
        let directory = tempdir().unwrap();
        let source_folder = directory.path().join("source");
        let destination = directory.path().join("new-organized");
        let source = source_folder.join("movie.mp4");
        let final_name = "550__S__MOVIE__S__Fight Club.mp4";
        fs::create_dir(&source_folder).unwrap();
        fs::write(&source, "source bytes").unwrap();

        let plan = operation_plan(
            directory.path(),
            &destination,
            false,
            vec![planned_operation(
                &source_folder,
                &source,
                &destination,
                final_name,
            )],
        );

        validate_operation_plan(&plan).unwrap();
        assert!(!destination.exists());

        let report = execute_operation_plan(&plan).unwrap();

        assert!(report.is_success());
        assert!(destination.is_dir());
        assert!(!source.exists());
        assert_eq!(
            fs::read_to_string(destination.join(final_name)).unwrap(),
            "source bytes"
        );
    }

    #[test]
    fn a_changed_source_is_rejected_without_mutation() {
        let directory = tempdir().unwrap();
        let source_folder = directory.path().join("source");
        let destination = directory.path().join("organized");
        let source = source_folder.join("movie.mkv");
        fs::create_dir(&source_folder).unwrap();
        fs::create_dir(&destination).unwrap();
        fs::write(&source, "original").unwrap();

        let plan = operation_plan(
            directory.path(),
            &destination,
            true,
            vec![planned_operation(
                &source_folder,
                &source,
                &destination,
                "550__S__MOVIE__S__Fight Club.mkv",
            )],
        );
        fs::write(&source, "the source changed after selection").unwrap();

        let error = validate_operation_plan(&plan).unwrap_err();

        assert!(matches!(error, FilesystemError::SourceChanged { .. }));
        assert!(source.exists());
        assert!(fs::read_dir(&destination).unwrap().next().is_none());
    }

    #[test]
    fn duplicate_destinations_are_rejected_before_execution() {
        let directory = tempdir().unwrap();
        let source_folder = directory.path().join("source");
        let destination = directory.path().join("organized");
        let first_source = source_folder.join("first.mkv");
        let second_source = source_folder.join("second.mkv");
        fs::create_dir(&source_folder).unwrap();
        fs::create_dir(&destination).unwrap();
        fs::write(&first_source, "first").unwrap();
        fs::write(&second_source, "second").unwrap();
        let final_name = "550__S__MOVIE__S__Same Title.mkv";

        let plan = operation_plan(
            directory.path(),
            &destination,
            true,
            vec![
                planned_operation(&source_folder, &first_source, &destination, final_name),
                planned_operation(&source_folder, &second_source, &destination, final_name),
            ],
        );

        let error = validate_operation_plan(&plan).unwrap_err();

        assert!(matches!(
            error,
            FilesystemError::DuplicateDestination { .. }
        ));
        assert!(first_source.exists());
        assert!(second_source.exists());
    }

    #[test]
    fn cross_volume_execution_verifies_and_publishes_the_copy_before_removing_source() {
        let directory = tempdir().unwrap();
        let source_folder = directory.path().join("source");
        let destination = directory.path().join("organized");
        let source = source_folder.join("movie.webm");
        let final_name = "550__S__MOVIE__S__Fight Club.webm";
        fs::create_dir(&source_folder).unwrap();
        fs::create_dir(&destination).unwrap();
        fs::write(&source, "cross volume video bytes").unwrap();

        let plan = operation_plan(
            directory.path(),
            &destination,
            true,
            vec![planned_operation(
                &source_folder,
                &source,
                &destination,
                final_name,
            )],
        );
        validate_operation_plan(&plan).unwrap();

        let report = execute_validated_operation_plan(&plan, MoveStrategy::CrossVolume, |_| {});

        assert!(report.is_success());
        assert!(!source.exists());
        assert_eq!(
            fs::read_to_string(destination.join(final_name)).unwrap(),
            "cross volume video bytes"
        );
        assert!(!contains_temporary_file(&destination));
    }

    #[test]
    fn cross_volume_verification_failure_preserves_source_and_cleans_temporary_copy() {
        let directory = tempdir().unwrap();
        let source_folder = directory.path().join("source");
        let destination = directory.path().join("organized");
        let source = source_folder.join("movie.mkv");
        let final_name = "550__S__MOVIE__S__Fight Club.mkv";
        fs::create_dir(&source_folder).unwrap();
        fs::create_dir(&destination).unwrap();
        fs::write(&source, "video bytes").unwrap();

        let extension = source_video_extension(&source).unwrap();
        let snapshot = snapshot_source_file(&source).unwrap();
        let operation = PlannedOperation::new(
            source_folder.clone(),
            source.clone(),
            destination.join(final_name),
            final_name.to_owned(),
            tmdb_movie("Fight Club"),
            None,
            extension,
            FileSnapshot::new(snapshot.size_bytes() + 1, snapshot.modified()),
        );

        let mut progress = |_: TransferProgress| {};
        let mut transfer = TransferContext::new(
            0,
            1,
            0,
            operation.source_snapshot().size_bytes(),
            &mut progress,
        );
        let error = copy_with_temporary_publish(&operation, true, &mut transfer).unwrap_err();

        assert!(matches!(error, FilesystemError::CopyVerification { .. }));
        assert!(source.exists());
        assert!(!destination.join(final_name).exists());
        assert!(!contains_temporary_file(&destination));
    }

    #[test]
    fn execution_stops_after_a_late_conflict_and_reports_pending_operations() {
        let directory = tempdir().unwrap();
        let source_folder = directory.path().join("source");
        let destination = directory.path().join("organized");
        let sources = [
            source_folder.join("first.mkv"),
            source_folder.join("second.mkv"),
            source_folder.join("third.mkv"),
        ];
        let names = [
            "550__S__MOVIE__S__First.mkv",
            "550__S__MOVIE__S__Second.mkv",
            "550__S__MOVIE__S__Third.mkv",
        ];
        fs::create_dir(&source_folder).unwrap();
        fs::create_dir(&destination).unwrap();
        for (source, content) in sources.iter().zip(["first", "second", "third"]) {
            fs::write(source, content).unwrap();
        }

        let operations = sources
            .iter()
            .zip(names)
            .map(|(source, name)| planned_operation(&source_folder, source, &destination, name))
            .collect();
        let plan = operation_plan(directory.path(), &destination, true, operations);
        validate_operation_plan(&plan).unwrap();
        let late_conflict = plan.operations()[1].destination_path().to_owned();

        let report =
            execute_validated_operation_plan(&plan, MoveStrategy::SameVolume, |progress| {
                if progress.operation_index() == 1 {
                    fs::write(&late_conflict, "late conflict").unwrap();
                }
            });

        assert_eq!(report.completed_count(), 1);
        assert_eq!(report.failed_count(), 1);
        assert_eq!(report.pending_count(), 1);
        assert!(!sources[0].exists());
        assert!(sources[1].exists());
        assert!(sources[2].exists());
        assert_eq!(fs::read_to_string(late_conflict).unwrap(), "late conflict");
    }

    fn contains_temporary_file(directory: &Path) -> bool {
        fs::read_dir(directory).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".tmdbtag.")
        })
    }
}
