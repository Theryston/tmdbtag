use serde::Deserialize;

use crate::{
    domain::{
        EpisodeRef, MediaType, TmdbEpisode, TmdbId, TmdbItem, TmdbSearchCandidate, TmdbSearchPage,
    },
    error::TmdbError,
};

/// Raw movie-search response fields used by the application.
#[derive(Debug, Deserialize)]
pub(crate) struct MovieSearchResponse {
    pub(crate) page: Option<u32>,
    pub(crate) total_pages: Option<u32>,
    pub(crate) results: Vec<MovieSearchResult>,
}

/// Raw movie-search result fields used by the application.
#[derive(Debug, Deserialize)]
pub(crate) struct MovieSearchResult {
    pub(crate) id: u64,
    pub(crate) title: Option<String>,
    pub(crate) original_title: Option<String>,
    pub(crate) release_date: Option<String>,
    pub(crate) adult: Option<bool>,
}

/// Raw TV-search response fields used by the application.
#[derive(Debug, Deserialize)]
pub(crate) struct TvSearchResponse {
    pub(crate) page: Option<u32>,
    pub(crate) total_pages: Option<u32>,
    pub(crate) results: Vec<TvSearchResult>,
}

/// Raw TV-search result fields used by the application.
#[derive(Debug, Deserialize)]
pub(crate) struct TvSearchResult {
    pub(crate) id: u64,
    pub(crate) name: Option<String>,
    pub(crate) original_name: Option<String>,
    pub(crate) first_air_date: Option<String>,
    pub(crate) adult: Option<bool>,
}

/// Raw movie-details response fields used by the application.
#[derive(Debug, Deserialize)]
pub(crate) struct MovieDetailsResponse {
    pub(crate) id: u64,
    pub(crate) title: Option<String>,
    pub(crate) original_title: Option<String>,
    pub(crate) release_date: Option<String>,
    pub(crate) media_type: Option<String>,
}

/// Raw TV-details response fields used by the application.
#[derive(Debug, Deserialize)]
pub(crate) struct TvDetailsResponse {
    pub(crate) id: u64,
    pub(crate) name: Option<String>,
    pub(crate) original_name: Option<String>,
    pub(crate) first_air_date: Option<String>,
    pub(crate) media_type: Option<String>,
}

/// Raw episode-details response fields used by the application.
#[derive(Debug, Deserialize)]
pub(crate) struct EpisodeDetailsResponse {
    pub(crate) name: Option<String>,
    pub(crate) season_number: u32,
    pub(crate) episode_number: u32,
}

pub(crate) fn map_movie_search(
    response: MovieSearchResponse,
    max_results: usize,
) -> Result<TmdbSearchPage, TmdbError> {
    let (page, total_pages) = page_values(response.page, response.total_pages, "movie search")?;
    let results = response
        .results
        .into_iter()
        .filter(|result| result.adult != Some(true))
        .take(max_results)
        .map(|result| {
            let id = TmdbId::new(result.id)
                .map_err(|_| invalid_response("movie search", "a result had an invalid ID"))?;
            let (title, original_title) =
                choose_title(result.title, result.original_title, "movie search")?;

            Ok(TmdbSearchCandidate {
                id,
                media_type: MediaType::Movie,
                title,
                original_title,
                year: year_from_date(result.release_date.as_deref()),
            })
        })
        .collect::<Result<Vec<_>, TmdbError>>()?;

    Ok(TmdbSearchPage {
        results,
        page,
        total_pages,
    })
}

pub(crate) fn map_tv_search(
    response: TvSearchResponse,
    max_results: usize,
) -> Result<TmdbSearchPage, TmdbError> {
    let (page, total_pages) = page_values(response.page, response.total_pages, "TV search")?;
    let results = response
        .results
        .into_iter()
        .filter(|result| result.adult != Some(true))
        .take(max_results)
        .map(|result| {
            let id = TmdbId::new(result.id)
                .map_err(|_| invalid_response("TV search", "a result had an invalid ID"))?;
            let (title, original_title) =
                choose_title(result.name, result.original_name, "TV search")?;

            Ok(TmdbSearchCandidate {
                id,
                media_type: MediaType::Series,
                title,
                original_title,
                year: year_from_date(result.first_air_date.as_deref()),
            })
        })
        .collect::<Result<Vec<_>, TmdbError>>()?;

    Ok(TmdbSearchPage {
        results,
        page,
        total_pages,
    })
}

