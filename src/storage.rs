//! Storage backends and cross-storage transfer execution.
//!
//! The organization workflow treats local disks and S3-compatible object storage as the same
//! kind of boundary: discover immutable media candidates, build a complete plan, validate it
//! again at the commit point, publish each destination without replacement, and remove a source
//! only after the destination has been verified. Terminal rendering and TMDB orchestration stay
//! outside this module.

use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use aws_sdk_s3::{
    Client,
    config::{Credentials, Region},
    error::ProvideErrorMetadata,
    primitives::ByteStream,
    types::{CompletedMultipartUpload, CompletedPart},
};

use crate::{
    config::S3Config,
    domain::{
        EpisodeRef, FileOperation, OperationStatus, SourceRoot, StorageKind, TmdbItem,
        VideoExtension,
    },
    error::StorageError,
    filesystem,
};

const S3_MULTIPART_THRESHOLD: u64 = 8 * 1024 * 1024;
const S3_MULTIPART_PART_SIZE: usize = 8 * 1024 * 1024;
const LOCAL_TRANSFER_BUFFER_SIZE: usize = 1024 * 1024;

static NEXT_TEMP_FILE_ID: AtomicU64 = AtomicU64::new(1);

/// A path owned by one of the supported storage backends.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum StoragePath {
    /// An exact local filesystem path.
    Local(PathBuf),
    /// A normalized full object key, including the configured S3 base prefix.
    S3(String),
}

impl StoragePath {
    /// Returns the backend represented by this path.
    pub const fn kind(&self) -> StorageKind {
        match self {
            Self::Local(_) => StorageKind::Local,
            Self::S3(_) => StorageKind::S3,
        }
    }

    /// Returns the local path when this is a local storage path.
    pub fn local_path(&self) -> Option<&Path> {
        match self {
            Self::Local(path) => Some(path),
            Self::S3(_) => None,
        }
    }

    /// Returns the S3 key when this is an object-storage path.
    pub fn s3_key(&self) -> Option<&str> {
        match self {
            Self::Local(_) => None,
            Self::S3(key) => Some(key),
        }
    }

    /// Returns a safe display form that never changes the owned path.
    pub fn display(&self) -> String {
        match self {
            Self::Local(path) => path.to_string_lossy().into_owned(),
            Self::S3(key) => key.clone(),
        }
    }

    /// Returns a display path relative to a storage root.
    pub fn display_relative_to(&self, root: &Self) -> String {
        match (self, root) {
            (Self::Local(path), Self::Local(root)) => relative_local_path(path, root),
            (Self::S3(key), Self::S3(root)) => relative_s3_key(key, root),
            _ => self.display(),
        }
    }

    /// Joins one generated filename without allowing it to escape the destination root.
    pub fn join_filename(&self, filename: &str) -> Result<Self, StorageError> {
        if filename.is_empty()
            || filename == "."
            || filename == ".."
            || filename.contains('/')
            || filename.contains('\\')
            || filename.chars().any(char::is_control)
        {
            return Err(StorageError::InvalidPath {
                path: sanitize_display(filename),
                reason: "the generated filename contains an unsafe path component",
            });
        }

        match self {
            Self::Local(path) => Ok(Self::Local(path.join(filename))),
            Self::S3(key) => Ok(Self::S3(join_s3_key(key, filename))),
        }
    }
}

/// A destination selected before it is created or used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageDestination {
    path: StoragePath,
    exists: bool,
    may_create_after_confirmation: bool,
}

impl StorageDestination {
    /// Creates a destination with the state observed during selection.
    pub fn new(path: StoragePath, exists: bool, may_create_after_confirmation: bool) -> Self {
        Self {
            path,
            exists,
            may_create_after_confirmation,
        }
    }

    /// Returns the exact destination path or object prefix.
    pub fn path(&self) -> &StoragePath {
        &self.path
    }

    /// Returns whether the destination existed during selection.
    pub const fn exists(&self) -> bool {
        self.exists
    }

    /// Returns whether creation is allowed after the final confirmation.
    pub const fn may_create_after_confirmation(&self) -> bool {
        self.may_create_after_confirmation
    }
}

/// One recursively discovered video candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageVideoFile {
    path: StoragePath,
    relative_path: String,
    size_bytes: Option<u64>,
}

impl StorageVideoFile {
    /// Creates a discovered file while retaining an exact backend path and relative label.
    pub fn new(path: StoragePath, relative_path: String, size_bytes: Option<u64>) -> Self {
        Self {
            path,
            relative_path,
            size_bytes,
        }
    }

    /// Returns the exact source path.
    pub fn path(&self) -> &StoragePath {
        &self.path
    }

    /// Returns the display path relative to the selected source root.
    pub fn relative_path(&self) -> &str {
        &self.relative_path
    }

    /// Returns the observed size, when it was available during discovery.
    pub const fn size_bytes(&self) -> Option<u64> {
        self.size_bytes
    }
}

/// A non-fatal condition encountered during storage discovery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageWarning {
    path: String,
    reason: String,
}

impl StorageWarning {
    /// Creates a display-safe discovery warning.
    pub fn new(path: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            reason: reason.into(),
        }
    }

    /// Returns the affected relative path or object key.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Returns the warning reason.
    pub fn reason(&self) -> &str {
        &self.reason
    }
}

/// The result of one backend discovery pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageDiscovery {
    items: Vec<StorageVideoFile>,
    warnings: Vec<StorageWarning>,
}

impl StorageDiscovery {
    /// Creates a discovery result.
    pub fn new(items: Vec<StorageVideoFile>, warnings: Vec<StorageWarning>) -> Self {
        Self { items, warnings }
    }

    /// Returns eligible files in deterministic order.
    pub fn items(&self) -> &[StorageVideoFile] {
        &self.items
    }

    /// Returns non-fatal discovery warnings.
    pub fn warnings(&self) -> &[StorageWarning] {
        &self.warnings
    }
}

/// The complete non-mutating storage selection for one organization attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageSelection {
    source_root: StoragePath,
    destination: StorageDestination,
    operation: FileOperation,
    files: Vec<StorageVideoFile>,
    source_description: String,
    destination_description: String,
}

impl StorageSelection {
    /// Creates a selection returned by the unified storage workflow.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        source_root: StoragePath,
        destination: StorageDestination,
        operation: FileOperation,
        files: Vec<StorageVideoFile>,
        source_description: String,
        destination_description: String,
    ) -> Self {
        Self {
            source_root,
            destination,
            operation,
            files,
            source_description,
            destination_description,
        }
    }

    /// Returns the source storage root.
    pub fn source_root(&self) -> &StoragePath {
        &self.source_root
    }

    /// Returns the selected destination.
    pub fn destination(&self) -> &StorageDestination {
        &self.destination
    }

    /// Returns the selected copy-or-move operation.
    pub const fn operation(&self) -> FileOperation {
        self.operation
    }

    /// Returns selected files in explorer order.
    pub fn files(&self) -> &[StorageVideoFile] {
        &self.files
    }

    /// Returns the source label used in previews.
    pub fn source_description(&self) -> &str {
        &self.source_description
    }

    /// Returns the destination label used in previews.
    pub fn destination_description(&self) -> &str {
        &self.destination_description
    }
}

/// A backend-specific source identity captured while a plan is built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageSnapshot {
    size_bytes: u64,
    identity: Option<String>,
}

impl StorageSnapshot {
    /// Creates a snapshot from size and an optional backend identity.
    pub fn new(size_bytes: u64, identity: Option<String>) -> Self {
        Self {
            size_bytes,
            identity,
        }
    }

