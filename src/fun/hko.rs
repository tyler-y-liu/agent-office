//! Hong Kong's real weather from the Hong Kong Observatory's open data, so it
//! rains out the window when it rains in Hong Kong, and a typhoon signal goes
//! up when one is hoisted. Off unless `AGENT_OFFICE_WEATHER=hko`: it is a
//! request to a third party every few minutes, though it sends nothing of yours.

use std::path::PathBuf;
use std::time::Duration;

use pixtuoid_scene::sky::Weather;
use serde_json::Value;
use tokio::sync::watch;

const CURRENT: &str =
    "https://data.weather.gov.hk/weatherAPI/opendata/weather.php?dataType=rhrread&lang=en";
const WARNINGS: &str =
    "https://data.weather.gov.hk/weatherAPI/opendata/weather.php?dataType=warnsum&lang=en";
const EVERY: Duration = Duration::from_secs(10 * 60);

/// What the Observatory says now.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Outlook {
    /// The weather to show; `None` leaves the office's own.
    pub weather: Option<Weather>,
    /// The tropical cyclone signal up: 1, 3, 8, 9 or 10.
    pub signal: Option<u8>,
}

impl Outlook {
    /// A signal No. 8 or higher: offices close and the ferries stop.
    pub fn typhoon(&self) -> bool {
        self.signal.is_some_and(|s| s >= 8)
    }
}

/// Whether the user turned the Observatory's weather on.
pub fn enabled() -> bool {
    let file = std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join(crate::tasks::client::ENV_FILE))
        .and_then(|p| std::fs::read_to_string(p).ok())
        .unwrap_or_default();
    std::env::var("AGENT_OFFICE_WEATHER")
        .ok()
        .or_else(|| crate::tasks::client::env_file_var(&file, "AGENT_OFFICE_WEATHER"))
        .is_some_and(|v| v.eq_ignore_ascii_case("hko"))
}

/// Start asking the Observatory, when the user turned it on; the outlook
/// stays empty otherwise. Must run inside a tokio runtime.
pub fn spawn() -> watch::Receiver<Outlook> {
    let (tx, rx) = watch::channel(Outlook::default());
    if !enabled() {
        return rx;
    }
    tokio::spawn(async move {
        let Ok(http) = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
        else {
            return;
        };
        let get = |url: &'static str| {
            let http = http.clone();
            async move { http.get(url).send().await?.json::<Value>().await }
        };
        loop {
            // A failed read keeps the last outlook rather than clearing the sky.
            if let (Ok(current), Ok(warnings)) = (get(CURRENT).await, get(WARNINGS).await) {
                let _ = tx.send(outlook(&current, &warnings));
            }
            tokio::time::sleep(EVERY).await;
        }
    });
    rx
}

/// The outlook from the Observatory's current weather and warning summary.
fn outlook(current: &Value, warnings: &Value) -> Outlook {
    let issued = |key: &str| {
        warnings
            .get(key)
            .filter(|w| w.get("actionCode").and_then(Value::as_str) != Some("CANCEL"))
    };
    let signal = issued("WTCSGNL")
        .and_then(|w| w.get("code").and_then(Value::as_str))
        .and_then(signal_of);
    let icon = current
        .get("icon")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(Value::as_u64);
    let weather = if signal.is_some_and(|s| s >= 8) {
        Some(Weather::Windy)
    } else if issued("WTS").is_some() {
        Some(Weather::Storm)
    } else if issued("WRAIN").is_some() {
        Some(Weather::Rain)
    } else {
        icon.and_then(weather_of_icon)
    };
    Outlook { weather, signal }
}

/// A signal's number from its code: `TC1`, `TC3`, `TC8NE`, `TC9`, `TC10`.
fn signal_of(code: &str) -> Option<u8> {
    let digits: String = code
        .strip_prefix("TC")?
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok().filter(|n| [1, 3, 8, 9, 10].contains(n))
}

/// The Observatory's weather icon as the office's weather.
fn weather_of_icon(icon: u64) -> Option<Weather> {
    Some(match icon {
        50..=52 | 70..=77 | 81 | 82 | 90..=93 => Weather::Clear,
        60 | 61 => Weather::Overcast,
        53 | 54 | 62..=64 => Weather::Rain,
        65 => Weather::Storm,
        80 => Weather::Windy,
        83 | 84 => Weather::Fog,
        85 => Weather::Smog,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The weather out the window follows the Observatory: its icon on an
    /// ordinary day, its warnings over the icon, and a No. 8 signal over all,
    /// since a typhoon is what everyone in the office is looking at.
    #[test]
    fn the_observatory_s_word_sets_the_weather_and_the_signal() {
        let sunny = json!({"icon": [50]});
        assert_eq!(
            outlook(&sunny, &json!({})),
            Outlook {
                weather: Some(Weather::Clear),
                signal: None
            }
        );
        assert_eq!(
            outlook(&json!({"icon": [63]}), &json!({})).weather,
            Some(Weather::Rain)
        );
        let rainstorm = json!({"WRAIN": {"code": "WRAINA", "actionCode": "ISSUE"}});
        assert_eq!(outlook(&sunny, &rainstorm).weather, Some(Weather::Rain));
        let t8 = json!({"WTCSGNL": {"code": "TC8NE", "actionCode": "ISSUE"},
                        "WRAIN": {"code": "WRAINA", "actionCode": "ISSUE"}});
        let o = outlook(&sunny, &t8);
        assert_eq!(o.signal, Some(8));
        assert_eq!(o.weather, Some(Weather::Windy));
        assert!(o.typhoon());
        let t3 = json!({"WTCSGNL": {"code": "TC3", "actionCode": "REISSUE"}});
        assert!(
            !outlook(&sunny, &t3).typhoon(),
            "No. 3 keeps the office open"
        );
        let lowered = json!({"WTCSGNL": {"code": "TC8SW", "actionCode": "CANCEL"}});
        assert_eq!(
            outlook(&sunny, &lowered).signal,
            None,
            "a cancelled signal is down"
        );
        assert_eq!(outlook(&json!({}), &json!({})), Outlook::default());
    }

    #[test]
    fn signal_codes_read_as_their_numbers() {
        assert_eq!(signal_of("TC1"), Some(1));
        assert_eq!(signal_of("TC10"), Some(10));
        assert_eq!(signal_of("TC9"), Some(9));
        assert_eq!(signal_of("TC8NW"), Some(8));
        assert_eq!(signal_of("TC5"), None);
        assert_eq!(signal_of("WRAINA"), None);
    }
}
