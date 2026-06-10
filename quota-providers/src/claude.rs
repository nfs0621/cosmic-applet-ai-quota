// SPDX-License-Identifier: MIT
//! Claude Code subscription quota via the OAuth usage endpoint.

use serde_json::Value;

use crate::{
    now_millis, parse_reset, ParsedUsage, Provider, ProviderStatus, QuotaSnapshot, QuotaWindow,
};

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const CREDENTIALS_PATH: &str = ".claude/.credentials.json";
/// Skip the call if the token expires within this margin.
const EXPIRY_BUFFER_MS: i64 = 60_000;

struct Credentials {
    access_token: String,
    expires_at_ms: i64,
    plan: Option<String>,
}

/// `~/.claude/.credentials.json` → `claudeAiOauth { accessToken, expiresAt, subscriptionType }`.
fn read_credentials() -> Result<Option<Credentials>, String> {
    let Some(path) = crate::home_path(CREDENTIALS_PATH) else {
        return Err("no home directory".into());
    };
    if !path.exists() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(&path).map_err(|e| format!("read credentials: {e}"))?;
    let value: Value =
        serde_json::from_str(&raw).map_err(|e| format!("credentials json: {e}"))?;
    let oauth = value
        .get("claudeAiOauth")
        .ok_or("claudeAiOauth missing from credentials")?;
    let access_token = oauth
        .get("accessToken")
        .and_then(Value::as_str)
        .ok_or("accessToken missing")?
        .to_owned();
    let expires_at_ms = oauth.get("expiresAt").and_then(Value::as_i64).unwrap_or(0);
    let plan = oauth
        .get("subscriptionType")
        .and_then(Value::as_str)
        .map(str::to_owned);
    Ok(Some(Credentials {
        access_token,
        expires_at_ms,
        plan,
    }))
}

/// Pure parser for the usage response. Tolerates absent windows; errors only
/// when no usable window is present at all.
pub fn parse_usage(value: &Value) -> Result<ParsedUsage, String> {
    const WINDOWS: [(&str, &str); 4] = [
        ("five_hour", "5-hour"),
        ("seven_day", "Weekly"),
        ("seven_day_sonnet", "Sonnet weekly"),
        ("seven_day_opus", "Opus weekly"),
    ];

    let mut windows = Vec::new();
    let mut panel_metric = None;
    for (key, label) in WINDOWS {
        let Some(entry) = value.get(key).filter(|v| !v.is_null()) else {
            continue;
        };
        let Some(utilization) = entry.get("utilization").and_then(Value::as_f64) else {
            continue;
        };
        let used_percent = utilization as f32;
        if key == "five_hour" {
            panel_metric = Some(used_percent);
        }
        windows.push(QuotaWindow {
            label: label.into(),
            used_percent,
            resets_at: parse_reset(entry.get("resets_at")),
        });
    }

    if windows.is_empty() {
        return Err("no usage windows in response".into());
    }
    Ok(ParsedUsage {
        // Fall back to the worst window if five_hour ever disappears.
        panel_metric: panel_metric.or_else(|| {
            windows
                .iter()
                .map(|w| w.used_percent)
                .fold(None, |acc: Option<f32>, p| Some(acc.map_or(p, |a| a.max(p))))
        }),
        windows,
        plan: None,
    })
}

pub async fn fetch(client: &reqwest::Client) -> QuotaSnapshot {
    let credentials = match read_credentials() {
        Ok(Some(c)) => c,
        Ok(None) => return QuotaSnapshot::not_configured(Provider::Claude),
        Err(e) => return QuotaSnapshot::error(Provider::Claude, e),
    };

    if credentials.expires_at_ms > 0
        && credentials.expires_at_ms - EXPIRY_BUFFER_MS <= now_millis()
    {
        return QuotaSnapshot::stale(Provider::Claude);
    }

    let response = client
        .get(USAGE_URL)
        .bearer_auth(&credentials.access_token)
        .header("anthropic-beta", "oauth-2025-04-20")
        .header("Accept", "application/json")
        .send()
        .await;

    let response = match response {
        Ok(r) => r,
        Err(e) => {
            return QuotaSnapshot::error(Provider::Claude, format!("request failed: {e}"))
        }
    };

    if matches!(response.status().as_u16(), 401 | 403) {
        return QuotaSnapshot::stale(Provider::Claude);
    }
    if !response.status().is_success() {
        return QuotaSnapshot::error(
            Provider::Claude,
            format!("usage endpoint returned {}", response.status()),
        );
    }

    let body: Value = match response.json().await {
        Ok(v) => v,
        Err(e) => return QuotaSnapshot::error(Provider::Claude, format!("bad json: {e}")),
    };

    match parse_usage(&body) {
        Ok(parsed) => QuotaSnapshot {
            provider: Provider::Claude,
            status: ProviderStatus::Ok,
            windows: parsed.windows,
            panel_metric: parsed.panel_metric,
            plan: credentials.plan,
        },
        Err(e) => QuotaSnapshot::error(Provider::Claude, e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_all_windows_with_panel_metric_from_five_hour() {
        let body = json!({
            "five_hour": { "utilization": 72.0, "resets_at": "2026-06-10T17:00:00Z" },
            "seven_day": { "utilization": 31.0, "resets_at": "2026-06-14T00:00:00Z" },
            "seven_day_opus": { "utilization": 15.5, "resets_at": "2026-06-14T00:00:00Z" }
        });
        let parsed = parse_usage(&body).unwrap();
        assert_eq!(parsed.panel_metric, Some(72.0));
        assert_eq!(parsed.windows.len(), 3);
        assert_eq!(parsed.windows[0].label, "5-hour");
        assert_eq!(parsed.windows[1].used_percent, 31.0);
        assert!(parsed.windows[0].resets_at.is_some());
    }

    #[test]
    fn missing_five_hour_falls_back_to_worst_window() {
        let body = json!({
            "seven_day": { "utilization": 31.0 },
            "seven_day_opus": { "utilization": 80.0 }
        });
        let parsed = parse_usage(&body).unwrap();
        assert_eq!(parsed.panel_metric, Some(80.0));
    }

    #[test]
    fn null_window_skipped() {
        let body = json!({
            "five_hour": { "utilization": 10.0 },
            "seven_day": null
        });
        let parsed = parse_usage(&body).unwrap();
        assert_eq!(parsed.windows.len(), 1);
    }

    #[test]
    fn empty_response_is_error() {
        assert!(parse_usage(&json!({})).is_err());
        assert!(parse_usage(&json!({"five_hour": {"foo": 1}})).is_err());
    }
}