    /// Returns the observed byte size.
    pub const fn size_bytes(&self) -> u64 {
        self.size_bytes
    }

    /// Returns the optional opaque backend identity, such as an S3 ETag.
    pub fn identity(&self) -> Option<&str> {
        self.identity.as_deref()
    }

    /// Compares the current observation to the planned source state.
    pub fn matches(&self, current: &Self) -> bool {
        self.size_bytes == current.size_bytes
            && self
                .identity
                .as_deref()
                .is_none_or(|identity| current.identity.as_deref() == Some(identity))
    }
}

/// One immutable cross-storage operation in the user-approved plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoragePlannedOperation {
    source_path: StoragePath,
    destination_path: StoragePath,
    source_display: String,
    destination_display: String,
    normalized_filename: String,
    tmdb_item: TmdbItem,
    episode: Option<EpisodeRef>,
    source_extension: VideoExtension,
    source_snapshot: StorageSnapshot,
}

impl StoragePlannedOperation {
    /// Creates an operation from verified metadata and a source snapshot.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        source_path: StoragePath,
        destination_path: StoragePath,
        source_display: String,
        destination_display: String,
        normalized_filename: String,
        tmdb_item: TmdbItem,
        episode: Option<EpisodeRef>,
        source_extension: VideoExtension,
        source_snapshot: StorageSnapshot,
    ) -> Self {
        Self {
            source_path,
            destination_path,
            source_display,
            destination_display,
            normalized_filename,
            tmdb_item,
            episode,
            source_extension,
            source_snapshot,
        }
    }

    /// Returns the exact source path.
    pub fn source_path(&self) -> &StoragePath {
        &self.source_path
    }

    /// Returns the exact destination path.
    pub fn destination_path(&self) -> &StoragePath {
        &self.destination_path
    }

    /// Returns the source path shown in the preview.
    pub fn source_display(&self) -> &str {
        &self.source_display
    }

    /// Returns the destination path shown in the preview.
    pub fn destination_display(&self) -> &str {
        &self.destination_display
    }

    /// Returns the generated filename.
    pub fn normalized_filename(&self) -> &str {
        &self.normalized_filename
    }

    /// Returns the verified TMDB item.
    pub fn tmdb_item(&self) -> &TmdbItem {
        &self.tmdb_item
    }

    /// Returns the verified series episode, if this is a series operation.
    pub const fn episode(&self) -> Option<EpisodeRef> {
        self.episode
    }

    /// Returns the selected source extension.
    pub fn source_extension(&self) -> &VideoExtension {
        &self.source_extension
    }

    /// Returns the planned source snapshot.
    pub fn source_snapshot(&self) -> &StorageSnapshot {
        &self.source_snapshot
    }
}

/// The complete immutable storage plan displayed before mutation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoragePlan {
    source_root: StoragePath,
    destination: StorageDestination,
    operation: FileOperation,
    source_description: String,
    destination_description: String,
    operations: Vec<StoragePlannedOperation>,
}

impl StoragePlan {
    /// Creates a plan from a complete storage selection and planned metadata operations.
    pub fn new(selection: &StorageSelection, operations: Vec<StoragePlannedOperation>) -> Self {
        Self {
            source_root: selection.source_root.clone(),
            destination: selection.destination.clone(),
            operation: selection.operation,
            source_description: selection.source_description.clone(),
            destination_description: selection.destination_description.clone(),
            operations,
        }
    }

    /// Returns the source storage root.
    pub fn source_root(&self) -> &StoragePath {
        &self.source_root
    }

    /// Returns the destination selection.
    pub fn destination(&self) -> &StorageDestination {
        &self.destination
    }

    /// Returns the selected operation.
    pub const fn operation(&self) -> FileOperation {
        self.operation
    }

    /// Returns the source description.
    pub fn source_description(&self) -> &str {
        &self.source_description
    }

    /// Returns the destination description.
    pub fn destination_description(&self) -> &str {
        &self.destination_description
    }

    /// Returns operations in deterministic preview order.
    pub fn operations(&self) -> &[StoragePlannedOperation] {
        &self.operations
    }

    /// Returns the number of planned files.
    pub fn operation_count(&self) -> usize {
        self.operations.len()
    }

    /// Returns the total source bytes represented by the plan.
    pub fn total_size_bytes(&self) -> u64 {
        self.operations.iter().fold(0, |total, operation| {
            total.saturating_add(operation.source_snapshot.size_bytes())
        })
    }
}

/// The result for one storage operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageOperationResult {
    source_display: String,
    destination_display: String,
    status: OperationStatus,
}

impl StorageOperationResult {
    fn new(
        source_display: impl Into<String>,
        destination_display: impl Into<String>,
        status: OperationStatus,
    ) -> Self {
        Self {
            source_display: source_display.into(),
            destination_display: destination_display.into(),
            status,
        }
    }

    /// Returns the source label.
    pub fn source_display(&self) -> &str {
        &self.source_display
    }

    /// Returns the destination label.
    pub fn destination_display(&self) -> &str {
        &self.destination_display
    }

    /// Returns the final status.
    pub fn status(&self) -> &OperationStatus {
        &self.status
    }
}

/// The report returned after a confirmed storage plan starts executing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageExecutionReport {
    operation: FileOperation,
    source_description: String,
    destination_description: String,
    results: Vec<StorageOperationResult>,
}

impl StorageExecutionReport {
    fn new(
        operation: FileOperation,
        source_description: impl Into<String>,
        destination_description: impl Into<String>,
        results: Vec<StorageOperationResult>,
    ) -> Self {
        Self {
            operation,
            source_description: source_description.into(),
            destination_description: destination_description.into(),
            results,
        }
    }

    /// Returns the selected operation.
    pub const fn operation(&self) -> FileOperation {
        self.operation
    }

    /// Returns the source description.
    pub fn source_description(&self) -> &str {
        &self.source_description
    }

    /// Returns the destination description.
    pub fn destination_description(&self) -> &str {
        &self.destination_description
    }

    /// Returns each result in plan order.
    pub fn results(&self) -> &[StorageOperationResult] {
        &self.results
    }

    /// Counts completed results.
    pub fn completed_count(&self) -> usize {
        self.results
            .iter()
            .filter(|result| result.status().is_completed())
            .count()
    }

    /// Counts failed results.
    pub fn failed_count(&self) -> usize {
        self.results
            .iter()
            .filter(|result| result.status().is_failed())
            .count()
    }

    /// Counts results that were not started after a failure.
    pub fn pending_count(&self) -> usize {
        self.results
            .iter()
            .filter(|result| matches!(result.status(), OperationStatus::Pending))
            .count()
    }

    /// Returns whether every planned operation completed.
    pub fn is_success(&self) -> bool {
        !self.results.is_empty() && self.completed_count() == self.results.len()
    }
}

/// Aggregate byte progress for one confirmed storage plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StorageTransferProgress {
    operation_index: usize,
    operation_count: usize,
    completed_bytes: u64,
    total_bytes: u64,
    current_file_bytes: u64,
    current_file_total: u64,
}

impl StorageTransferProgress {
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

    /// Returns the zero-based operation index.
    pub const fn operation_index(self) -> usize {
        self.operation_index
    }

    /// Returns the number of operations.
    pub const fn operation_count(self) -> usize {
        self.operation_count
    }

    /// Returns aggregate transferred bytes.
    pub const fn completed_bytes(self) -> u64 {
        self.completed_bytes
    }

    /// Returns aggregate planned bytes.
    pub const fn total_bytes(self) -> u64 {
        self.total_bytes
    }

    /// Returns bytes transferred for the active file.
    pub const fn current_file_bytes(self) -> u64 {
        self.current_file_bytes
    }

