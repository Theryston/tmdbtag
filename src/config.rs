use std::{env, fmt};

use crate::error::ConfigError;

/// The initial language requested for TMDB metadata.
pub const DEFAULT_TMDB_LANGUAGE: &str = "pt-BR";

/// Startup values collected before the filesystem workflow begins.
///
/// The API key deliberately has a redacted `Debug` implementation. The value is kept only for
/// the current process and is exposed through a narrow accessor for the future TMDB client.
#[derive(Clone)]
pub struct StartupConfig {
    tmdb_api_key: String,
    tmdb_language: String,
}

impl StartupConfig {
    /// Creates a validated startup configuration from prompt values.
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
    /// Callers must not log, format, serialize, or store this value.
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

#[cfg(test)]
mod tests {
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
        assert_eq!(
            normalize_language_tag("not a locale"),
            Err(ConfigError::InvalidLanguage)
        );
        assert_eq!(
            normalize_language_tag("pt_BR"),
            Err(ConfigError::InvalidLanguage)
        );
    }
}
