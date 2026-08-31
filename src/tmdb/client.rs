use std::{fmt, io::Read, time::Duration};

use reqwest::{
    StatusCode, Url,
    blocking::{Client, Response},
    header::RETRY_AFTER,
};
use serde::de::DeserializeOwned;

use crate::{
    config::StartupConfig,
    domain::{EpisodeRef, MediaType, TmdbEpisode, TmdbId, TmdbItem, TmdbSearchPage},
    error::TmdbError,
};

use super::models;

/// The official TMDB v3 API origin.
pub const DEFAULT_BASE_URL: &str = "https://api.themoviedb.org/";

/// Finite request timeout used by normal commands.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);

const MAX_RESPONSE_BYTES: u64 = 5 * 1024 * 1024;
const MAX_SEARCH_RESULTS: usize = 20;

/// A reusable, configured TMDB API client.
///
/// The client owns the HTTP transport, API-key authentication, language query parameter, and
/// response-size/timeout policy. It never prompts, mutates the filesystem, or chooses a search
/// result for the user.
#[derive(Clone)]
pub struct TmdbClient {
    http: Client,
    api_key: String,
    language: String,
    base_url: Url,
}

impl fmt::Debug for TmdbClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TmdbClient")
            .field("api_key", &"[REDACTED]")
            .field("language", &self.language)
            .field("base_url", &self.base_url)
            .finish()
    }
}

impl TmdbClient {
    /// Creates a client for the official TMDB API origin.
    pub fn new(config: &StartupConfig) -> Result<Self, TmdbError> {
        Self::with_base_url_and_timeout(config, DEFAULT_BASE_URL, DEFAULT_TIMEOUT)
    }

    /// Creates a client with a custom origin, primarily for local mocked HTTP tests.
    pub fn with_base_url(config: &StartupConfig, base_url: &str) -> Result<Self, TmdbError> {
        Self::with_base_url_and_timeout(config, base_url, DEFAULT_TIMEOUT)
    }

    /// Creates a client with a custom origin and timeout for deterministic tests.
    pub fn with_base_url_and_timeout(
        config: &StartupConfig,
        base_url: &str,
        timeout: Duration,
    ) -> Result<Self, TmdbError> {
        let base_url = Url::parse(base_url).map_err(|_| TmdbError::InvalidBaseUrl)?;
        if !matches!(base_url.scheme(), "http" | "https")
            || base_url.host_str().is_none()
            || base_url.query().is_some()
            || base_url.fragment().is_some()
        {
            return Err(TmdbError::InvalidBaseUrl);
        }

        let http = Client::builder()
            .timeout(timeout)
            .user_agent(concat!("tmdbtag/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|error| TmdbError::ClientBuild {
                message: error.to_string(),
            })?;

        Ok(Self {
            http,
            api_key: config.tmdb_api_key().to_owned(),
            language: config.tmdb_language().to_owned(),
            base_url,
        })
    }

    /// Performs a lightweight authenticated request before the filesystem workflow begins.
    pub fn validate_credentials(&self) -> Result<(), TmdbError> {
        let _: serde_json::Value = self.get_json(
            &["3", "configuration"],
            &[],
            Operation::ValidateCredentials,
            MissingResource::Description("TMDB configuration".to_owned()),
        )?;
        Ok(())
    }

    /// Searches the movie namespace and returns the first bounded result page.
    pub fn search_movies(
        &self,
        query: &str,
    ) -> Result<Vec<crate::domain::TmdbSearchCandidate>, TmdbError> {
        Ok(self.search_movies_page(query, 1)?.results)
    }

    /// Searches the movie namespace at a one-based TMDB page.
    pub fn search_movies_page(&self, query: &str, page: u32) -> Result<TmdbSearchPage, TmdbError> {
        self.search_page(MediaType::Movie, query, page)
    }

    /// Searches the TV namespace and returns the first bounded result page.
    pub fn search_series(
        &self,
        query: &str,
    ) -> Result<Vec<crate::domain::TmdbSearchCandidate>, TmdbError> {
        Ok(self.search_series_page(query, 1)?.results)
    }

    /// Searches the TV namespace at a one-based TMDB page.
    pub fn search_series_page(&self, query: &str, page: u32) -> Result<TmdbSearchPage, TmdbError> {
        self.search_page(MediaType::Series, query, page)
    }

