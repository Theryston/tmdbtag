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

/// The standard AWS S3 endpoint used when no custom endpoint is supplied.
pub const DEFAULT_S3_ENDPOINT: &str = "https://s3.amazonaws.com";

/// The directory created in the current user's home directory for application configuration.
pub const CONFIG_DIRECTORY_NAME: &str = ".tmdbtag";

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

/// Validated credentials and connection settings for one named S3 bucket.
///
/// S3 profiles are persisted in a user-owned catalog. A profile deliberately contains connection
/// details only; source and destination prefixes belong to one organization run and are never
/// persisted with the credentials. Secret values are kept behind accessors and are redacted from
/// debug output.
#[derive(Clone, PartialEq, Eq)]
pub struct S3Config {
    name: String,
    access_key: String,
    secret_key: String,
    bucket: String,
    endpoint: String,
    region: String,
}

impl S3Config {
    /// Creates a validated named S3 configuration from interactive or persisted values.
    pub fn new(
        name: String,
        access_key: String,
        secret_key: String,
        bucket: String,
        endpoint: String,
        region: String,
    ) -> Result<Self, ConfigError> {
        let name = required_s3_value(name, ConfigError::MissingS3Name);
        let access_key = required_s3_value(access_key, ConfigError::MissingS3AccessKey);
        let secret_key = required_s3_value(secret_key, ConfigError::MissingS3SecretKey);
        let bucket = required_s3_value(bucket, ConfigError::MissingS3Bucket);
        let region = required_s3_value(region, ConfigError::MissingS3Region);

        let (name, access_key, secret_key, bucket, region) =
            (name?, access_key?, secret_key?, bucket?, region?);
        let endpoint = endpoint.trim().to_owned();
        let endpoint = if endpoint.is_empty() {
            DEFAULT_S3_ENDPOINT.to_owned()
        } else {
            endpoint
        };

        if name.chars().any(char::is_control) {
            return Err(ConfigError::InvalidS3Name);
        }
        if bucket.chars().any(|character| {
            character.is_whitespace() || character.is_control() || character == '/'
        }) {
            return Err(ConfigError::InvalidS3Bucket);
        }
        if region
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
        {
            return Err(ConfigError::InvalidS3Region);
        }

        let parsed_endpoint =
            reqwest::Url::parse(&endpoint).map_err(|_| ConfigError::InvalidS3Endpoint)?;
        if !matches!(parsed_endpoint.scheme(), "http" | "https")
            || parsed_endpoint.host_str().is_none()
            || !parsed_endpoint.username().is_empty()
            || parsed_endpoint.password().is_some()
            || parsed_endpoint.query().is_some()
            || parsed_endpoint.fragment().is_some()
        {
            return Err(ConfigError::InvalidS3Endpoint);
        }

        let endpoint = endpoint.trim_end_matches('/').to_owned();

        Ok(Self {
            name,
            access_key,
            secret_key,
            bucket,
            endpoint,
            region,
        })
    }

    /// Returns the user-facing name used to identify this saved S3 bucket.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the access key used to create the in-memory S3 client.
    pub fn access_key(&self) -> &str {
        &self.access_key
    }

    /// Returns the secret key used to create the in-memory S3 client.
    pub fn secret_key(&self) -> &str {
        &self.secret_key
    }

    /// Returns the configured bucket name.
    pub fn bucket(&self) -> &str {
        &self.bucket
    }

    /// Returns the endpoint URL used by the S3-compatible client.
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Returns the signing region.
    pub fn region(&self) -> &str {
        &self.region
    }

    /// Returns a credential-free label suitable for profile selectors and diagnostics.
    pub fn display_label(&self) -> String {
        format!("{} · bucket {} · {}", self.name, self.bucket, self.region)
    }
}

impl fmt::Debug for S3Config {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("S3Config")
            .field("name", &self.name)
            .field("access_key", &"[REDACTED]")
            .field("secret_key", &"[REDACTED]")
            .field("bucket", &self.bucket)
            .field("endpoint", &self.endpoint)
            .field("region", &self.region)
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
    /// Recollect only the API key after TMDB rejects the saved credential.
    RepairApiKey,
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

    /// Loads every complete, valid S3 bucket saved in the configuration file.
    pub fn load_s3_profiles(&self) -> Result<Vec<S3Config>, ConfigError> {
        Ok(self.load()?.s3_profiles())
    }

    fn save(&self, config: &StartupConfig) -> Result<(), ConfigError> {
        let stored = self.load()?;
        let directory = self
            .path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        self.write_stored(
            StartupConfigValues {
                tmdb_api_key: Some(config.tmdb_api_key.clone()),
                tmdb_language: Some(config.tmdb_language.clone()),
                // Preserve every S3 profile so an unrelated TMDB update cannot discard storage
                // connections.
                s3: stored.s3,
            },
            directory,
        )
    }

