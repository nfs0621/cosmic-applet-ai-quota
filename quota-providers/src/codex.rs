// SPDX-License-Identifier: MIT
//! OpenAI Codex subscription quota via the ChatGPT backend usage endpoint.

use serde_json::Value;

use crate::{parse_reset, ParsedUsage, Provider, ProviderStatus, QuotaSnapshot, QuotaWindow};

const USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const AUTH_PATH: &str = ".codex/auth.json";

struct Credentials {
    access_token: String,
    account_id: Option<String>,
}

/// `~/.codex/auth.json` → `tokens { access_token, account_id }`. No expiry
/// field exists, so expiry shows up as a 401 at request time instead.
fn read_credentials() -> Result<Option<Credentials>, String> {
    let Some(path) = crate::home_path(AUTH_PATH) else {
        return Err("no home directory".into());
    };
    if !path.exists() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(&path).map_err(|e| format!("read auth.json: {e}"))?;
    let value: Value = serde_json::from_str(&raw).map_err(|e| format!("auth json: {e}"))?;
    let tokens = value.get("tokens").ok_or("tokens missing from auth.json")?;
    let access_token = tokens
        .get("access_token")
        .and_then(Value::as_str)
        .ok_or("access_token missing")?
        .to_owned();
    let account_id = tokens
        .get("account_id")
        .and_then(Value::as_str)
        .map(str::to_owned);
    Ok(Some(Credentials {
        access_token,
        account_id,
    }))
}

/// Pure parser. `rate_limit` (or `rate_limits`) may be an object or an array
/// of one entry; windows are `primary_window` (5h) and `secondary_window`
/// (weekly), each `{ used_percent, reset_at }`.
pub fn parse_usage(value: &Value) -> Result<ParsedUsage, String> {
    let rate_limit = value
        .get("rate_limit")
        .or_else(|| value.get("rate_limits"))
        .ok_or("no rate_limit in response")?;
    let entry = match rate_limit {
        Value::Array(items) => items.first().ok_or("rate_limit array empty")?,
        other @ Value::Object(_) => other,
        _ => return Err("unexpected rate_limit shape".into()),
    };

    let mut windows = Vec::new();
    let mut panel_metric = None;
    for (key, label) in [("primary_window", "5-hour"), ("secondary_window", "Weekly")] {
        let Some(window) = entry.get(key).filter(|v| !v.is_null()) else {
            continue;
        };
        let Some(used) = window.get("used_percent").and_then(Value::as_f64) else {
            continue;
        };
        let used_percent = used as f32;
        if key == "primary_window" {
            panel_metric = Some(used_percent);
        }
        windows.push(QuotaWindow {
            label: label.into(),
            used_percent,
            resets_at: parse_reset(window.get("reset_at").or_else(|| window.get("resets_at"))),
        });
    }

    if windows.is_empty() {
        return Err("no rate limit windows in response".into());
    }
    Ok(ParsedUsage {
        panel_metric,
        windows,
        plan: value
            .get("plan_type")
            .and_then(Value::as_str)
            .map(str::to_owned),
    })
}

pub async fn fetch(client: &reqwest::Client) -> QuotaSnapshot {
    let credentials = match read_credentials() {
        Ok(Some(c)) => c,
        Ok(None) => return QuotaSnapshot::not_configured(Provider::Codex),
        Err(e) => return QuotaSnapshot::error(Provider::Codex, e),
    };

    let mut request = client
        .get(USAGE_URL)
        .bearer_auth(&credentials.access_token)
        .header("Accept", "application/json");
    if let Some(account_id) = &credentials.account_id {
        request = request.header("ChatGPT-Account-Id", account_id);
    }

    let response = match request.send().await {
        Ok(r) => r,
        Err(e) => return QuotaSnapshot::error(Provider::Codex, format!("request failed: {e}")),
    };

    if matches!(response.status().as_u16(), 401 | 403) {
        return QuotaSnapshot::stale(Provider::Codex);
    }
    if !response.status().is_success() {
        return QuotaSnapshot::error(
            Provider::Codex,
            format!("usage endpoint returned {}", response.status()),
        );
    }

    let body: Value = match response.json().await {
        Ok(v) => v,
        Err(e) => return QuotaSnapshot::error(Provider::Codex, format!("bad json: {e}")),
    };

    match parse_usage(&body) {
        Ok(parsed) => QuotaSnapshot {
            provider: Provider::Codex,
            status: ProviderStatus::Ok,
            windows: parsed.windows,
            panel_metric: parsed.panel_metric,
            plan: parsed.plan,
        },
        Err(e) => QuotaSnapshot::error(Provider::Codex, e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_array_form_with_epoch_resets() {
        let body = json!({
            "rate_limit": [{
                "primary_window": { "used_percent": 18.0, "reset_at": 1718040000 },
                "secondary_window": { "used_percent": 9.0, "reset_at": 1718600000 },
                "limit_reached": false
            }],
            "plan_type": "plus"
        });
        let parsed = parse_usage(&body).unwrap();
        assert_eq!(parsed.panel_metric, Some(18.0));
        assert_eq!(parsed.windows.len(), 2);
        assert_eq!(parsed.windows[1].label, "Weekly");
        assert!(parsed.windows[0].resets_at.is_some());
        assert_eq!(parsed.plan.as_deref(), Some("plus"));
    }

    #[test]
    fn parses_object_form_and_rate_limits_alias() {
        let body = json!({
            "rate_limits": {
                "primary_window": { "used_percent": 50.5 }
            }
        });
        let parsed = parse_usage(&body).unwrap();
        assert_eq!(parsed.panel_metric, Some(50.5));
        assert_eq!(parsed.windows.len(), 1);
        assert!(parsed.windows[0].resets_at.is_none());
    }

    #[test]
    fn missing_rate_limit_is_error() {
        assert!(parse_usage(&json!({})).is_err());
        assert!(parse_usage(&json!({"rate_limit": []})).is_err());
        assert!(parse_usage(&json!({"rate_limit": {"primary_window": {}}})).is_err());
    }
}
