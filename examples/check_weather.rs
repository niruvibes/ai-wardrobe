fn main() -> anyhow::Result<()> {
    let query = std::env::args().nth(1).unwrap_or_else(|| "Sydney".into());
    let locations = wardrobe::weather::search_locations(&query)?;
    for location in &locations {
        println!("{} · {}", location.label(), location.timezone);
    }
    let location = locations
        .first()
        .ok_or_else(|| anyhow::anyhow!("No location found"))?;
    let forecast = wardrobe::weather::fetch_forecast(location)?;
    println!(
        "Forecast: {} hourly readings, fetched {}",
        forecast.hours.len(),
        forecast.fetched_at
    );
    anyhow::ensure!(forecast.hours.len() >= 24, "Not enough forecast hours");
    let first = forecast.hours.first().unwrap();
    let (summary, hours) = forecast.window(
        first.time + chrono::Duration::minutes(30),
        first.time + chrono::Duration::hours(8),
    )?;
    println!(
        "Checked {} hours: {:.1}–{:.1} °C; feels like {:.1}–{:.1} °C; rain {:.0}%; wind {:.1} km/h",
        hours.len(),
        summary.temperature_min,
        summary.temperature_max,
        summary.feels_min,
        summary.feels_max,
        summary.rain_probability,
        summary.wind_kmh
    );
    Ok(())
}