    /// Searches the requested TMDB namespace at a one-based page.
    pub fn search(
        &self,
        media_type: MediaType,
        query: &str,
        page: u32,
    ) -> Result<TmdbSearchPage, TmdbError> {
        self.search_page(media_type, query, page)
    }

    /// Fetches verified details for a movie or TV series.
    pub fn get_item(&self, media_type: MediaType, id: TmdbId) -> Result<TmdbItem, TmdbError> {
        match media_type {
            MediaType::Movie => self.get_movie(id),
            MediaType::Series => self.get_series(id),
        }
    }

    /// Fetches verified movie details.
    pub fn get_movie(&self, id: TmdbId) -> Result<TmdbItem, TmdbError> {
        let id_text = id.to_string();
        let response: models::MovieDetailsResponse = self.get_json(
            &["3", "movie", id_text.as_str()],
            &[],
            Operation::MovieDetails,
            MissingResource::Description(format!("movie {id}")),
        )?;
        models::map_movie_details(response, id)
    }

    /// Fetches verified TV-series details.
    pub fn get_series(&self, id: TmdbId) -> Result<TmdbItem, TmdbError> {
        let id_text = id.to_string();
        let response: models::TvDetailsResponse = self.get_json(
            &["3", "tv", id_text.as_str()],
            &[],
            Operation::SeriesDetails,
            MissingResource::Description(format!("TV series {id}")),
        )?;
        models::map_tv_details(response, id)
    }

    /// Fetches and validates one episode in a verified TV series.
    pub fn get_episode_details(
        &self,
        series_id: TmdbId,
        episode: EpisodeRef,
    ) -> Result<TmdbEpisode, TmdbError> {
        let series_text = series_id.to_string();
        let season_text = episode.season().to_string();
        let episode_text = episode.episode().to_string();
        let response: models::EpisodeDetailsResponse = self.get_json(
            &[
                "3",
                "tv",
                series_text.as_str(),
                "season",
                season_text.as_str(),
                "episode",
                episode_text.as_str(),
            ],
            &[],
            Operation::EpisodeDetails,
            MissingResource::Episode { series_id, episode },
        )?;
        models::map_episode_details(response, series_id, episode)
    }

    fn search_page(
        &self,
        media_type: MediaType,
        query: &str,
        page: u32,
    ) -> Result<TmdbSearchPage, TmdbError> {
        let query = query.trim();
        if query.is_empty() {
            return Err(TmdbError::EmptySearchQuery);
        }
        if page == 0 {
            return Err(TmdbError::InvalidSearchPage);
        }

        let page_text = page.to_string();
        let additional_params = [
            ("query", query.to_owned()),
            ("include_adult", "false".to_owned()),
            ("page", page_text),
        ];

        match media_type {
            MediaType::Movie => {
                let response: models::MovieSearchResponse = self.get_json(
                    &["3", "search", "movie"],
                    &additional_params,
                    Operation::MovieSearch,
                    MissingResource::Description("movie search".to_owned()),
                )?;
                models::map_movie_search(response, MAX_SEARCH_RESULTS)
            }
            MediaType::Series => {
                let response: models::TvSearchResponse = self.get_json(
                    &["3", "search", "tv"],
                    &additional_params,
                    Operation::SeriesSearch,
                    MissingResource::Description("TV search".to_owned()),
                )?;
                models::map_tv_search(response, MAX_SEARCH_RESULTS)
            }
        }
    }

    fn get_json<T: DeserializeOwned>(
        &self,
        segments: &[&str],
        additional_params: &[(&str, String)],
        operation: Operation,
        missing_resource: MissingResource,
    ) -> Result<T, TmdbError> {
        let url = self.url_for_segments(segments)?;
        let mut params = Vec::with_capacity(additional_params.len() + 2);
        params.push(("api_key", self.api_key.clone()));
        params.push(("language", self.language.clone()));
        params.extend(
            additional_params
                .iter()
                .map(|(name, value)| (*name, value.clone())),
        );

        let response = self
            .http
            .get(url)
            .query(&params)
            .send()
            .map_err(|error| map_request_error(operation, error))?;

        let status = response.status();
        if !status.is_success() {
            return Err(map_status_error(
                status,
                &response,
                operation,
                missing_resource,
            ));
        }

        if response
            .content_length()
            .is_some_and(|length| length > MAX_RESPONSE_BYTES)
        {
            return Err(invalid_response(
                operation,
                "the response exceeded the maximum supported size",
            ));
        }

        let mut body = Vec::new();
        response
            .take(MAX_RESPONSE_BYTES + 1)
            .read_to_end(&mut body)
            .map_err(|error| map_body_read_error(operation, error))?;
        if body.len() as u64 > MAX_RESPONSE_BYTES {
            return Err(invalid_response(
                operation,
                "the response exceeded the maximum supported size",
            ));
        }

        serde_json::from_slice(&body)
            .map_err(|error| invalid_response(operation, &error.to_string()))
    }

