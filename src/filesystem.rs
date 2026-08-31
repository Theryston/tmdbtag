//! Filesystem discovery and path validation for the interactive workflow.
//!
//! This module deliberately stops at selection. It does not create, rename, copy, delete, or
//! move anything. Later planning and execution stages receive the exact paths returned here and
//! are responsible for revalidating them immediately before a confirmed mutation.

use std::{
    cmp::Ordering,
    env, fs, io,
    path::{Component, Path, PathBuf},
};

use crate::{
    domain::{DestinationSelection, SourceFolder, SourceRoot, VideoExtension, VideoFile},
    error::FilesystemError,
};

/// Backward-compatible access to the shared video-extension policy.
pub use crate::domain::VIDEO_EXTENSIONS;

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
    discover_video_files_in_directory(source_folder.path(), true, &mut files, &mut warnings)?;

    files.sort_by(|left, right| compare_paths(left.path(), right.path()));
    warnings.sort_by(|left, right| {
        compare_paths(left.path(), right.path()).then_with(|| left.reason().cmp(right.reason()))
    });
    Ok(Discovery::new(files, warnings))
}

fn discover_video_files_in_directory(
    directory: &Path,
    is_root: bool,
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
            discover_video_files_in_directory(&path, false, files, warnings)?;
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

/// Returns whether a path has one of the recognized video filename extensions.
pub fn has_video_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| VideoExtension::parse(extension).is_ok())
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
}
