use std::{
    env, fmt,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

use crate::{
    error::{AppResult, ConfigError},
    ui::{InteractiveUi, MessageLevel},
};

/// The initial language requested for TMDB metadata.
pub const DEFAULT_TMDB_LANGUAGE: &str = "pt-BR";

/// The directory created in the current user's home directory for application configuration.
pub const CONFIG_DIRECTORY_NAME: &str = ".title-tmdb-file";

/// The JSON file containing the user's TMDB configuration.
pub const CONFIG_FILE_NAME: &str = "config.json";

/// Startup values collected before the filesystem workflow begins.
///
/// The API key deliberately has a redacted `Debug` implementation. The value is kept in memory
/// for the current process and is exposed through a narrow accessor for the future TMDB client.
#[derive(Clone)]
pub struct StartupConfig {
    tmdb_api_key: String,
    tmdb_language: String,
}

impl StartupConfig {
    /// Creates a validated startup configuration from prompt or configuration-file values.
    pub fn new(api_key: String, language: String) -> Result<Self, ConfigError> {
        let api_key = api_key.trim().to_owned();
        if api_key.is_empty() {
            return Err(ConfigError::MissingApiKey);
        }

        let tmdb_language = normalize_language_tag(&language)?;

        Ok(Self {
            tmdb_api_key: api_key,
            tmdb_language,
        })
    }

    /// Returns the configured API key for the in-process TMDB client.
    ///
    /// Callers must not log, format, serialize, or store this value outside the configuration
    /// persistence boundary.
    pub fn tmdb_api_key(&self) -> &str {
        &self.tmdb_api_key
    }

    /// Returns the normalized TMDB metadata language.
    pub fn tmdb_language(&self) -> &str {
        &self.tmdb_language
    }
}

impl fmt::Debug for StartupConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StartupConfig")
            .field("tmdb_api_key", &"[REDACTED]")
            .field("tmdb_language", &self.tmdb_language)
            .finish()
    }
}

/// Determines whether the shared configuration wizard asks for missing values or replaces all
/// values in the saved configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigPromptMode {
    /// Ask only for absent or invalid fields during the normal workflow.
    MissingOnly,
    /// Ask for both fields so the `config` command can update them deliberately.
    ReplaceAll,
}

/// Owns the on-disk location and serialization policy for the user's configuration.
///
/// The store is intentionally separate from `StartupConfig`: the runtime type contains the
/// validated secret, while the storage type models optional fields so a partially written or
/// manually edited file can be detected without pretending it is complete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigStore {
    path: PathBuf,
}

impl ConfigStore {
    /// Creates a store at the standard per-user configuration path.
    pub fn for_current_user() -> Result<Self, ConfigError> {
        let home = home_directory().ok_or(ConfigError::HomeDirectoryUnavailable)?;
        Ok(Self::from_path(
            home.join(CONFIG_DIRECTORY_NAME).join(CONFIG_FILE_NAME),
        ))
    }

    /// Creates a store at an explicit path.
    ///
    /// This constructor is also used by tests so they never need to read or modify the real user
    /// configuration.
    pub fn from_path(path: PathBuf) -> Self {
        Self { path }
    }

    /// Returns the path used by this store without exposing any credential value.
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn load(&self) -> Result<StoredConfig, ConfigError> {
        let contents = match fs::read_to_string(&self.path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(StoredConfig::default());
            }
            Err(source) => {
                return Err(ConfigError::Read {
                    path: self.path.clone(),
                    source,
                });
            }
        };

        serde_json::from_str(&contents).map_err(|source| ConfigError::InvalidFile {
            path: self.path.clone(),
            source,
        })
    }

    fn save(&self, config: &StartupConfig) -> Result<(), ConfigError> {
        let directory = self
            .path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let directory_existed = directory.exists();

        fs::create_dir_all(directory).map_err(|source| ConfigError::CreateDirectory {
            path: directory.to_owned(),
            source,
        })?;

        // The directory is private when this application creates it. Existing directory
        // permissions are left alone so an explicitly managed home-directory layout is not
        // unexpectedly rewritten.
        #[cfg(unix)]
        if !directory_existed {
            fs::set_permissions(directory, fs::Permissions::from_mode(0o700)).map_err(
                |source| ConfigError::CreateDirectory {
                    path: directory.to_owned(),
                    source,
                },
            )?;
        }

        // Tighten an existing file before truncating it. New files receive the same mode through
        // OpenOptionsExt below, preventing the API key from being created as a world-readable
        // file on Unix-like systems.
        #[cfg(unix)]
        if self.path.exists() {
            fs::set_permissions(&self.path, fs::Permissions::from_mode(0o600)).map_err(
                |source| ConfigError::Write {
                    path: self.path.clone(),
                    source,
                },
            )?;
        }

        let mut options = OpenOptions::new();
        options.create(true).truncate(true).write(true);
        #[cfg(unix)]
        options.mode(0o600);

        let mut file = options
            .open(&self.path)
            .map_err(|source| ConfigError::Write {
                path: self.path.clone(),
                source,
            })?;

        #[cfg(unix)]
        fs::set_permissions(&self.path, fs::Permissions::from_mode(0o600)).map_err(|source| {
            ConfigError::Write {
                path: self.path.clone(),
                source,
            }
        })?;

        let stored = StoredConfig {
            tmdb_api_key: Some(config.tmdb_api_key.clone()),
            tmdb_language: Some(config.tmdb_language.clone()),
        };

        serde_json::to_writer_pretty(&mut file, &stored).map_err(ConfigError::Serialize)?;
        file.write_all(b"\n")
            .and_then(|_| file.sync_all())
            .map_err(|source| ConfigError::Write {
                path: self.path.clone(),
                source,
            })?;

        Ok(())
    }
}

