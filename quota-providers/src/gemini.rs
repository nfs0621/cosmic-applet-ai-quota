// SPDX-License-Identifier: MIT
//! Gemini CLI subscription quota via the Cloud Code private API.
//! Dormant until `~/.gemini/oauth_creds.json` exists.

use serde_json::Value;

use crate::{
    now_millis, parse_reset, ParsedUsage, Provider, ProviderStatus, QuotaSnapshot, QuotaWindow,
};

const QUOTA_URL: &str = "https://cloudcode-pa.googleapis.com/v1internal:retrieveUserQuota";
const CREDENTIALS_PATH: &str = ".gemini/oauth_creds.json";
const EXPIRY_BUFFER_MS: i64 = 60_000;

struct Credentials {
    access_token: String,
    expiry_ms: i64,
}

/// `~/.gemini/oauth_creds.json` → `{ access_token, expiry_date }` (ms epoch).
fn read_credentials() -> Result<Option<Credentials>, String> {
    let Some(path) = crate::home_path(CREDENTIALS_PATH) else {
        return Err("no home directory".into());
    };
    if !path.exists() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(&path).map_err(|e| format!("read oauth_creds: {e}"))?;
    let value: Value = serde_json::from_str(&raw).map_err(|e| format!("creds json: {e}"))?;
    let access_token = value
        .get("access_token")
        .and_then(Value::as_str)
        .ok_or("access_token missing")?
        .to_owned();
    let expiry_ms = value.get("expiry_date").and_then(Value::as_i64).unwrap_or(0);
    Ok(Some(Credentials {
        access_token,
        expiry_ms,
    }))
}

fn bucket_label(bucket: &Value) -> String {
    for key in ["modelId", "model", "name", "tokenType"] {
        if let Some(label) = bucket.get(key).and_then(Value::as_str) {
            return label.to_owned();
        }
    }
    "quota".to_owned()
}

fn bucket_used_percent(bucket: &Value) -> Option<f32> {
    for key in ["remainingFraction", "remaining_fraction"] {
        if let Some(fraction) = bucket.get(key).and_then(Value::as_f64) {
            return Some(((1.0 - fraction) * 100.0).clamp(0.0, 100.0) as f32);
        }
    }
    let remaining = bucket
        .get("remainingAmount")
        .or_else(|| bucket.get("remaining"))
        .and_then(Value::as_f64)?;
    let limit = bucket.get("limit").and_then(Value::as_f64)?;
    if limit <= 0.0 {
        return None;
    }
    Some(((1.0 - remaining / limit) * 100.0).clamp(0.0, 100.0) as f32)
}

/// Pure parser for the `buckets` array; field names are best-effort since the
/// endpoint is private, so unrecognized buckets are skipped.
pub fn parse_quota(value: &Value) -> Result<ParsedUsage, String> {
    let buckets = value
        .get("buckets")
        .and_then(Value::as_array)
        .ok_or("no buckets in response")?;

    let mut windows = Vec::new();
    for bucket in buckets {
        let Some(used_percent) = bucket_used_percent(bucket) else {
            continue;
        };
        windows.push(QuotaWindow {
            label: bucket_label(bucket),
            used_percent,
            resets_at: parse_reset(
                bucket
                    .get("resetTime")
                    .or_else(|| bucket.get("reset_time")),
            ),
        });
    }

    if windows.is_empty() {
        return Err("no parseable quota buckets".into());
    }
    let panel_metric = windows
        .iter()
        .map(|w| w.used_percent)
        .fold(None, |acc: Option<f32>, p| Some(acc.map_or(p, |a| a.max(p))));
    Ok(ParsedUsage {
        windows,
        panel_metric,
        plan: None,
    })
}

pub async fn fetch(client: &reqwest::Client) -> QuotaSnapshot {
    let credentials = match read_credentials() {
        Ok(Some(c)) => c,
        Ok(None) => return QuotaSnapshot::not_configured(Provider::Gemini),
        Err(e) => return QuotaSnapshot::error(Provider::Gemini, e),
    };

    if credentials.expiry_ms > 0 && credentials.expiry_ms - EXPIRY_BUFFER_MS <= now_millis() {
        return QuotaSnapshot::stale(Provider::Gemini);
    }

    let response = client
        .get(QUOTA_URL)
        .bearer_auth(&credentials.access_token)
        .header("Accept", "application/json")
        .send()
        .await;

    let response = match response {
        Ok(r) => r,
        Err(e) => return QuotaSnapshot::error(Provider::Gemini, format!("request failed: {e}")),
    };

    if matches!(response.status().as_u16(), 401 | 403) {
        return QuotaSnapshot::stale(Provider::Gemini);
    }
    if !response.status().is_success() {
        return QuotaSnapshot::error(
            Provider::Gemini,
            format!("quota endpoint returned {}", response.status()),
        );
    }

    let body: Value = match response.json().await {
        Ok(v) => v,
        Err(e) => return QuotaSnapshot::error(Provider::Gemini, format!("bad json: {e}")),
    };

    match parse_quota(&body) {
        Ok(parsed) => QuotaSnapshot {
            provider: Provider::Gemini,
            status: ProviderStatus::Ok,
            windows: parsed.windows,
            panel_metric: parsed.panel_metric,
            plan: parsed.plan,
        },
        Err(e) => QuotaSnapshot::error(Provider::Gemini, e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_remaining_fraction_buckets() {
        let body = json!({
            "buckets": [
                { "modelId": "gemini-3-pro", "remainingFraction": 0.85, "resetTime": "2026-06-11T00:00:00Z" },
                { "modelId": "gemini-3-flash", "remainingFraction": 0.40 }
            ]
        });
        let parsed = parse_quota(&body).unwrap();
        assert_eq!(parsed.windows.len(), 2);
        assert!((parsed.windows[0].used_percent - 15.0).abs() < 0.01);
        assert!((parsed.panel_metric.unwrap() - 60.0).abs() < 0.01);
        assert!(parsed.windows[0].resets_at.is_some());
    }

    #[test]
    fn parses_remaining_over_limit() {
        let body = json!({
            "buckets": [
                { "name": "requests", "remaining": 250.0, "limit": 1000.0 }
            ]
        });
        let parsed = parse_quota(&body).unwrap();
        assert!((parsed.windows[0].used_percent - 75.0).abs() < 0.01);
    }

    #[test]
    fn skips_unparseable_buckets_and_errors_when_none_left() {
        let body = json!({ "buckets": [ { "mystery": true } ] });
        assert!(parse_quota(&body).is_err());
        assert!(parse_quota(&json!({})).is_err());
    }
}