    /// Adds one named S3 bucket while retaining TMDB settings and existing buckets.
    pub fn add_s3_profile(&self, s3: &S3Config) -> Result<(), ConfigError> {
        let stored = self.load()?;
        if stored.s3.iter().any(|profile| {
            profile
                .logical_name()
                .is_some_and(|name| name.trim().eq_ignore_ascii_case(s3.name()))
        }) {
            return Err(ConfigError::DuplicateS3Name {
                name: s3.name().to_owned(),
            });
        }

        let directory = self
            .path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let mut profiles = stored.s3;
        profiles.push(StoredS3Config::from_config(s3));
        self.write_stored(
            StartupConfigValues {
                tmdb_api_key: stored.tmdb_api_key,
                tmdb_language: stored.tmdb_language,
                s3: profiles,
            },
            directory,
        )
    }

    /// Removes one named S3 bucket and reports whether a profile was removed.
    pub fn remove_s3_profile(&self, name: &str) -> Result<bool, ConfigError> {
        let stored = self.load()?;
        let mut profiles = stored.s3;
        let original_count = profiles.len();
        profiles.retain(|profile| {
            !profile
                .logical_name()
                .is_some_and(|profile_name| profile_name.trim().eq_ignore_ascii_case(name.trim()))
        });
        if profiles.len() == original_count {
            return Ok(false);
        }

        let directory = self
            .path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        self.write_stored(
            StartupConfigValues {
                tmdb_api_key: stored.tmdb_api_key,
                tmdb_language: stored.tmdb_language,
                s3: profiles,
            },
            directory,
        )?;
        Ok(true)
    }

