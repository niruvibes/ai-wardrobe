use crate::model::{Location, Plan, WeatherSummary, now};
use anyhow::{Context, Result, bail, ensure};
use chrono::{DateTime, Duration, NaiveDateTime, Timelike, Utc};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hour {
    pub time: NaiveDateTime,
    pub temperature: f32,
    pub feels_like: f32,
    pub rain_probability: f32,
    pub precipitation_mm: f32,
    pub wind_kmh: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Forecast {
    pub location: Location,
    pub fetched_at: String,
    pub hours: Vec<Hour>,
}

impl Forecast {
    pub fn fresh(&self) -> bool {
        DateTime::parse_from_rfc3339(&self.fetched_at).is_ok_and(|at| {
            let age = Utc::now().signed_duration_since(at);
            age >= Duration::minutes(-5) && age <= Duration::hours(3)
        })
    }
    /// Include every hourly interval intersecting the outing, including partial hours.
    /// Timestamps are local to the forecast location, never the computer's timezone.
    pub fn window(
        &self,
        start: NaiveDateTime,
        end: NaiveDateTime,
    ) -> Result<(WeatherSummary, Vec<Hour>)> {
        ensure!(
            end > start && end - start <= Duration::hours(48),
            "Choose an outing between 1 minute and 48 hours."
        );
        let first = start
            .with_minute(0)
            .unwrap()
            .with_second(0)
            .unwrap()
            .with_nanosecond(0)
            .unwrap();
        let mut at = first;
        let mut selected = Vec::new();
        while at < end {
            ensure!(
                self.hours.iter().filter(|h| h.time == at).count() <= 1,
                "This outing includes an ambiguous daylight-saving hour. Enter manual conditions for this outing."
            );
            let hour = self.hours.iter().find(|h| h.time == at).with_context(|| {
                format!(
                    "Forecast does not cover {at}. Refresh the forecast or enter manual conditions."
                )
            })?;
            ensure!(
                [
                    hour.temperature,
                    hour.feels_like,
                    hour.rain_probability,
                    hour.precipitation_mm,
                    hour.wind_kmh
                ]
                .iter()
                .all(|v| v.is_finite()),
                "Forecast has incomplete weather data."
            );
            selected.push(hour.clone());
            at += Duration::hours(1);
        }
        let min = |f: fn(&Hour) -> f32| selected.iter().map(f).fold(f32::INFINITY, f32::min);
        let max = |f: fn(&Hour) -> f32| selected.iter().map(f).fold(f32::NEG_INFINITY, f32::max);
        let summary = WeatherSummary {
            temperature_min: min(|h| h.temperature),
            temperature_max: max(|h| h.temperature),
            feels_min: min(|h| h.feels_like),
            feels_max: max(|h| h.feels_like),
            rain_probability: max(|h| h.rain_probability),
            precipitation_mm: selected.iter().map(|h| h.precipitation_mm).sum(),
            wind_kmh: max(|h| h.wind_kmh),
            source: format!(
                "Open-Meteo · fetched {} · {}",
                self.fetched_at, self.location.timezone
            ),
        };
        summary.validate()?;
        Ok((summary, selected))
    }

    pub fn apply_to(&self, plan: &mut Plan) -> Result<Vec<Hour>> {
        ensure!(
            self.fresh(),
            "The cached forecast is over 3 hours old. Refresh it or select manual conditions."
        );
        let (summary, hours) = self.window(plan.start, plan.end)?;
        plan.weather = summary;
        plan.location = self.location.label();
        Ok(hours)
    }
}

fn client() -> Result<Client> {
    Ok(Client::builder()
        .timeout(std::time::Duration::from_secs(25))
        .user_agent("Wardrobe/0.1 (personal desktop app)")
        .build()?)
}

pub fn search_locations(query: &str) -> Result<Vec<Location>> {
    ensure!(
        query.trim().chars().count() >= 2,
        "Enter at least two characters for a place."
    );
    #[derive(Deserialize)]
    struct Response {
        #[serde(default)]
        results: Vec<Location>,
    }
    let response: Response = client()?
        .get("https://geocoding-api.open-meteo.com/v1/search")
        .query(&[
            ("name", query.trim()),
            ("count", "8"),
            ("language", "en"),
            ("format", "json"),
        ])
        .send()?
        .error_for_status()?
        .json()?;
    Ok(response.results)
}

#[derive(Deserialize)]
pub struct HourlyResponse {
    pub time: Vec<String>,
    pub temperature_2m: Vec<Option<f32>>,
    pub apparent_temperature: Vec<Option<f32>>,
    pub precipitation_probability: Vec<Option<f32>>,
    pub precipitation: Vec<Option<f32>>,
    pub wind_speed_10m: Vec<Option<f32>>,
}

pub fn parse_hours(raw: HourlyResponse) -> Result<Vec<Hour>> {
    let count = raw.time.len();
    ensure!(
        count > 0
            && [
                raw.temperature_2m.len(),
                raw.apparent_temperature.len(),
                raw.precipitation_probability.len(),
                raw.precipitation.len(),
                raw.wind_speed_10m.len()
            ]
            .iter()
            .all(|n| *n == count),
        "Weather service returned mismatched hourly data."
    );
    let mut hours = Vec::with_capacity(count);
    for (i, time) in raw.time.iter().enumerate() {
        let required = |v: Option<f32>| {
            v.filter(|v|v.is_finite()).context("Weather service returned a missing hourly value. Try refreshing or manual conditions.")
        };
        hours.push(Hour {
            time: NaiveDateTime::parse_from_str(time, "%Y-%m-%dT%H:%M")?,
            temperature: required(raw.temperature_2m[i])?,
            feels_like: required(raw.apparent_temperature[i])?,
            rain_probability: required(raw.precipitation_probability[i])?,
            precipitation_mm: required(raw.precipitation[i])?,
            wind_kmh: required(raw.wind_speed_10m[i])?,
        });
    }
    Ok(hours)
}

pub fn fetch_forecast(location: &Location) -> Result<Forecast> {
    #[derive(Deserialize)]
    struct Response {
        timezone: String,
        hourly: HourlyResponse,
    }
    ensure!(
        location.latitude.is_finite()
            && (-90.0..=90.0).contains(&location.latitude)
            && location.longitude.is_finite()
            && (-180.0..=180.0).contains(&location.longitude),
        "Invalid location coordinates."
    );
    let response: Response = client()?.get("https://api.open-meteo.com/v1/forecast")
        .query(&[("latitude", location.latitude.to_string()), ("longitude", location.longitude.to_string()),
            ("hourly", "temperature_2m,apparent_temperature,precipitation_probability,precipitation,wind_speed_10m".into()),
            ("temperature_unit", "celsius".into()), ("wind_speed_unit", "kmh".into()), ("precipitation_unit", "mm".into()),
            ("timezone", location.timezone.clone()), ("forecast_days", "7".into())])
        .send()?.error_for_status()?.json()?;
    if response.timezone != location.timezone {
        bail!("Weather service returned a different timezone. Search for the location again.");
    }
    Ok(Forecast {
        location: location.clone(),
        fetched_at: now(),
        hours: parse_hours(response.hourly)?,
    })
}