    /// Returns planned bytes for the active file.
    pub const fn current_file_total(self) -> u64 {
        self.current_file_total
    }
}

/// The common boundary implemented by every source and destination storage backend.
pub trait StorageBackend {
    /// Returns the stable backend kind.
    fn kind(&self) -> StorageKind;

    /// Returns an English, credential-free description for previews.
    fn description(&self) -> String;

    /// Resolves the source root used for discovery.
    fn source_root(&self) -> Result<StoragePath, StorageError>;

    /// Resolves a user-entered destination without creating it.
    fn resolve_destination(
        &self,
        source_root: &StoragePath,
        input: &str,
    ) -> Result<StorageDestination, StorageError>;

    /// Recursively discovers eligible videos, optionally excluding a destination subtree.
    fn discover_videos(
        &self,
        source_root: &StoragePath,
        excluded_destination: Option<&StoragePath>,
    ) -> Result<StorageDiscovery, StorageError>;

    /// Validates the destination state and its relationship with the source root.
    fn validate_destination(
        &self,
        source_root: &StoragePath,
        destination: &StorageDestination,
    ) -> Result<(), StorageError>;

    /// Creates a missing destination at the commit point when the plan allows it.
    fn ensure_destination(&self, destination: &StorageDestination) -> Result<(), StorageError>;

    /// Checks whether one final destination object or file currently exists.
    fn destination_exists(&self, path: &StoragePath) -> Result<bool, StorageError>;

    /// Reads the current source size and backend identity.
    fn snapshot(&self, path: &StoragePath) -> Result<StorageSnapshot, StorageError>;

    /// Reads and validates a source video's extension.
    fn source_video_extension(&self, path: &StoragePath) -> Result<VideoExtension, StorageError>;

    /// Transfers one object within this backend, reporting bytes as they become complete.
    fn transfer_within(
        &self,
        source: &StoragePath,
        destination: &StoragePath,
        snapshot: &StorageSnapshot,
        on_progress: &mut dyn FnMut(u64),
    ) -> Result<(), StorageError>;

    /// Downloads one source object into a local destination-side temporary file.
    fn download_to_file(
        &self,
        source: &StoragePath,
        destination: &Path,
        snapshot: &StorageSnapshot,
        on_progress: &mut dyn FnMut(u64),
    ) -> Result<(), StorageError>;

    /// Uploads one local source file to a destination object without replacing it.
    fn upload_from_file(
        &self,
        source: &Path,
        destination: &StoragePath,
        snapshot: &StorageSnapshot,
        on_progress: &mut dyn FnMut(u64),
    ) -> Result<(), StorageError>;

    /// Creates a unique local destination-side temporary file path.
    fn create_temporary_path(
        &self,
        destination: &StoragePath,
        operation_index: usize,
    ) -> Result<PathBuf, StorageError>;

    /// Publishes a local temporary file at a local destination without replacement.
    fn publish_temporary_file(
        &self,
        temporary: &Path,
        destination: &StoragePath,
    ) -> Result<(), StorageError>;

    /// Removes a source only when its planned snapshot still matches.
    fn remove(&self, source: &StoragePath, snapshot: &StorageSnapshot) -> Result<(), StorageError>;
}

/// The local filesystem implementation of [`StorageBackend`].
#[derive(Debug, Default, Clone, Copy)]
pub struct LocalStorage;

impl LocalStorage {
    /// Creates a local filesystem backend.
    pub const fn new() -> Self {
        Self
    }

    fn local_path<'a>(&self, path: &'a StoragePath) -> Result<&'a Path, StorageError> {
        path.local_path().ok_or_else(|| StorageError::InvalidPath {
            path: path.display(),
            reason: "the local backend received an S3 object key",
        })
    }

    fn local_destination_directory(path: &Path) -> &Path {
        path.parent().unwrap_or_else(|| Path::new("."))
    }
}

impl StorageBackend for LocalStorage {
    fn kind(&self) -> StorageKind {
        StorageKind::Local
    }

    fn description(&self) -> String {
        "Local filesystem".to_owned()
    }

    fn source_root(&self) -> Result<StoragePath, StorageError> {
        filesystem::current_source_root()
            .map(|root| StoragePath::Local(root.path().to_owned()))
            .map_err(|error| StorageError::Local {
                operation: "resolving the current source directory",
                message: error.to_string(),
            })
    }

    fn resolve_destination(
        &self,
        source_root: &StoragePath,
        input: &str,
    ) -> Result<StorageDestination, StorageError> {
        let local_root = match source_root {
            StoragePath::Local(path) => SourceRoot::new(path.clone()),
            StoragePath::S3(_) => {
                filesystem::current_source_root().map_err(|error| StorageError::Local {
                    operation: "resolving the local destination root",
                    message: error.to_string(),
                })?
            }
        };
        filesystem::resolve_destination(&local_root, input)
            .map(|destination| {
                StorageDestination::new(
                    StoragePath::Local(destination.path().to_owned()),
                    destination.exists(),
                    destination.may_create_after_confirmation(),
                )
            })
            .map_err(|error| StorageError::Local {
                operation: "resolving the local destination",
                message: error.to_string(),
            })
    }

    fn discover_videos(
        &self,
        source_root: &StoragePath,
        excluded_destination: Option<&StoragePath>,
    ) -> Result<StorageDiscovery, StorageError> {
        let source_root_path = self.local_path(source_root)?;
        let root = SourceRoot::new(source_root_path.to_owned());
        let discovery = match excluded_destination.and_then(StoragePath::local_path) {
            Some(destination) => filesystem::discover_video_files_in_source_root(
                &root,
                &crate::domain::DestinationSelection::new(destination.to_owned(), true, false),
            ),
            None => filesystem::discover_video_files_in_source_root_without_destination(&root),
        }
        .map_err(|error| StorageError::Local {
            operation: "discovering local video files",
            message: error.to_string(),
        })?;

        let items = discovery
            .items()
            .iter()
            .map(|file| {
                let relative_path = relative_local_path(file.path(), source_root_path);
                StorageVideoFile::new(
                    StoragePath::Local(file.path().to_owned()),
                    relative_path,
                    file.size_bytes(),
                )
            })
            .collect();
        let warnings = discovery
            .warnings()
            .iter()
            .map(|warning| {
                StorageWarning::new(
                    relative_local_path(warning.path(), source_root_path),
                    warning.reason(),
                )
            })
            .collect();

        Ok(StorageDiscovery::new(items, warnings))
    }