/// Runs the shared interactive configuration flow and persists a complete configuration.
pub fn configure_interactively<U: InteractiveUi>(
    ui: &mut U,
    store: &ConfigStore,
    mode: ConfigPromptMode,
) -> AppResult<Option<StartupConfig>> {
    let stored = match store.load() {
        Ok(config) => config,
        Err(ConfigError::InvalidFile { .. }) if mode == ConfigPromptMode::ReplaceAll => {
            ui.show_message(
                MessageLevel::Warning,
                "The existing configuration file is invalid and will be replaced.",
            )?;
            StoredConfig::default()
        }
        Err(error) => return Err(error.into()),
    };

    let prompt_api_key = mode == ConfigPromptMode::ReplaceAll || !stored.has_api_key();
    let prompt_language = mode == ConfigPromptMode::ReplaceAll || !stored.has_language();
    let should_persist = mode == ConfigPromptMode::ReplaceAll || prompt_api_key || prompt_language;
    let total_steps = (if prompt_api_key { 1 } else { 0 }) + (if prompt_language { 1 } else { 0 });

    let api_key_default = stored.api_key_default().or_else(tmdb_api_key_default);
    let language_default = stored.language_default();

    let mut api_key = if prompt_api_key {
        None
    } else {
        stored.api_key_default()
    };
    let mut language = if prompt_language {
        None
    } else {
        stored
            .tmdb_language
            .as_deref()
            .and_then(|value| normalize_language_tag(value).ok())
    };

    loop {
        if api_key.is_none() && prompt_api_key {
            let current_step = 1;
            ui.show_step(current_step, total_steps, "TMDB API key")?;
            let Some(value) = ui.ask_masked_secret("TMDB API key", api_key_default.as_deref())?
            else {
                return Ok(None);
            };
            api_key = Some(value);
        }

        if language.is_none() && prompt_language {
            let current_step = if prompt_api_key { 2 } else { 1 };
            ui.show_step(current_step, total_steps, "TMDB metadata language")?;
            let Some(value) =
                ui.ask_text("TMDB metadata language", Some(language_default.as_str()))?
            else {
                return Ok(None);
            };
            language = Some(value);
        }

        let (Some(api_key_value), Some(language_value)) = (api_key.clone(), language.clone())
        else {
            // This branch is reachable only if a caller changes the prompt policy without
            // providing a value. Keep the loop explicit rather than introducing an unchecked
            // unwrap at the secret boundary.
            continue;
        };

        match StartupConfig::new(api_key_value, language_value) {
            Ok(config) => {
                if should_persist {
                    store.save(&config)?;
                }
                return Ok(Some(config));
            }
            Err(error @ ConfigError::MissingApiKey) => {
                api_key = None;
                ui.show_message(MessageLevel::Error, &error.to_string())?;
            }
            Err(error @ ConfigError::InvalidLanguage) => {
                language = None;
                ui.show_message(MessageLevel::Error, &error.to_string())?;
            }
            Err(error) => return Err(error.into()),
        }
    }
}

/// Reads an optional API-key default without printing or validating the secret.
pub fn tmdb_api_key_default() -> Option<String> {
    env::var("TMDB_API_KEY")
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

/// Reads the optional language default, falling back to the documented initial locale.
pub fn tmdb_language_default() -> String {
    env::var("TMDB_LANGUAGE")
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_TMDB_LANGUAGE.to_owned())
}

#[cfg(windows)]
fn home_directory() -> Option<PathBuf> {
    env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .or_else(|| {
            let drive = env::var_os("HOMEDRIVE")?;
            let path = env::var_os("HOMEPATH")?;
            Some(PathBuf::from(drive).join(path))
        })
        .filter(|path| !path.as_os_str().is_empty())
}

#[cfg(not(windows))]
fn home_directory() -> Option<PathBuf> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
}