    fn write_stored(
        &self,
        values: StartupConfigValues,
        directory: &Path,
    ) -> Result<(), ConfigError> {
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
            tmdb_api_key: values.tmdb_api_key,
            tmdb_language: values.tmdb_language,
            s3: values.s3,
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

    let prompt_api_key = match mode {
        ConfigPromptMode::MissingOnly => !stored.has_api_key(),
        ConfigPromptMode::ReplaceAll | ConfigPromptMode::RepairApiKey => true,
    };
    let prompt_language = match mode {
        ConfigPromptMode::MissingOnly => !stored.has_language(),
        ConfigPromptMode::ReplaceAll => true,
        ConfigPromptMode::RepairApiKey => false,
    };
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

/// Collects and persists one new named S3 bucket through the shared interactive flow.
pub fn configure_s3_interactively<U: InteractiveUi>(
    ui: &mut U,
    store: &ConfigStore,
) -> AppResult<Option<S3Config>> {
    // Parse the existing file before collecting credentials. An explicit add command must not
    // overwrite a malformed configuration file or silently discard the other profiles.
    let _ = store.load()?;

    let total_steps = 6;
    let mut name = None;
    let mut access_key = None;
    let mut secret_key = None;
    let mut bucket = None;
    let mut endpoint = None;
    let mut region = None;

    loop {
        if name.is_none() {
            ui.show_step(1, total_steps, "S3 storage name")?;
            let Some(value) = ui.ask_text("S3 storage name", None)? else {
                return Ok(None);
            };
            name = Some(value);
        }

        if access_key.is_none() {
            ui.show_step(2, total_steps, "S3 access key")?;
            let Some(value) = ui.ask_text("S3 access key", None)? else {
                return Ok(None);
            };
            access_key = Some(value);
        }

        if secret_key.is_none() {
            ui.show_step(3, total_steps, "S3 secret key")?;
            let Some(value) = ui.ask_masked_secret("S3 secret key", None)? else {
                return Ok(None);
            };
            secret_key = Some(value);
        }

        if bucket.is_none() {
            ui.show_step(4, total_steps, "S3 bucket")?;
            let Some(value) = ui.ask_text("S3 bucket name", None)? else {
                return Ok(None);
            };
            bucket = Some(value);
        }

        if endpoint.is_none() {
            ui.show_step(5, total_steps, "S3 endpoint")?;
            let Some(value) = ui.ask_text(
                "S3 endpoint URL (optional; press Enter for the AWS default)",
                Some(DEFAULT_S3_ENDPOINT),
            )?
            else {
                return Ok(None);
            };
            endpoint = Some(value);
        }

        if region.is_none() {
            ui.show_step(6, total_steps, "S3 region")?;
            let Some(value) = ui.ask_text("S3 region", None)? else {
                return Ok(None);
            };
            region = Some(value);
        }

        match S3Config::new(
            name.clone().unwrap_or_default(),
            access_key.clone().unwrap_or_default(),
            secret_key.clone().unwrap_or_default(),
            bucket.clone().unwrap_or_default(),
            endpoint.clone().unwrap_or_default(),
            region.clone().unwrap_or_default(),
        ) {
            Ok(config) => match store.add_s3_profile(&config) {
                Ok(()) => return Ok(Some(config)),
                Err(error @ ConfigError::DuplicateS3Name { .. }) => {
                    ui.show_message(MessageLevel::Error, &error.to_string())?;
                    name = None;
                }
                Err(error) => return Err(error.into()),
            },
            Err(error) => {
                ui.show_message(MessageLevel::Error, &error.to_string())?;
                // Recollect the complete profile after a validation failure. This avoids
                // retaining a partially invalid value and keeps the prompt policy predictable
                // for both the normal workflow bootstrap and `storage add`.
                name = None;
                access_key = None;
                secret_key = None;
                bucket = None;
                endpoint = None;
                region = None;
            }
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

fn required_s3_value(value: String, missing: ConfigError) -> Result<String, ConfigError> {
    let value = value.trim().to_owned();
    if value.is_empty() {
        Err(missing)
    } else {
        Ok(value)
    }
}

#[derive(Clone, Default, Deserialize, Serialize)]
struct StartupConfigValues {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tmdb_api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tmdb_language: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    s3: Vec<StoredS3Config>,
}

#[derive(Clone, Default, Deserialize, Serialize)]
struct StoredConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tmdb_api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tmdb_language: Option<String>,
    #[serde(
        default,
        deserialize_with = "deserialize_s3_profiles",
        skip_serializing_if = "Vec::is_empty"
    )]
    s3: Vec<StoredS3Config>,
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

    fn s3_profiles(&self) -> Vec<S3Config> {
        self.s3
            .iter()
            .filter_map(|stored| {
                let bucket = stored.bucket.clone()?;
                let name = stored.name.clone().unwrap_or_else(|| bucket.clone());
                S3Config::new(
                    name,
                    stored.access_key.clone()?,
                    stored.secret_key.clone()?,
                    bucket,
                    stored.endpoint.clone().unwrap_or_default(),
                    stored.region.clone()?,
                )
                .ok()
            })
            .collect()
    }
}

#[derive(Clone, Default, Deserialize, Serialize)]
struct StoredS3Config {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    access_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    secret_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    bucket: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    endpoint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    region: Option<String>,
}

impl StoredS3Config {
    fn logical_name(&self) -> Option<&str> {
        self.name.as_deref().or(self.bucket.as_deref())
    }

    fn from_config(config: &S3Config) -> Self {
        Self {
            name: Some(config.name.clone()),
            access_key: Some(config.access_key.clone()),
            secret_key: Some(config.secret_key.clone()),
            bucket: Some(config.bucket.clone()),
            endpoint: Some(config.endpoint.clone()),
            region: Some(config.region.clone()),
        }
    }
}

fn deserialize_s3_profiles<'de, D>(deserializer: D) -> Result<Vec<StoredS3Config>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    match value {
        serde_json::Value::Null => Ok(Vec::new()),
        serde_json::Value::Array(value) => serde_json::from_value(serde_json::Value::Array(value))
            .map_err(serde::de::Error::custom),
        serde_json::Value::Object(value) => {
            serde_json::from_value(serde_json::Value::Object(value))
                .map(|profile| vec![profile])
                .map_err(serde::de::Error::custom)
        }
        _ => Err(serde::de::Error::custom(
            "the s3 configuration must be an object or an array",
        )),
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

    #[test]
    fn saved_s3_configuration_round_trips_without_losing_tmdb_values() {
        let directory = tempdir().unwrap();
        let store = ConfigStore::from_path(
            directory
                .path()
                .join(CONFIG_DIRECTORY_NAME)
                .join(CONFIG_FILE_NAME),
        );
        let tmdb = StartupConfig::new("tmdb-secret".to_owned(), "en-US".to_owned()).unwrap();
        let s3 = S3Config::new(
            "primary".to_owned(),
            "access".to_owned(),
            "secret".to_owned(),
            "bucket".to_owned(),
            "https://s3.example.test".to_owned(),
            "us-east-1".to_owned(),
        )
        .unwrap();

        store.save(&tmdb).unwrap();
        store.add_s3_profile(&s3).unwrap();

        let loaded = store.load_s3_profiles().unwrap().pop().unwrap();
        assert_eq!(loaded.name(), "primary");
        assert_eq!(loaded.access_key(), "access");
        assert_eq!(loaded.secret_key(), "secret");
        assert_eq!(loaded.bucket(), "bucket");
        assert_eq!(loaded.endpoint(), "https://s3.example.test");
        assert_eq!(loaded.region(), "us-east-1");
        let stored = store.load().unwrap();
        assert_eq!(stored.api_key_default().as_deref(), Some("tmdb-secret"));
        assert_eq!(stored.language_default(), "en-US");
    }

    #[test]
    fn s3_catalog_supports_multiple_buckets_and_removes_only_the_requested_bucket() {
        let directory = tempdir().unwrap();
        let store = ConfigStore::from_path(
            directory
                .path()
                .join(CONFIG_DIRECTORY_NAME)
                .join(CONFIG_FILE_NAME),
        );
        let first = S3Config::new(
            "primary".to_owned(),
            "access-one".to_owned(),
            "secret-one".to_owned(),
            "bucket-one".to_owned(),
            String::new(),
            "us-east-1".to_owned(),
        )
        .unwrap();
        let second = S3Config::new(
            "archive".to_owned(),
            "access-two".to_owned(),
            "secret-two".to_owned(),
            "bucket-two".to_owned(),
            "https://s3.example.test".to_owned(),
            "eu-west-1".to_owned(),
        )
        .unwrap();

        store.add_s3_profile(&first).unwrap();
        store.add_s3_profile(&second).unwrap();

        let contents = fs::read_to_string(store.path()).unwrap();
        assert!(contents.contains("\"s3\": ["));
        assert!(!contents.contains("base_path"));

        let profiles = store.load_s3_profiles().unwrap();
        assert_eq!(profiles.len(), 2);
        assert_eq!(profiles[0].name(), "primary");
        assert_eq!(profiles[1].name(), "archive");

        assert!(matches!(
            store.add_s3_profile(&first),
            Err(ConfigError::DuplicateS3Name { .. })
        ));
        assert!(store.remove_s3_profile("PRIMARY").unwrap());
        assert!(!store.remove_s3_profile("missing").unwrap());

        let remaining = store.load_s3_profiles().unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].name(), "archive");
    }

    #[test]
    fn s3_configuration_debug_output_redacts_credentials() {
        let config = S3Config::new(
            "primary".to_owned(),
            "access-secret".to_owned(),
            "secret-value".to_owned(),
            "bucket".to_owned(),
            "https://s3.example.test".to_owned(),
            "us-east-1".to_owned(),
        )
        .unwrap();

        let debug = format!("{config:?}");

        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("access-secret"));
        assert!(!debug.contains("secret-value"));
        assert!(debug.contains("bucket"));
    }

    #[test]
    fn invalid_s3_profile_values_are_rejected_at_the_configuration_boundary() {
        assert!(matches!(
            S3Config::new(
                "profile\nname".to_owned(),
                "access".to_owned(),
                "secret".to_owned(),
                "bucket".to_owned(),
                "https://s3.example.test".to_owned(),
                "us-east-1".to_owned(),
            ),
            Err(ConfigError::InvalidS3Name)
        ));
        assert!(matches!(
            S3Config::new(
                "profile".to_owned(),
                "access".to_owned(),
                "secret".to_owned(),
                "bucket/name".to_owned(),
                "https://s3.example.test".to_owned(),
                "us-east-1".to_owned(),
            ),
            Err(ConfigError::InvalidS3Bucket)
        ));
        assert!(matches!(
            S3Config::new(
                "profile".to_owned(),
                "access".to_owned(),
                "secret".to_owned(),
                "bucket".to_owned(),
                "file:///tmp/s3".to_owned(),
                "us-east-1".to_owned(),
            ),
            Err(ConfigError::InvalidS3Endpoint)
        ));
        assert!(matches!(
            S3Config::new(
                "profile".to_owned(),
                "access".to_owned(),
                "secret".to_owned(),
                "bucket".to_owned(),
                "https://user:secret@s3.example.com".to_owned(),
                "us-east-1".to_owned(),
            ),
            Err(ConfigError::InvalidS3Endpoint)
        ));
    }

    #[test]
    fn blank_s3_endpoint_uses_the_standard_aws_endpoint() {
        let config = S3Config::new(
            "profile".to_owned(),
            "access".to_owned(),
            "secret".to_owned(),
            "bucket".to_owned(),
            "   ".to_owned(),
            "us-east-1".to_owned(),
        )
        .unwrap();

        assert_eq!(config.endpoint(), DEFAULT_S3_ENDPOINT);
    }

    #[test]
    fn saved_s3_profile_without_endpoint_uses_the_standard_aws_endpoint() {
        let directory = tempdir().unwrap();
        let store = ConfigStore::from_path(
            directory
                .path()
                .join(CONFIG_DIRECTORY_NAME)
                .join(CONFIG_FILE_NAME),
        );
        fs::create_dir_all(store.path().parent().unwrap()).unwrap();
        fs::write(
            store.path(),
            r#"{
  "s3": {
    "access_key": "access",
    "secret_key": "secret",
    "bucket": "bucket",
    "base_path": "legacy/media",
    "region": "us-east-1"
  }
}"#,
        )
        .unwrap();

        let config = store.load_s3_profiles().unwrap().pop().unwrap();

        assert_eq!(config.name(), "bucket");
        assert_eq!(config.endpoint(), DEFAULT_S3_ENDPOINT);
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
