use std::thread;
use std::time::Duration;

use serde_json::Value;
use slint::Weak;

use crate::app_state::WEATHER_CITY;
use crate::links::{http_get_string, url_encode};
use crate::MainWindow;

struct Report {
    city: String,
    temp: i32,
    hi: i32,
    lo: i32,
    humidity: i32,
    wind: i32,
    kind: i32,
    desc: &'static str,
}

/// Weather code to (icon kind, text). Kinds: 0 sun, 1 partly cloudy, 2 cloud, 3 rain, 4 snow, 5 storm, 6 fog.
fn describe(code: i64) -> (i32, &'static str) {
    match code {
        0 => (0, "Clear sky"),
        1 => (0, "Mostly clear"),
        2 => (1, "Partly cloudy"),
        3 => (2, "Overcast"),
        45 | 48 => (6, "Fog"),
        51 | 53 | 55 | 56 | 57 => (3, "Drizzle"),
        61 | 63 | 65 | 66 | 67 => (3, "Rain"),
        80 | 81 | 82 => (3, "Showers"),
        71 | 73 | 75 | 77 | 85 | 86 => (4, "Snow"),
        95 | 96 | 99 => (5, "Thunderstorm"),
        _ => (2, "Cloudy"),
    }
}

fn fetch(city: &str) -> Option<Report> {
    let geo_url = format!(
        "https://geocoding-api.open-meteo.com/v1/search?name={}&count=1",
        url_encode(city)
    );
    let geo: Value = serde_json::from_str(&http_get_string(&geo_url)?).ok()?;
    let place = geo["results"].get(0)?;
    let lat = place["latitude"].as_f64()?;
    let lon = place["longitude"].as_f64()?;
    let name = place["name"].as_str().unwrap_or(city).to_string();

    let url = format!(
        "https://api.open-meteo.com/v1/forecast?latitude={lat}&longitude={lon}\
         &current=temperature_2m,relative_humidity_2m,weather_code,wind_speed_10m\
         &daily=temperature_2m_max,temperature_2m_min&timezone=auto"
    );
    let data: Value = serde_json::from_str(&http_get_string(&url)?).ok()?;
    let cur = &data["current"];
    let (kind, desc) = describe(cur["weather_code"].as_i64()?);

    Some(Report {
        city: name,
        temp: cur["temperature_2m"].as_f64()?.round() as i32,
        humidity: cur["relative_humidity_2m"].as_f64()?.round() as i32,
        wind: cur["wind_speed_10m"].as_f64()?.round() as i32,
        hi: data["daily"]["temperature_2m_max"].get(0)?.as_f64()?.round() as i32,
        lo: data["daily"]["temperature_2m_min"].get(0)?.as_f64()?.round() as i32,
        kind,
        desc,
    })
}

/// Refreshes the weather every 15 minutes (retries after 1 minute when offline).
pub fn spawn(ui: Weak<MainWindow>) {
    thread::spawn(move || loop {
        let wait_secs = match fetch(WEATHER_CITY) {
            Some(r) => {
                let sent = ui.upgrade_in_event_loop(move |ui| {
                    ui.set_weather_city(r.city.into());
                    ui.set_weather_temp(format!("{}°", r.temp).into());
                    ui.set_weather_desc(r.desc.into());
                    ui.set_weather_hilo(format!("{}° / {}°", r.hi, r.lo).into());
                    ui.set_weather_humidity(format!("{}%", r.humidity).into());
                    ui.set_weather_wind(format!("{} km/h", r.wind).into());
                    ui.set_weather_kind(r.kind);
                    ui.set_weather_ready(true);
                });
                if sent.is_err() {
                    return;
                }
                900
            }
            None => 60,
        };
        thread::sleep(Duration::from_secs(wait_secs));
    });
}