    fn validate_destination(
        &self,
        source_root: &StoragePath,
        destination: &StorageDestination,
    ) -> Result<(), StorageError> {
        let destination_path = self.local_path(destination.path())?;
        if let StoragePath::Local(source_root_path) = source_root
            && paths_equivalent(source_root_path, destination_path)
        {
            return Err(StorageError::InvalidPath {
                path: relative_local_path(destination_path, source_root_path),
                reason: "the destination cannot be the source root",
            });
        }

        match fs::symlink_metadata(destination_path) {
            Ok(metadata) if metadata.file_type().is_symlink() => Err(StorageError::InvalidPath {
                path: destination_path.to_string_lossy().into_owned(),
                reason: "the destination cannot be a symbolic link",
            }),
            Ok(metadata) if !metadata.is_dir() => Err(StorageError::InvalidPath {
                path: destination_path.to_string_lossy().into_owned(),
                reason: "the destination must be a directory",
            }),
            Ok(_) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                validate_local_destination_parent(destination_path)
            }
            Err(error) => Err(StorageError::Local {
                operation: "validating the local destination",
                message: error.to_string(),
            }),
        }
    }

    fn ensure_destination(&self, destination: &StorageDestination) -> Result<(), StorageError> {
        let path = self.local_path(destination.path())?;
        match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                Err(StorageError::InvalidPath {
                    path: path.to_string_lossy().into_owned(),
                    reason: "the destination must be a real directory",
                })
            }
            Ok(_) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                if !destination.may_create_after_confirmation() {
                    return Err(StorageError::DestinationCreation {
                        path: path.to_string_lossy().into_owned(),
                        message: "creation was not authorized by the plan".to_owned(),
                    });
                }
                validate_local_destination_parent(path)?;
                fs::create_dir_all(path).map_err(|source| StorageError::DestinationCreation {
                    path: path.to_string_lossy().into_owned(),
                    message: source.to_string(),
                })
            }
            Err(error) => Err(StorageError::Local {
                operation: "creating or validating the local destination",
                message: error.to_string(),
            }),
        }
    }

    fn destination_exists(&self, path: &StoragePath) -> Result<bool, StorageError> {
        let path = self.local_path(path)?;
        match fs::symlink_metadata(path) {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(StorageError::Local {
                operation: "checking a local destination",
                message: error.to_string(),
            }),
        }
    }

    fn snapshot(&self, path: &StoragePath) -> Result<StorageSnapshot, StorageError> {
        let path = self.local_path(path)?;
        let snapshot =
            filesystem::snapshot_source_file(path).map_err(|error| StorageError::Local {
                operation: "reading local source metadata",
                message: error.to_string(),
            })?;
        let identity = snapshot.modified().and_then(|modified| {
            modified
                .duration_since(std::time::UNIX_EPOCH)
                .ok()
                .map(|duration| format!("{}:{}", duration.as_secs(), duration.subsec_nanos()))
        });
        Ok(StorageSnapshot::new(snapshot.size_bytes(), identity))
    }

    fn source_video_extension(&self, path: &StoragePath) -> Result<VideoExtension, StorageError> {
        let path = self.local_path(path)?;
        filesystem::source_video_extension(path).map_err(|error| StorageError::Local {
            operation: "reading the local video extension",
            message: error.to_string(),
        })
    }

    fn transfer_within(
        &self,
        source: &StoragePath,
        destination: &StoragePath,
        snapshot: &StorageSnapshot,
        on_progress: &mut dyn FnMut(u64),
    ) -> Result<(), StorageError> {
        let source = self.local_path(source)?;
        let destination = self.local_path(destination)?;
        let destination_storage_path = StoragePath::Local(destination.to_owned());
        let temporary = self.create_temporary_path(&destination_storage_path, 0)?;
        let result = copy_local_file(source, &temporary, snapshot.size_bytes(), on_progress)
            .and_then(|()| self.publish_temporary_file(&temporary, &destination_storage_path));
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    fn download_to_file(
        &self,
        source: &StoragePath,
        destination: &Path,
        snapshot: &StorageSnapshot,
        on_progress: &mut dyn FnMut(u64),
    ) -> Result<(), StorageError> {
        let source = self.local_path(source)?;
        copy_local_file(source, destination, snapshot.size_bytes(), on_progress)
    }

    fn upload_from_file(
        &self,
        _source: &Path,
        _destination: &StoragePath,
        _snapshot: &StorageSnapshot,
        _on_progress: &mut dyn FnMut(u64),
    ) -> Result<(), StorageError> {
        Err(StorageError::UnsupportedTransfer)
    }

    fn create_temporary_path(
        &self,
        destination: &StoragePath,
        operation_index: usize,
    ) -> Result<PathBuf, StorageError> {
        let destination = self.local_path(destination)?;
        let directory = Self::local_destination_directory(destination);
        let process_id = std::process::id();
        for attempt in 0..100_u64 {
            let id = NEXT_TEMP_FILE_ID.fetch_add(1, Ordering::Relaxed);
            let path = directory.join(format!(
                ".tmdbtag.{process_id}.{operation_index}.{id}.{attempt}.tmp"
            ));
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(_) => return Ok(path),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(StorageError::Local {
                        operation: "creating a destination-side temporary file",
                        message: error.to_string(),
                    });
                }
            }
        }
        Err(StorageError::Local {
            operation: "creating a destination-side temporary file",
            message: "could not find an unused temporary filename".to_owned(),
        })
    }

    fn publish_temporary_file(
        &self,
        temporary: &Path,
        destination: &StoragePath,
    ) -> Result<(), StorageError> {
        let destination = self.local_path(destination)?;
        match fs::hard_link(temporary, destination) {
            Ok(()) => fs::remove_file(temporary).map_err(|error| StorageError::TemporaryCleanup {
                path: temporary.to_string_lossy().into_owned(),
                message: error.to_string(),
            }),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                Err(StorageError::DestinationAlreadyExists {
                    path: destination.to_string_lossy().into_owned(),
                })
            }
            Err(error) => Err(StorageError::DestinationPublication {
                path: destination.to_string_lossy().into_owned(),
                message: error.to_string(),
            }),
        }
    }

    fn remove(&self, source: &StoragePath, snapshot: &StorageSnapshot) -> Result<(), StorageError> {
        let source_path = self.local_path(source)?;
        let current = self.snapshot(source)?;
        if !snapshot.matches(&current) {
            return Err(StorageError::SourceChanged {
                path: source_path.to_string_lossy().into_owned(),
            });
        }
        fs::remove_file(source_path).map_err(|error| StorageError::SourceRemoval {
            path: source_path.to_string_lossy().into_owned(),
            message: error.to_string(),
        })
    }
}

/// An S3-compatible object-storage implementation of the common storage boundary.
pub struct S3Storage {
    client: Client,
    runtime: tokio::runtime::Runtime,
    bucket: String,
    base_path: String,
    endpoint: String,
}

impl std::fmt::Debug for S3Storage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("S3Storage")
            .field("bucket", &self.bucket)
            .field("base_path", &self.base_path)
            .field("endpoint", &self.endpoint)
            .finish_non_exhaustive()
    }
}

impl S3Storage {
    /// Creates an S3 client using the explicitly configured endpoint and static credentials.
    pub fn new(config: &S3Config) -> Result<Self, StorageError> {
        let credentials = Credentials::new(
            config.access_key(),
            config.secret_key(),
            None,
            None,
            "tmdbtag-config",
        );
        let sdk_config = aws_sdk_s3::Config::builder()
            .behavior_version_latest()
            .region(Region::new(config.region().to_owned()))
            .endpoint_url(config.endpoint())
            .credentials_provider(credentials)
            .force_path_style(true)
            .build();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|_| StorageError::S3Request {
                operation: "initializing the S3 runtime",
            })?;

        Ok(Self {
            client: Client::from_conf(sdk_config),
            runtime,
            bucket: config.bucket().to_owned(),
            base_path: config.base_path().to_owned(),
            endpoint: config.endpoint().to_owned(),
        })
    }

    fn s3_key<'a>(&self, path: &'a StoragePath) -> Result<&'a str, StorageError> {
        path.s3_key().ok_or_else(|| StorageError::InvalidPath {
            path: path.display(),
            reason: "the S3 backend received a local filesystem path",
        })
    }

    fn is_missing_error<E>(error: &aws_sdk_s3::error::SdkError<E>) -> bool
    where
        E: std::fmt::Debug + ProvideErrorMetadata,
    {
        let code = error
            .as_service_error()
            .and_then(|service_error| service_error.code());
        code.is_some_and(|code| {
            matches!(
                code,
                "NotFound" | "NoSuchKey" | "NoSuchObject" | "404" | "Not Found"
            )
        }) || error
            .raw_response()
            .is_some_and(|response| response.status().as_u16() == 404)
    }

    fn head_object(
        &self,
        key: &str,
    ) -> Result<aws_sdk_s3::operation::head_object::HeadObjectOutput, StorageError> {
        self.runtime
            .block_on(
                self.client
                    .head_object()
                    .bucket(&self.bucket)
                    .key(key)
                    .send(),
            )
            .map_err(|error| map_s3_error(&error, "reading S3 object metadata"))
    }

    fn abort_multipart(&self, key: &str, upload_id: &str) {
        let _ = self.runtime.block_on(
            self.client
                .abort_multipart_upload()
                .bucket(&self.bucket)
                .key(key)
                .upload_id(upload_id)
                .send(),
        );
    }
}