    fn url_for_segments(&self, segments: &[&str]) -> Result<Url, TmdbError> {
        let mut url = self.base_url.clone();
        {
            let mut path = url
                .path_segments_mut()
                .map_err(|_| TmdbError::InvalidBaseUrl)?;
            for segment in segments {
                if segment.is_empty() {
                    return Err(TmdbError::InvalidBaseUrl);
                }
                path.push(segment);
            }
        }
        Ok(url)
    }
}

#[derive(Debug, Clone, Copy)]
enum Operation {
    ValidateCredentials,
    MovieSearch,
    SeriesSearch,
    MovieDetails,
    SeriesDetails,
    EpisodeDetails,
}

impl Operation {
    const fn label(self) -> &'static str {
        match self {
            Self::ValidateCredentials => "validating credentials",
            Self::MovieSearch => "searching movies",
            Self::SeriesSearch => "searching TV series",
            Self::MovieDetails => "fetching movie details",
            Self::SeriesDetails => "fetching TV series details",
            Self::EpisodeDetails => "validating episode details",
        }
    }
}

#[derive(Debug, Clone)]
enum MissingResource {
    Description(String),
    Episode {
        series_id: TmdbId,
        episode: EpisodeRef,
    },
}

fn map_request_error(operation: Operation, error: reqwest::Error) -> TmdbError {
    if error.is_timeout() {
        TmdbError::Timeout {
            operation: operation.label().to_owned(),
        }
    } else {
        TmdbError::Network {
            operation: operation.label().to_owned(),
        }
    }
}

fn map_body_read_error(operation: Operation, error: std::io::Error) -> TmdbError {
    if error.kind() == std::io::ErrorKind::TimedOut {
        TmdbError::Timeout {
            operation: operation.label().to_owned(),
        }
    } else {
        TmdbError::Network {
            operation: operation.label().to_owned(),
        }
    }
}

fn map_status_error(
    status: StatusCode,
    response: &Response,
    operation: Operation,
    missing_resource: MissingResource,
) -> TmdbError {
    if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        return TmdbError::Authentication {
            status: status.as_u16(),
        };
    }
    if status == StatusCode::TOO_MANY_REQUESTS {
        return TmdbError::RateLimited {
            retry_after_seconds: response
                .headers()
                .get(RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.trim().parse().ok()),
        };
    }
    if status == StatusCode::NOT_FOUND {
        return match missing_resource {
            MissingResource::Description(resource) => TmdbError::NotFound { resource },
            MissingResource::Episode { series_id, episode } => TmdbError::EpisodeNotFound {
                series_id: series_id.value(),
                season: episode.season(),
                episode: episode.episode(),
            },
        };
    }
    if status.is_server_error() {
        return TmdbError::Server {
            status: status.as_u16(),
        };
    }

    TmdbError::UnexpectedStatus {
        operation: operation.label().to_owned(),
        status: status.as_u16(),
    }
}