/// Normalizes a plausible BCP-47-style locale for use in TMDB query parameters.
///
/// Full support for TMDB's language catalog belongs to the TMDB integration task. This boundary
/// only rejects empty, malformed, or whitespace-containing values and canonicalizes common
/// language/region casing.
pub fn normalize_language_tag(value: &str) -> Result<String, ConfigError> {
    let value = value.trim();
    let mut parts = value.split('-');
    let language = parts.next().ok_or(ConfigError::InvalidLanguage)?;

    if !(2..=3).contains(&language.len())
        || !language.bytes().all(|byte| byte.is_ascii_alphabetic())
    {
        return Err(ConfigError::InvalidLanguage);
    }

    let mut normalized = vec![language.to_ascii_lowercase()];
    for (index, part) in parts.enumerate() {
        if part.is_empty()
            || part.len() > 8
            || !part.bytes().all(|byte| byte.is_ascii_alphanumeric())
        {
            return Err(ConfigError::InvalidLanguage);
        }

        let normalized_part =
            if part.len() == 4 && part.bytes().all(|byte| byte.is_ascii_alphabetic()) {
                let mut characters = part.chars();
                let first = characters
                    .next()
                    .expect("part length was checked before casing");
                format!(
                    "{}{}",
                    first.to_ascii_uppercase(),
                    characters.as_str().to_ascii_lowercase()
                )
            } else if part.len() == 2 && part.bytes().all(|byte| byte.is_ascii_alphabetic()) {
                part.to_ascii_uppercase()
            } else {
                part.to_ascii_lowercase()
            };

        normalized.push(normalized_part);

        // BCP-47 allows extension/private-use sections, but accepting arbitrary long chains here
        // would make local validation less useful. Keep the startup boundary intentionally small.
        if index >= 3 {
            return Err(ConfigError::InvalidLanguage);
        }
    }

    Ok(normalized.join("-"))
}

#[derive(Clone, Default, Deserialize, Serialize)]
struct StoredConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tmdb_api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tmdb_language: Option<String>,
}

impl StoredConfig {
    fn has_api_key(&self) -> bool {
        self.api_key_default().is_some()
    }

    fn has_language(&self) -> bool {
        self.tmdb_language
            .as_deref()
            .and_then(|value| normalize_language_tag(value).ok())
            .is_some()
    }

    fn api_key_default(&self) -> Option<String> {
        self.tmdb_api_key
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    }

    fn language_default(&self) -> String {
        self.tmdb_language
            .as_deref()
            .and_then(|value| normalize_language_tag(value).ok())
            .or_else(|| normalize_language_tag(&tmdb_language_default()).ok())
            .unwrap_or_else(|| DEFAULT_TMDB_LANGUAGE.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn startup_config_redacts_the_api_key_in_debug_output() {
        let config = StartupConfig::new("secret-value".to_owned(), "pt-br".to_owned()).unwrap();
        let debug = format!("{config:?}");

        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("secret-value"));
        assert_eq!(config.tmdb_language(), "pt-BR");
        assert_eq!(config.tmdb_api_key(), "secret-value");
    }

    #[test]
    fn language_tags_are_canonicalized_for_common_forms() {
        assert_eq!(normalize_language_tag(" PT-br ").unwrap(), "pt-BR");
        assert_eq!(normalize_language_tag("zh-hant-tw").unwrap(), "zh-Hant-TW");
        assert_eq!(normalize_language_tag("es-419").unwrap(), "es-419");
    }

    #[test]
    fn invalid_startup_values_are_rejected_without_echoing_input() {
        assert!(matches!(
            StartupConfig::new("   ".to_owned(), "pt-BR".to_owned()),
            Err(ConfigError::MissingApiKey)
        ));
        assert!(matches!(
            normalize_language_tag("not a locale"),
            Err(ConfigError::InvalidLanguage)
        ));
        assert!(matches!(
            normalize_language_tag("pt_BR"),
            Err(ConfigError::InvalidLanguage)
        ));
    }

    #[test]
    fn missing_config_files_load_as_empty_configuration() {
        let directory = tempdir().unwrap();
        let store = ConfigStore::from_path(directory.path().join(CONFIG_FILE_NAME));

        let loaded = store.load().unwrap();

        assert!(!loaded.has_api_key());
        assert!(!loaded.has_language());
    }

    #[test]
    fn saved_configuration_round_trips_through_the_documented_json_schema() {
        let directory = tempdir().unwrap();
        let store = ConfigStore::from_path(
            directory
                .path()
                .join(CONFIG_DIRECTORY_NAME)
                .join(CONFIG_FILE_NAME),
        );
        let config = StartupConfig::new("secret-value".to_owned(), "pt-br".to_owned()).unwrap();

        store.save(&config).unwrap();

        let contents = fs::read_to_string(store.path()).unwrap();
        assert!(contents.contains("\"tmdb_api_key\": \"secret-value\""));
        assert!(contents.contains("\"tmdb_language\": \"pt-BR\""));
        let loaded = store.load().unwrap();
        assert_eq!(loaded.api_key_default().as_deref(), Some("secret-value"));
        assert_eq!(loaded.language_default(), "pt-BR");
    }

    #[cfg(unix)]
    #[test]
    fn saved_configuration_restricts_the_file_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempdir().unwrap();
        let store = ConfigStore::from_path(
            directory
                .path()
                .join(CONFIG_DIRECTORY_NAME)
                .join(CONFIG_FILE_NAME),
        );
        let config = StartupConfig::new("secret-value".to_owned(), "pt-BR".to_owned()).unwrap();

        store.save(&config).unwrap();

        let mode = fs::metadata(store.path()).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let directory_mode = fs::metadata(store.path().parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(directory_mode, 0o700);
    }
}