fn map_s3_error<E>(error: &aws_sdk_s3::error::SdkError<E>, operation: &'static str) -> StorageError
where
    E: std::fmt::Debug + ProvideErrorMetadata,
{
    let code = error
        .as_service_error()
        .and_then(|service_error| service_error.code());
    let status = error
        .raw_response()
        .map(|response| response.status().as_u16());

    if status.is_some_and(|status| matches!(status, 401 | 403))
        || code.is_some_and(|code| {
            matches!(
                code,
                "AccessDenied"
                    | "ExpiredToken"
                    | "InvalidAccessKeyId"
                    | "InvalidToken"
                    | "SignatureDoesNotMatch"
                    | "Unauthorized"
            )
        })
    {
        return StorageError::S3Authentication { operation };
    }

    if status == Some(429)
        || code.is_some_and(|code| {
            matches!(
                code,
                "RequestLimitExceeded" | "SlowDown" | "Throttled" | "Throttling"
            )
        })
    {
        return StorageError::S3RateLimited { operation };
    }

    StorageError::S3Request { operation }
}

impl StorageBackend for S3Storage {
    fn kind(&self) -> StorageKind {
        StorageKind::S3
    }

    fn description(&self) -> String {
        let prefix = if self.base_path.is_empty() {
            "(bucket root)".to_owned()
        } else {
            self.base_path.clone()
        };
        format!(
            "S3-compatible storage · bucket {} · prefix {} · endpoint {}",
            self.bucket, prefix, self.endpoint
        )
    }

    fn source_root(&self) -> Result<StoragePath, StorageError> {
        Ok(StoragePath::S3(self.base_path.clone()))
    }

    fn resolve_destination(
        &self,
        _source_root: &StoragePath,
        input: &str,
    ) -> Result<StorageDestination, StorageError> {
        let relative = normalize_s3_key(input)?;
        let key = if self.base_path.is_empty() {
            relative
        } else if relative.is_empty() {
            self.base_path.clone()
        } else {
            join_s3_key(&self.base_path, &relative)
        };
        Ok(StorageDestination::new(StoragePath::S3(key), true, false))
    }

    fn discover_videos(
        &self,
        source_root: &StoragePath,
        excluded_destination: Option<&StoragePath>,
    ) -> Result<StorageDiscovery, StorageError> {
        let root_key = self.s3_key(source_root)?;
        let prefix = if root_key.is_empty() {
            String::new()
        } else {
            format!("{root_key}/")
        };
        let excluded_key = excluded_destination.and_then(StoragePath::s3_key);
        let mut continuation_token = None;
        let mut items = Vec::new();

        loop {
            let mut request = self
                .client
                .list_objects_v2()
                .bucket(&self.bucket)
                .prefix(&prefix);
            if let Some(token) = continuation_token.as_deref() {
                request = request.continuation_token(token);
            }
            let response = self
                .runtime
                .block_on(request.send())
                .map_err(|error| map_s3_error(&error, "discovering S3 video objects"))?;

            for object in response.contents() {
                let Some(key) = object.key() else {
                    continue;
                };
                if key.ends_with('/')
                    || excluded_key.is_some_and(|excluded| {
                        key == excluded || key.starts_with(&format!("{excluded}/"))
                    })
                {
                    continue;
                }
                let Some(relative_path) = key.strip_prefix(&prefix) else {
                    continue;
                };
                if relative_path.is_empty() || !is_safe_s3_relative_path(relative_path) {
                    continue;
                }
                let Some(extension) = Path::new(relative_path)
                    .extension()
                    .and_then(|extension| extension.to_str())
                else {
                    continue;
                };
                if VideoExtension::parse(extension).is_err() {
                    continue;
                }
                let size_bytes = object.size().and_then(|size| u64::try_from(size).ok());
                items.push(StorageVideoFile::new(
                    StoragePath::S3(key.to_owned()),
                    relative_path.to_owned(),
                    size_bytes,
                ));
            }

            continuation_token = response.next_continuation_token().map(ToOwned::to_owned);
            if continuation_token.is_none() {
                break;
            }
        }

        items.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
        Ok(StorageDiscovery::new(items, Vec::new()))
    }

    fn validate_destination(
        &self,
        source_root: &StoragePath,
        destination: &StorageDestination,
    ) -> Result<(), StorageError> {
        let Some(source_key) = source_root.s3_key() else {
            let _ = self.s3_key(destination.path())?;
            return Ok(());
        };
        let destination_key = self.s3_key(destination.path())?;
        if source_key == destination_key {
            return Err(StorageError::InvalidPath {
                path: if destination_key.is_empty() {
                    ".".to_owned()
                } else {
                    destination_key.to_owned()
                },
                reason: "the destination cannot be the source prefix",
            });
        }
        Ok(())
    }

    fn ensure_destination(&self, destination: &StorageDestination) -> Result<(), StorageError> {
        let _ = self.s3_key(destination.path())?;
        Ok(())
    }

    fn destination_exists(&self, path: &StoragePath) -> Result<bool, StorageError> {
        let key = self.s3_key(path)?;
        let response = self.runtime.block_on(
            self.client
                .head_object()
                .bucket(&self.bucket)
                .key(key)
                .send(),
        );
        match response {
            Ok(_) => Ok(true),
            Err(error) if Self::is_missing_error(&error) => Ok(false),
            Err(error) => Err(map_s3_error(&error, "checking an S3 destination object")),
        }
    }

    fn snapshot(&self, path: &StoragePath) -> Result<StorageSnapshot, StorageError> {
        let key = self.s3_key(path)?;
        let response = self.head_object(key)?;
        let size = response
            .content_length()
            .and_then(|value| u64::try_from(value).ok())
            .ok_or(StorageError::S3InvalidResponse {
                operation: "reading S3 object size",
            })?;
        Ok(StorageSnapshot::new(
            size,
            response.e_tag().map(ToOwned::to_owned),
        ))
    }

    fn source_video_extension(&self, path: &StoragePath) -> Result<VideoExtension, StorageError> {
        let key = self.s3_key(path)?;
        let extension = Path::new(key)
            .extension()
            .and_then(|extension| extension.to_str())
            .ok_or_else(|| StorageError::InvalidPath {
                path: key.to_owned(),
                reason: "the S3 object has no usable video extension",
            })?;
        VideoExtension::parse(extension).map_err(|_| StorageError::InvalidPath {
            path: key.to_owned(),
            reason: "the S3 object extension is not a supported video format",
        })
    }

    fn transfer_within(
        &self,
        source: &StoragePath,
        destination: &StoragePath,
        snapshot: &StorageSnapshot,
        on_progress: &mut dyn FnMut(u64),
    ) -> Result<(), StorageError> {
        let source_key = self.s3_key(source)?;
        let destination_key = self.s3_key(destination)?;
        let copy_source = format!(
            "{}/{}",
            self.bucket,
            urlencoding::encode(source_key).into_owned()
        );
        let mut request = self
            .client
            .copy_object()
            .bucket(&self.bucket)
            .key(destination_key)
            .copy_source(copy_source)
            .if_none_match("*");
        if let Some(etag) = snapshot.identity() {
            request = request.copy_source_if_match(etag);
        }
        self.runtime
            .block_on(request.send())
            .map_err(|error| map_s3_error(&error, "copying an S3 object"))?;
        on_progress(snapshot.size_bytes());
        Ok(())
    }

    fn download_to_file(
        &self,
        source: &StoragePath,
        destination: &Path,
        snapshot: &StorageSnapshot,
        on_progress: &mut dyn FnMut(u64),
    ) -> Result<(), StorageError> {
        let key = self.s3_key(source)?;
        let mut request = self.client.get_object().bucket(&self.bucket).key(key);
        if let Some(etag) = snapshot.identity() {
            request = request.if_match(etag);
        }
        let response = self
            .runtime
            .block_on(request.send())
            .map_err(|error| map_s3_error(&error, "downloading an S3 object"))?;
        let mut file = OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(destination)
            .map_err(|error| StorageError::Local {
                operation: "opening a download temporary file",
                message: error.to_string(),
            })?;
        let mut body = response.body;
        let mut completed = 0_u64;
        while let Some(chunk) =
            self.runtime
                .block_on(body.try_next())
                .map_err(|_| StorageError::S3Request {
                    operation: "reading an S3 object body",
                })?
        {
            file.write_all(&chunk)
                .map_err(|error| StorageError::Local {
                    operation: "writing a downloaded object",
                    message: error.to_string(),
                })?;
            completed = completed.saturating_add(chunk.len() as u64);
            on_progress(completed.min(snapshot.size_bytes()));
        }
        file.flush().map_err(|error| StorageError::Local {
            operation: "flushing a downloaded object",
            message: error.to_string(),
        })?;
        if completed != snapshot.size_bytes() {
            return Err(StorageError::CopyVerification {
                path: destination.to_string_lossy().into_owned(),
            });
        }
        Ok(())
    }

    fn upload_from_file(
        &self,
        source: &Path,
        destination: &StoragePath,
        snapshot: &StorageSnapshot,
        on_progress: &mut dyn FnMut(u64),
    ) -> Result<(), StorageError> {
        let destination_key = self.s3_key(destination)?;
        let metadata = fs::metadata(source).map_err(|error| StorageError::Local {
            operation: "reading the upload source",
            message: error.to_string(),
        })?;
        if metadata.len() != snapshot.size_bytes() {
            return Err(StorageError::SourceChanged {
                path: source.to_string_lossy().into_owned(),
            });
        }

        if snapshot.size_bytes() <= S3_MULTIPART_THRESHOLD {
            let body = fs::read(source).map_err(|error| StorageError::Local {
                operation: "reading the upload source",
                message: error.to_string(),
            })?;
            self.runtime
                .block_on(
                    self.client
                        .put_object()
                        .bucket(&self.bucket)
                        .key(destination_key)
                        .if_none_match("*")
                        .body(ByteStream::from(body))
                        .send(),
                )
                .map_err(|error| map_s3_error(&error, "uploading an object to S3"))?;
            on_progress(snapshot.size_bytes());
            return Ok(());
        }

        upload_multipart(
            self,
            source,
            destination_key,
            snapshot.size_bytes(),
            on_progress,
        )
    }

    fn create_temporary_path(
        &self,
        _destination: &StoragePath,
        _operation_index: usize,
    ) -> Result<PathBuf, StorageError> {
        Err(StorageError::UnsupportedTransfer)
    }

    fn publish_temporary_file(
        &self,
        _temporary: &Path,
        _destination: &StoragePath,
    ) -> Result<(), StorageError> {
        Err(StorageError::UnsupportedTransfer)
    }

    fn remove(&self, source: &StoragePath, snapshot: &StorageSnapshot) -> Result<(), StorageError> {
        let key = self.s3_key(source)?;
        let mut request = self.client.delete_object().bucket(&self.bucket).key(key);
        if let Some(etag) = snapshot.identity() {
            request = request.if_match(etag);
        }
        self.runtime
            .block_on(request.send())
            .map_err(|error| map_s3_error(&error, "removing the source object from S3"))?;
        Ok(())
    }
}

fn upload_multipart(
    storage: &S3Storage,
    source: &Path,
    destination_key: &str,
    total_size: u64,
    on_progress: &mut dyn FnMut(u64),
) -> Result<(), StorageError> {
    let created = storage
        .runtime
        .block_on(
            storage
                .client
                .create_multipart_upload()
                .bucket(&storage.bucket)
                .key(destination_key)
                .send(),
        )
        .map_err(|error| map_s3_error(&error, "starting a multipart S3 upload"))?;
    let upload_id = created.upload_id().ok_or(StorageError::S3InvalidResponse {
        operation: "starting a multipart S3 upload",
    })?;
    let mut file = File::open(source).map_err(|error| StorageError::Local {
        operation: "opening the multipart upload source",
        message: error.to_string(),
    })?;
    let mut completed_parts = Vec::new();
    let mut completed_bytes = 0_u64;
    let mut part_number = 1_i32;

    loop {
        let mut buffer = vec![0_u8; S3_MULTIPART_PART_SIZE];
        let read = file.read(&mut buffer).map_err(|error| StorageError::Local {
            operation: "reading a multipart upload source",
            message: error.to_string(),
        });
        let read = match read {
            Ok(read) => read,
            Err(error) => {
                storage.abort_multipart(destination_key, upload_id);
                return Err(error);
            }
        };
        if read == 0 {
            break;
        }
        buffer.truncate(read);

        let uploaded = storage.runtime.block_on(
            storage
                .client
                .upload_part()
                .bucket(&storage.bucket)
                .key(destination_key)
                .upload_id(upload_id)
                .part_number(part_number)
                .body(ByteStream::from(buffer))
                .send(),
        );
        let uploaded = match uploaded {
            Ok(uploaded) => uploaded,
            Err(error) => {
                storage.abort_multipart(destination_key, upload_id);
                return Err(map_s3_error(&error, "uploading a multipart S3 part"));
            }
        };
        let Some(etag) = uploaded.e_tag() else {
            storage.abort_multipart(destination_key, upload_id);
            return Err(StorageError::S3InvalidResponse {
                operation: "uploading a multipart S3 part",
            });
        };
        completed_parts.push(
            CompletedPart::builder()
                .e_tag(etag)
                .part_number(part_number)
                .build(),
        );
        completed_bytes = completed_bytes.saturating_add(read as u64);
        on_progress(completed_bytes.min(total_size));
        part_number += 1;
    }

    let multipart = CompletedMultipartUpload::builder()
        .set_parts(Some(completed_parts))
        .build();
    let completed = storage.runtime.block_on(
        storage
            .client
            .complete_multipart_upload()
            .bucket(&storage.bucket)
            .key(destination_key)
            .upload_id(upload_id)
            .if_none_match("*")
            .multipart_upload(multipart)
            .send(),
    );
    if let Err(error) = completed {
        storage.abort_multipart(destination_key, upload_id);
        return Err(map_s3_error(&error, "completing a multipart S3 upload"));
    }
    Ok(())
}

/// Validates every operation in a storage plan without mutating either backend.
pub fn validate_plan(
    plan: &StoragePlan,
    source_backend: &dyn StorageBackend,
    destination_backend: &dyn StorageBackend,
) -> Result<(), StorageError> {
    if plan.operations.is_empty() {
        return Err(StorageError::EmptyPlan);
    }
    if source_backend.kind() != plan.source_root.kind() {
        return Err(StorageError::InvalidPath {
            path: plan.source_root.display(),
            reason: "the plan source does not match the source storage backend",
        });
    }
    if destination_backend.kind() != plan.destination.path.kind() {
        return Err(StorageError::InvalidPath {
            path: plan.destination.path.display(),
            reason: "the plan destination does not match the destination storage backend",
        });
    }

    destination_backend.validate_destination(&plan.source_root, &plan.destination)?;
    let mut source_paths = Vec::new();
    let mut destination_paths = Vec::new();

    for operation in &plan.operations {
        if operation.source_path.kind() != source_backend.kind()
            || operation.destination_path.kind() != destination_backend.kind()
        {
            return Err(StorageError::InvalidPath {
                path: operation.source_display.clone(),
                reason: "an operation path does not match its storage backend",
            });
        }
        if source_paths
            .iter()
            .any(|path: &StoragePath| path == &operation.source_path)
        {
            return Err(StorageError::InvalidPath {
                path: operation.source_display.clone(),
                reason: "the same source was selected more than once",
            });
        }
        source_paths.push(operation.source_path.clone());

        if destination_paths
            .iter()
            .any(|path: &StoragePath| path == &operation.destination_path)
        {
            return Err(StorageError::InvalidPath {
                path: operation.destination_display.clone(),
                reason: "more than one selected file would use the same destination",
            });
        }
        destination_paths.push(operation.destination_path.clone());

        let current = source_backend.snapshot(&operation.source_path)?;
        if !operation.source_snapshot.matches(&current) {
            return Err(StorageError::SourceChanged {
                path: operation.source_display.clone(),
            });
        }
        let extension = source_backend.source_video_extension(&operation.source_path)?;
        if extension != operation.source_extension {
            return Err(StorageError::SourceChanged {
                path: operation.source_display.clone(),
            });
        }
        if destination_backend.destination_exists(&operation.destination_path)? {
            return Err(StorageError::DestinationAlreadyExists {
                path: operation.destination_display.clone(),
            });
        }
    }

    validate_local_source_containers(plan)?;
    Ok(())
}

/// Executes a validated plan and reports aggregate byte progress.
pub fn execute_plan_with_progress<F>(
    plan: &StoragePlan,
    source_backend: &dyn StorageBackend,
    destination_backend: &dyn StorageBackend,
    mut on_progress: F,
) -> Result<StorageExecutionReport, StorageError>
where
    F: FnMut(StorageTransferProgress),
{
    validate_plan(plan, source_backend, destination_backend)?;
    destination_backend.ensure_destination(&plan.destination)?;

    let total_bytes = plan.total_size_bytes();
    let operation_count = plan.operation_count();
    let mut completed_bytes = 0_u64;
    let mut results = Vec::with_capacity(operation_count);

    for (index, operation) in plan.operations.iter().enumerate() {
        let file_total = operation.source_snapshot.size_bytes();
        on_progress(StorageTransferProgress::new(
            index,
            operation_count,
            completed_bytes,
            total_bytes,
            0,
            file_total,
        ));

        let mut report_progress = |current_file_bytes: u64| {
            let current_file_bytes = current_file_bytes.min(file_total);
            let aggregate = completed_bytes
                .saturating_add(current_file_bytes)
                .min(total_bytes);
            on_progress(StorageTransferProgress::new(
                index,
                operation_count,
                aggregate,
                total_bytes,
                current_file_bytes,
                file_total,
            ));
        };

        match execute_one(
            plan.operation,
            operation,
            source_backend,
            destination_backend,
            &mut report_progress,
        ) {
            Ok(()) => {
                completed_bytes = completed_bytes.saturating_add(file_total).min(total_bytes);
                on_progress(StorageTransferProgress::new(
                    index,
                    operation_count,
                    completed_bytes,
                    total_bytes,
                    file_total,
                    file_total,
                ));
                results.push(StorageOperationResult::new(
                    operation.source_display.clone(),
                    operation.destination_display.clone(),
                    OperationStatus::Completed,
                ));
            }
            Err(error) => {
                results.push(StorageOperationResult::new(
                    operation.source_display.clone(),
                    operation.destination_display.clone(),
                    OperationStatus::Failed {
                        reason: error.to_string(),
                    },
                ));
                for pending in plan.operations.iter().skip(index + 1) {
                    results.push(StorageOperationResult::new(
                        pending.source_display.clone(),
                        pending.destination_display.clone(),
                        OperationStatus::Pending,
                    ));
                }
                break;
            }
        }
    }

    Ok(StorageExecutionReport::new(
        plan.operation,
        plan.source_description.clone(),
        plan.destination_description.clone(),
        results,
    ))
}

fn execute_one(
    file_operation: FileOperation,
    operation: &StoragePlannedOperation,
    source_backend: &dyn StorageBackend,
    destination_backend: &dyn StorageBackend,
    on_progress: &mut dyn FnMut(u64),
) -> Result<(), StorageError> {
    let current_source = source_backend.snapshot(&operation.source_path)?;
    if !operation.source_snapshot.matches(&current_source) {
        return Err(StorageError::SourceChanged {
            path: operation.source_display.clone(),
        });
    }
    if destination_backend.destination_exists(&operation.destination_path)? {
        return Err(StorageError::DestinationAlreadyExists {
            path: operation.destination_display.clone(),
        });
    }

    if operation.source_path.kind() == operation.destination_path.kind() {
        destination_backend.transfer_within(
            &operation.source_path,
            &operation.destination_path,
            &operation.source_snapshot,
            on_progress,
        )?;
    } else {
        match (
            operation.source_path.kind(),
            operation.destination_path.kind(),
        ) {
            (StorageKind::Local, StorageKind::S3) => {
                let source = operation.source_path.local_path().ok_or_else(|| {
                    StorageError::InvalidPath {
                        path: operation.source_display.clone(),
                        reason: "the local source path is unavailable",
                    }
                })?;
                destination_backend.upload_from_file(
                    source,
                    &operation.destination_path,
                    &operation.source_snapshot,
                    on_progress,
                )?;
            }
            (StorageKind::S3, StorageKind::Local) => {
                let temporary =
                    destination_backend.create_temporary_path(&operation.destination_path, 0)?;
                let download = source_backend.download_to_file(
                    &operation.source_path,
                    &temporary,
                    &operation.source_snapshot,
                    on_progress,
                );
                if let Err(error) = download {
                    let _ = fs::remove_file(&temporary);
                    return Err(error);
                }
                if let Err(error) = destination_backend
                    .publish_temporary_file(&temporary, &operation.destination_path)
                {
                    let _ = fs::remove_file(&temporary);
                    return Err(error);
                }
            }
            _ => return Err(StorageError::UnsupportedTransfer),
        }
    }

    let destination_snapshot = destination_backend.snapshot(&operation.destination_path)?;
    if destination_snapshot.size_bytes() != operation.source_snapshot.size_bytes() {
        return Err(StorageError::CopyVerification {
            path: operation.destination_display.clone(),
        });
    }

    let current_source = source_backend.snapshot(&operation.source_path)?;
    if !operation.source_snapshot.matches(&current_source) {
        return Err(StorageError::SourceChanged {
            path: operation.source_display.clone(),
        });
    }

    if file_operation == FileOperation::Move {
        source_backend.remove(&operation.source_path, &operation.source_snapshot)?;
    }
    Ok(())
}

fn validate_local_source_containers(plan: &StoragePlan) -> Result<(), StorageError> {
    let (StoragePath::Local(source_root), StoragePath::Local(destination)) =
        (&plan.source_root, &plan.destination.path)
    else {
        return Ok(());
    };

    for operation in &plan.operations {
        let Some(source_path) = operation.source_path.local_path() else {
            continue;
        };
        let Some(container) = direct_source_container(source_path, source_root) else {
            continue;
        };
        if container.as_path() != source_root.as_path()
            && (paths_equivalent(destination, &container)
                || path_is_same_or_descendant(destination, &container))
        {
            return Err(StorageError::InvalidPath {
                path: relative_local_path(destination, source_root),
                reason: "the destination overlaps a selected source container",
            });
        }
    }
    Ok(())
}

fn direct_source_container(path: &Path, root: &Path) -> Option<PathBuf> {
    let relative = path.strip_prefix(root).ok()?;
    let first = relative.components().next()?;
    match first {
        Component::Normal(name) => Some(root.join(name)),
        _ => None,
    }
}

fn copy_local_file(
    source: &Path,
    destination: &Path,
    expected_size: u64,
    on_progress: &mut dyn FnMut(u64),
) -> Result<(), StorageError> {
    let mut source_file = File::open(source).map_err(|error| StorageError::Local {
        operation: "opening the local source video",
        message: error.to_string(),
    })?;
    let mut destination_file = OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(destination)
        .map_err(|error| StorageError::Local {
            operation: "opening the local temporary video",
            message: error.to_string(),
        })?;
    let mut buffer = vec![0_u8; LOCAL_TRANSFER_BUFFER_SIZE];
    let mut copied = 0_u64;
    loop {
        let read = source_file
            .read(&mut buffer)
            .map_err(|error| StorageError::Local {
                operation: "reading a local source video",
                message: error.to_string(),
            })?;
        if read == 0 {
            break;
        }
        destination_file
            .write_all(&buffer[..read])
            .map_err(|error| StorageError::Local {
                operation: "writing a local destination video",
                message: error.to_string(),
            })?;
        copied = copied.saturating_add(read as u64);
        on_progress(copied.min(expected_size));
    }
    destination_file
        .flush()
        .map_err(|error| StorageError::Local {
            operation: "flushing a local destination video",
            message: error.to_string(),
        })?;

    if copied != expected_size
        || fs::metadata(destination)
            .map(|metadata| metadata.len())
            .ok()
            != Some(expected_size)
    {
        return Err(StorageError::CopyVerification {
            path: destination.to_string_lossy().into_owned(),
        });
    }
    Ok(())
}

fn validate_local_destination_parent(path: &Path) -> Result<(), StorageError> {
    let Some(parent) = path.parent() else {
        return Err(StorageError::InvalidPath {
            path: path.to_string_lossy().into_owned(),
            reason: "the destination has no parent directory",
        });
    };
    let metadata = fs::symlink_metadata(parent).map_err(|error| StorageError::Local {
        operation: "validating the local destination parent",
        message: error.to_string(),
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(StorageError::InvalidPath {
            path: parent.to_string_lossy().into_owned(),
            reason: "the destination parent must be a real directory",
        });
    }
    Ok(())
}

fn paths_equivalent(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (fs::canonicalize(left), fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn path_is_same_or_descendant(path: &Path, ancestor: &Path) -> bool {
    paths_equivalent(path, ancestor) || path.strip_prefix(ancestor).is_ok()
}

fn relative_local_path(path: &Path, root: &Path) -> String {
    if let Ok(relative) = path.strip_prefix(root) {
        return if relative.as_os_str().is_empty() {
            ".".to_owned()
        } else {
            relative.to_string_lossy().into_owned()
        };
    }

    let path_components = path.components().collect::<Vec<_>>();
    let root_components = root.components().collect::<Vec<_>>();
    let common = path_components
        .iter()
        .zip(root_components.iter())
        .take_while(|(left, right)| left == right)
        .count();
    if common == 0 {
        return path.to_string_lossy().into_owned();
    }
    let mut relative = PathBuf::new();
    for component in root_components.iter().skip(common) {
        if matches!(component, Component::Prefix(_) | Component::RootDir) {
            return path.to_string_lossy().into_owned();
        }
        relative.push("..");
    }
    for component in path_components.iter().skip(common) {
        relative.push(component.as_os_str());
    }
    relative.to_string_lossy().into_owned()
}

fn relative_s3_key(key: &str, root: &str) -> String {
    if key == root {
        return ".".to_owned();
    }
    if root.is_empty() {
        return if key.is_empty() {
            ".".to_owned()
        } else {
            key.to_owned()
        };
    }
    key.strip_prefix(&format!("{root}/"))
        .unwrap_or(key)
        .to_owned()
}

fn join_s3_key(prefix: &str, child: &str) -> String {
    if prefix.is_empty() {
        child.to_owned()
    } else if child.is_empty() {
        prefix.to_owned()
    } else {
        format!("{prefix}/{child}")
    }
}

fn normalize_s3_key(value: &str) -> Result<String, StorageError> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(String::new());
    }
    if value.starts_with('/') || value.contains('\\') || value.chars().any(char::is_control) {
        return Err(StorageError::InvalidPath {
            path: sanitize_display(value),
            reason: "an S3 path must be a relative slash-separated prefix",
        });
    }
    let mut components = Vec::new();
    for component in value.split('/') {
        if component.is_empty() || component == "." || component == ".." {
            if component == "." || component == ".." {
                return Err(StorageError::InvalidPath {
                    path: sanitize_display(value),
                    reason: "an S3 path cannot contain dot path components",
                });
            }
            continue;
        }
        components.push(component);
    }
    Ok(components.join("/"))
}

fn is_safe_s3_relative_path(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('/')
        && !value.contains('\\')
        && !value.chars().any(char::is_control)
        && value
            .split('/')
            .all(|component| !component.is_empty() && component != "." && component != "..")
}

fn sanitize_display(value: &str) -> String {
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
    use tempfile::tempdir;

    #[test]
    fn storage_paths_join_generated_names_without_changing_metadata() {
        let path = StoragePath::S3("library/movies".to_owned())
            .join_filename("550__S__MOVIE__S__Title.mkv")
            .unwrap();
        assert_eq!(
            path.s3_key(),
            Some("library/movies/550__S__MOVIE__S__Title.mkv")
        );
    }

    #[test]
    fn s3_paths_reject_escape_components() {
        assert!(normalize_s3_key("library/../outside").is_err());
        assert!(normalize_s3_key("/absolute").is_err());
        assert!(normalize_s3_key("library\\outside").is_err());
    }

    #[test]
    fn local_relative_paths_are_display_only() {
        let root = Path::new("/media/input");
        let child = Path::new("/media/input/nested/movie.mkv");
        assert_eq!(relative_local_path(child, root), "nested/movie.mkv");
    }

    #[test]
    fn local_backend_publishes_a_verified_copy_without_replacing_data() {
        let directory = tempdir().unwrap();
        let source = directory.path().join("source.mkv");
        let destination_directory = directory.path().join("out");
        fs::create_dir(&destination_directory).unwrap();
        fs::write(&source, b"video bytes").unwrap();
        let destination = destination_directory.join("organized.mkv");
        let backend = LocalStorage::new();
        let source_path = StoragePath::Local(source.clone());
        let destination_path = StoragePath::Local(destination.clone());
        let snapshot = backend.snapshot(&source_path).unwrap();
        let mut progress = Vec::new();

        backend
            .transfer_within(&source_path, &destination_path, &snapshot, &mut |bytes| {
                progress.push(bytes)
            })
            .unwrap();

        assert_eq!(fs::read(&source).unwrap(), b"video bytes");
        assert_eq!(fs::read(destination).unwrap(), b"video bytes");
        assert!(!progress.is_empty());
    }
}