fn invalid_response(operation: Operation, reason: &str) -> TmdbError {
    TmdbError::InvalidResponse {
        operation: operation.label().to_owned(),
        reason: reason.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::{Arc, Mutex},
        thread::{self, JoinHandle},
    };

    use super::*;
    use crate::{
        config::StartupConfig,
        domain::{EpisodeRef, MediaType, TmdbId},
    };

    #[derive(Debug)]
    struct MockResponse {
        status: u16,
        body: String,
        headers: Vec<(String, String)>,
    }

    impl MockResponse {
        fn json(status: u16, body: &str) -> Self {
            Self {
                status,
                body: body.to_owned(),
                headers: Vec::new(),
            }
        }

        fn with_header(mut self, name: &str, value: &str) -> Self {
            self.headers.push((name.to_owned(), value.to_owned()));
            self
        }
    }

    fn status_reason(status: u16) -> &'static str {
        match status {
            200 => "OK",
            400 => "Bad Request",
            401 => "Unauthorized",
            404 => "Not Found",
            429 => "Too Many Requests",
            500 => "Internal Server Error",
            503 => "Service Unavailable",
            _ => "Test Response",
        }
    }

    fn spawn_server(
        responses: Vec<MockResponse>,
    ) -> (String, Arc<Mutex<Vec<String>>>, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let requests_for_thread = Arc::clone(&requests);

        let handle = thread::spawn(move || {
            for response in responses {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();

                let mut request = Vec::new();
                let mut buffer = [0_u8; 1024];
                while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                    let bytes_read = stream.read(&mut buffer).unwrap();
                    if bytes_read == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..bytes_read]);
                }

                let request_line = String::from_utf8_lossy(&request)
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .to_owned();
                requests_for_thread.lock().unwrap().push(request_line);

                let extra_headers = response
                    .headers
                    .iter()
                    .map(|(name, value)| format!("{name}: {value}\r\n"))
                    .collect::<String>();
                let http_response = format!(
                    "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n{}\r\n{}",
                    response.status,
                    status_reason(response.status),
                    response.body.len(),
                    extra_headers,
                    response.body
                );
                let _ = stream.write_all(http_response.as_bytes());
            }
        });

        (format!("http://{address}/"), requests, handle)
    }

    fn spawn_delayed_server(delay: Duration) -> (String, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                thread::sleep(delay);
                let _ = stream.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
                );
            }
        });

        (format!("http://{address}/"), handle)
    }

    fn test_config() -> StartupConfig {
        StartupConfig::new("test-only-api-key".to_owned(), "pt-BR".to_owned()).unwrap()
    }

    fn client_for(base_url: &str) -> TmdbClient {
        TmdbClient::with_base_url(&test_config(), base_url).unwrap()
    }

    #[test]
    fn movie_search_maps_results_filters_adult_items_and_propagates_query_parameters() {
        let (base_url, requests, handle) = spawn_server(vec![MockResponse::json(
            200,
            r#"{
                "page": 2,
                "total_pages": 3,
                "total_results": 3,
                "results": [
                    {
                        "id": 550,
                        "title": " Clube da Luta ",
                        "original_title": "Fight Club",
                        "release_date": "1999-10-15",
                        "adult": false
                    },
                    {
                        "id": 551,
                        "title": "Adult result",
                        "original_title": "Adult result",
                        "release_date": "2020-01-01",
                        "adult": true
                    },
                    {
                        "id": 552,
                        "title": "",
                        "original_title": "Fallback title",
                        "release_date": "unknown",
                        "adult": false
                    }
                ]
            }"#,
        )]);
        let client = client_for(&base_url);

        let page = client.search_movies_page("  Fight Club  ", 2).unwrap();

        handle.join().unwrap();
        assert_eq!(page.page, 2);
        assert_eq!(page.total_pages, 3);
        assert!(page.has_next_page());
        assert_eq!(page.results.len(), 2);
        assert_eq!(page.results[0].id.value(), 550);
        assert_eq!(page.results[0].media_type, MediaType::Movie);
        assert_eq!(page.results[0].title, "Clube da Luta");
        assert_eq!(
            page.results[0].original_title.as_deref(),
            Some("Fight Club")
        );
        assert_eq!(page.results[0].year, Some(1999));
        assert_eq!(page.results[1].title, "Fallback title");
        assert_eq!(page.results[1].year, None);

        let request = requests.lock().unwrap().join("\n");
        assert!(request.contains("GET /3/search/movie?"));
        assert!(request.contains("api_key=test-only-api-key"));
        assert!(request.contains("language=pt-BR"));
        assert!(request.contains("query=Fight+Club"));
        assert!(request.contains("include_adult=false"));
        assert!(request.contains("page=2"));
    }

    #[test]
    fn tv_search_maps_series_namespace_and_year() {
        let (base_url, requests, handle) = spawn_server(vec![MockResponse::json(
            200,
            r#"{
                "page": 1,
                "total_pages": 1,
                "results": [
                    {
                        "id": 1399,
                        "name": "Game of Thrones",
                        "original_name": "Game of Thrones",
                        "first_air_date": "2011-04-17",
                        "adult": false
                    }
                ]
            }"#,
        )]);
        let client = client_for(&base_url);

        let results = client.search_series("Game of Thrones").unwrap();

        handle.join().unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id.value(), 1399);
        assert_eq!(results[0].media_type, MediaType::Series);
        assert_eq!(results[0].title, "Game of Thrones");
        assert_eq!(results[0].year, Some(2011));
        assert!(requests.lock().unwrap()[0].contains("GET /3/search/tv?"));
    }

    #[test]
    fn empty_search_results_are_returned_without_an_implicit_selection() {
        let (base_url, _, handle) = spawn_server(vec![MockResponse::json(
            200,
            r#"{"page":1,"total_pages":1,"total_results":0,"results":[]}"#,
        )]);
        let results = client_for(&base_url)
            .search_movies("unknown title")
            .unwrap();

        handle.join().unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn details_and_episode_requests_map_verified_metadata() {
        let (base_url, requests, handle) = spawn_server(vec![
            MockResponse::json(
                200,
                r#"{
                    "id": 550,
                    "title": "",
                    "original_title": "Fight Club",
                    "release_date": "1999-10-15"
                }"#,
            ),
            MockResponse::json(
                200,
                r#"{
                    "id": 1399,
                    "name": " Game of Thrones ",
                    "original_name": "Game of Thrones",
                    "first_air_date": "2011-04-17"
                }"#,
            ),
            MockResponse::json(
                200,
                r#"{
                    "name": "Winter Is Coming",
                    "season_number": 1,
                    "episode_number": 1
                }"#,
            ),
        ]);
        let client = client_for(&base_url);
        let movie_id = TmdbId::new(550).unwrap();
        let series_id = TmdbId::new(1399).unwrap();

        let movie = client.get_movie(movie_id).unwrap();
        let series = client.get_item(MediaType::Series, series_id).unwrap();
        let episode = client
            .get_episode_details(series_id, EpisodeRef::new(1, 1))
            .unwrap();

        handle.join().unwrap();
        assert_eq!(movie.title, "Fight Club");
        assert_eq!(movie.original_title.as_deref(), Some("Fight Club"));
        assert_eq!(series.media_type, MediaType::Series);
        assert_eq!(series.title, "Game of Thrones");
        assert_eq!(episode.series_id, series_id);
        assert_eq!(episode.episode, EpisodeRef::new(1, 1));
        assert_eq!(episode.title.as_deref(), Some("Winter Is Coming"));

        let request = requests.lock().unwrap().join("\n");
        assert!(request.contains("GET /3/movie/550?"));
        assert!(request.contains("GET /3/tv/1399?"));
        assert!(request.contains("GET /3/tv/1399/season/1/episode/1?"));
    }

    #[test]
    fn special_season_zero_is_sent_to_tmdb_and_can_be_verified() {
        let (base_url, requests, handle) = spawn_server(vec![MockResponse::json(
            200,
            r#"{"name":"Special","season_number":0,"episode_number":1}"#,
        )]);
        let client = client_for(&base_url);
        let series_id = TmdbId::new(1399).unwrap();

        let episode = client
            .get_episode_details(series_id, EpisodeRef::new(0, 1))
            .unwrap();

        handle.join().unwrap();
        assert_eq!(episode.episode, EpisodeRef::new(0, 1));
        assert!(requests.lock().unwrap()[0].contains("/season/0/episode/1?"));
    }

    #[test]
    fn invalid_details_and_episode_responses_are_rejected_without_body_or_key_leaks() {
        let (base_url, _, handle) = spawn_server(vec![
            MockResponse::json(200, r#"{"id": 551, "title": "Wrong"}"#),
            MockResponse::json(200, r##"{"id": 550, "title": "#"##),
            MockResponse::json(
                200,
                r#"{"name":"Wrong episode","season_number":2,"episode_number":1}"#,
            ),
        ]);
        let client = client_for(&base_url);
        let movie_id = TmdbId::new(550).unwrap();
        let error = client.get_movie(movie_id).unwrap_err();
        assert!(matches!(error, TmdbError::InvalidResponse { .. }));

        let error = client.get_movie(movie_id).unwrap_err();
        assert!(matches!(error, TmdbError::InvalidResponse { .. }));
        assert!(!error.to_string().contains("test-only-api-key"));

        let series_id = TmdbId::new(1399).unwrap();
        let error = client
            .get_episode_details(series_id, EpisodeRef::new(1, 1))
            .unwrap_err();
        assert!(matches!(error, TmdbError::InvalidResponse { .. }));
        handle.join().unwrap();
    }

    #[test]
    fn an_explicit_detail_media_type_mismatch_is_rejected() {
        let (base_url, _, handle) = spawn_server(vec![MockResponse::json(
            200,
            r#"{"id":550,"title":"Unexpected","media_type":"tv"}"#,
        )]);
        let error = client_for(&base_url)
            .get_movie(TmdbId::new(550).unwrap())
            .unwrap_err();

        handle.join().unwrap();
        assert_eq!(
            error,
            TmdbError::MediaTypeMismatch {
                expected: MediaType::Movie,
                actual: "tv".to_owned(),
            }
        );
    }

    #[test]
    fn status_codes_are_mapped_to_actionable_typed_errors() {
        let (base_url, _, handle) = spawn_server(vec![MockResponse::json(
            401,
            r#"{"status_message":"Invalid API key"}"#,
        )]);
        let error = client_for(&base_url).validate_credentials().unwrap_err();
        handle.join().unwrap();
        assert_eq!(error, TmdbError::Authentication { status: 401 });

        let (base_url, _, handle) = spawn_server(vec![
            MockResponse::json(429, r#"{"status_message":"Slow down"}"#)
                .with_header("Retry-After", "7"),
        ]);
        let error = client_for(&base_url).validate_credentials().unwrap_err();
        handle.join().unwrap();
        assert_eq!(
            error,
            TmdbError::RateLimited {
                retry_after_seconds: Some(7)
            }
        );

        let (base_url, _, handle) = spawn_server(vec![MockResponse::json(
            503,
            r#"{"status_message":"Unavailable"}"#,
        )]);
        let error = client_for(&base_url).validate_credentials().unwrap_err();
        handle.join().unwrap();
        assert_eq!(error, TmdbError::Server { status: 503 });

        let (base_url, _, handle) = spawn_server(vec![MockResponse::json(
            404,
            r#"{"status_message":"Not found"}"#,
        )]);
        let error = client_for(&base_url)
            .get_movie(TmdbId::new(550).unwrap())
            .unwrap_err();
        handle.join().unwrap();
        assert_eq!(
            error,
            TmdbError::NotFound {
                resource: "movie 550".to_owned()
            }
        );

        let (base_url, _, handle) = spawn_server(vec![MockResponse::json(
            404,
            r#"{"status_message":"Episode not found"}"#,
        )]);
        let error = client_for(&base_url)
            .get_episode_details(TmdbId::new(1399).unwrap(), EpisodeRef::new(0, 1))
            .unwrap_err();
        handle.join().unwrap();
        assert_eq!(
            error,
            TmdbError::EpisodeNotFound {
                series_id: 1399,
                season: 0,
                episode: 1,
            }
        );
    }

    #[test]
    fn timeout_empty_query_and_invalid_page_are_bounded_errors() {
        let (base_url, handle) = spawn_delayed_server(Duration::from_millis(200));
        let client = TmdbClient::with_base_url_and_timeout(
            &test_config(),
            &base_url,
            Duration::from_millis(50),
        )
        .unwrap();
        let error = client.validate_credentials().unwrap_err();
        handle.join().unwrap();
        assert!(matches!(error, TmdbError::Timeout { .. }));

        let client = client_for("http://127.0.0.1:1/");
        assert_eq!(
            client.search_movies("   ").unwrap_err(),
            TmdbError::EmptySearchQuery
        );
        assert_eq!(
            client.search_movies_page("Movie", 0).unwrap_err(),
            TmdbError::InvalidSearchPage
        );
    }

    #[test]
    fn client_debug_output_redacts_the_api_key() {
        let client = client_for("http://127.0.0.1:1/");
        let debug = format!("{client:?}");

        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("test-only-api-key"));
    }
}