pub(crate) fn map_movie_details(
    response: MovieDetailsResponse,
    requested_id: TmdbId,
) -> Result<TmdbItem, TmdbError> {
    validate_media_type(response.media_type.as_deref(), MediaType::Movie)?;
    let id = map_response_id(response.id, requested_id, "movie details")?;
    let (title, original_title) =
        choose_title(response.title, response.original_title, "movie details")?;

    Ok(TmdbItem {
        id,
        media_type: MediaType::Movie,
        title,
        original_title,
        year: year_from_date(response.release_date.as_deref()),
    })
}

pub(crate) fn map_tv_details(
    response: TvDetailsResponse,
    requested_id: TmdbId,
) -> Result<TmdbItem, TmdbError> {
    validate_media_type(response.media_type.as_deref(), MediaType::Series)?;
    let id = map_response_id(response.id, requested_id, "TV details")?;
    let (title, original_title) =
        choose_title(response.name, response.original_name, "TV details")?;

    Ok(TmdbItem {
        id,
        media_type: MediaType::Series,
        title,
        original_title,
        year: year_from_date(response.first_air_date.as_deref()),
    })
}

pub(crate) fn map_episode_details(
    response: EpisodeDetailsResponse,
    series_id: TmdbId,
    requested_episode: EpisodeRef,
) -> Result<TmdbEpisode, TmdbError> {
    if response.season_number != requested_episode.season()
        || response.episode_number != requested_episode.episode()
    {
        return Err(invalid_response(
            "episode details",
            "the returned season and episode do not match the request",
        ));
    }

    Ok(TmdbEpisode {
        series_id,
        episode: requested_episode,
        title: clean_text(response.name),
    })
}

fn map_response_id(
    response_id: u64,
    requested_id: TmdbId,
    operation: &str,
) -> Result<TmdbId, TmdbError> {
    let response_id = TmdbId::new(response_id)
        .map_err(|_| invalid_response(operation, "the response had an invalid ID"))?;
    if response_id != requested_id {
        return Err(invalid_response(
            operation,
            "the response ID did not match the requested ID",
        ));
    }

    Ok(response_id)
}

fn validate_media_type(response_type: Option<&str>, expected: MediaType) -> Result<(), TmdbError> {
    let Some(response_type) = response_type
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(());
    };

    let expected_type = match expected {
        MediaType::Movie => "movie",
        MediaType::Series => "tv",
    };
    if response_type != expected_type {
        return Err(TmdbError::MediaTypeMismatch {
            expected,
            actual: response_type.to_owned(),
        });
    }

    Ok(())
}

fn page_values(
    page: Option<u32>,
    total_pages: Option<u32>,
    operation: &str,
) -> Result<(u32, u32), TmdbError> {
    let page = page.unwrap_or(1);
    let total_pages = total_pages.unwrap_or(1);
    if page == 0 || total_pages == 0 || page > total_pages {
        return Err(invalid_response(
            operation,
            "the response contained invalid pagination values",
        ));
    }

    Ok((page, total_pages))
}

fn choose_title(
    localized: Option<String>,
    original: Option<String>,
    operation: &str,
) -> Result<(String, Option<String>), TmdbError> {
    let original = clean_text(original);
    let title = clean_text(localized).or_else(|| original.clone());
    let title =
        title.ok_or_else(|| invalid_response(operation, "the response had no usable title"))?;
    Ok((title, original))
}

fn clean_text(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn year_from_date(value: Option<&str>) -> Option<u16> {
    let year = value?.get(..4)?;
    if !year.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }

    year.parse().ok()
}

fn invalid_response(operation: &str, reason: &str) -> TmdbError {
    TmdbError::InvalidResponse {
        operation: operation.to_owned(),
        reason: reason.to_owned(),
    }
}
