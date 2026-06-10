// SPDX-License-Identifier: MIT
//! Quota snapshots for AI coding subscriptions, read from each CLI's local
//! OAuth credentials. Strictly read-only: credential files are never written,
//! and tokens are never logged or embedded in error messages.

pub mod claude;
pub mod codex;
pub mod gemini;

use std::path::PathBuf;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Provider {
    Claude,
    Codex,
    Gemini,
}

impl Provider {
    pub fn name(self) -> &'static str {
        match self {
            Provider::Claude => "Claude",
            Provider::Codex => "OpenAI Codex",
            Provider::Gemini => "Gemini",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProviderStatus {
    /// Fresh data from the provider.
    Ok,
    /// Token expired or rejected; windows hold the last known data, if any.
    Stale,
    /// Credential file absent — provider not set up on this machine.
    NotConfigured,
    /// Fetch or parse failure unrelated to auth.
    Error(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct QuotaWindow {
    pub label: String,
    /// 0.0–100.0, percent of the quota window consumed.
    pub used_percent: f32,
    pub resets_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct QuotaSnapshot {
    pub provider: Provider,
    pub status: ProviderStatus,
    pub windows: Vec<QuotaWindow>,
    /// The single number this provider contributes to the panel display.
    pub panel_metric: Option<f32>,
    pub plan: Option<String>,
}

impl QuotaSnapshot {
    pub fn not_configured(provider: Provider) -> Self {
        Self {
            provider,
            status: ProviderStatus::NotConfigured,
            windows: Vec::new(),
            panel_metric: None,
            plan: None,
        }
    }

    pub fn stale(provider: Provider) -> Self {
        Self {
            provider,
            status: ProviderStatus::Stale,
            windows: Vec::new(),
            panel_metric: None,
            plan: None,
        }
    }

    pub fn error(provider: Provider, message: impl Into<String>) -> Self {
        Self {
            provider,
            status: ProviderStatus::Error(message.into()),
            windows: Vec::new(),
            panel_metric: None,
            plan: None,
        }
    }
}

/// Windows + panel metric extracted from one provider's usage response.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedUsage {
    pub windows: Vec<QuotaWindow>,
    pub panel_metric: Option<f32>,
    pub plan: Option<String>,
}

/// Fetch every provider concurrently. Always returns one snapshot per
/// provider in a fixed order; failures degrade to status values.
pub async fn fetch_all() -> Vec<QuotaSnapshot> {
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .user_agent("cosmic-applet-ai-quota/0.1")
        .build()
    {
        Ok(client) => client,
        Err(e) => {
            let msg = format!("http client: {e}");
            return vec![
                QuotaSnapshot::error(Provider::Claude, msg.clone()),
                QuotaSnapshot::error(Provider::Codex, msg.clone()),
                QuotaSnapshot::error(Provider::Gemini, msg),
            ];
        }
    };

    let (claude, codex, gemini) = tokio::join!(
        claude::fetch(&client),
        codex::fetch(&client),
        gemini::fetch(&client),
    );
    vec![claude, codex, gemini]
}

/// Merge a new fetch round into the previous state: a Stale/Error result
/// keeps the last known windows (marked Stale) rather than blanking the UI.
pub fn merge(old: &[QuotaSnapshot], new: Vec<QuotaSnapshot>) -> Vec<QuotaSnapshot> {
    new.into_iter()
        .map(|snapshot| match snapshot.status {
            ProviderStatus::Stale | ProviderStatus::Error(_) => {
                let previous = old
                    .iter()
                    .find(|o| o.provider == snapshot.provider && !o.windows.is_empty());
                match previous {
                    Some(prev) => QuotaSnapshot {
                        provider: snapshot.provider,
                        status: ProviderStatus::Stale,
                        windows: prev.windows.clone(),
                        panel_metric: prev.panel_metric,
                        plan: prev.plan.clone(),
                    },
                    None => snapshot,
                }
            }
            _ => snapshot,
        })
        .collect()
}

/// Highest panel metric across providers with data, plus whether that data
/// includes anything stale. Drives the panel button text.
pub fn worst_metric(snapshots: &[QuotaSnapshot]) -> Option<(f32, bool)> {
    let mut worst: Option<f32> = None;
    let mut any_stale = false;
    for s in snapshots {
        if let Some(metric) = s.panel_metric {
            if matches!(s.status, ProviderStatus::Ok | ProviderStatus::Stale) {
                if s.status == ProviderStatus::Stale {
                    any_stale = true;
                }
                worst = Some(worst.map_or(metric, |w: f32| w.max(metric)));
            }
        }
    }
    worst.map(|w| (w, any_stale))
}

pub(crate) fn home_path(relative: &str) -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(relative))
}

pub(crate) fn parse_rfc3339(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

pub(crate) fn epoch_seconds(value: i64) -> Option<DateTime<Utc>> {
    DateTime::<Utc>::from_timestamp(value, 0)
}

/// A window's reset time, accepting RFC3339 strings or unix-second numbers.
pub(crate) fn parse_reset(value: Option<&Value>) -> Option<DateTime<Utc>> {
    match value? {
        Value::String(s) => parse_rfc3339(s),
        Value::Number(n) => n.as_i64().and_then(epoch_seconds),
        _ => None,
    }
}

pub(crate) fn now_millis() -> i64 {
    Utc::now().timestamp_millis()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(label: &str, used: f32) -> QuotaWindow {
        QuotaWindow {
            label: label.into(),
            used_percent: used,
            resets_at: None,
        }
    }

    fn ok_snapshot(provider: Provider, used: f32) -> QuotaSnapshot {
        QuotaSnapshot {
            provider,
            status: ProviderStatus::Ok,
            windows: vec![window("5-hour", used)],
            panel_metric: Some(used),
            plan: Some("max".into()),
        }
    }

    #[test]
    fn merge_keeps_previous_windows_when_new_is_stale() {
        let old = vec![ok_snapshot(Provider::Claude, 72.0)];
        let merged = merge(&old, vec![QuotaSnapshot::stale(Provider::Claude)]);
        assert_eq!(merged[0].status, ProviderStatus::Stale);
        assert_eq!(merged[0].windows, old[0].windows);
        assert_eq!(merged[0].panel_metric, Some(72.0));
        assert_eq!(merged[0].plan.as_deref(), Some("max"));
    }

    #[test]
    fn merge_keeps_previous_windows_when_new_is_error() {
        let old = vec![ok_snapshot(Provider::Codex, 18.0)];
        let merged = merge(
            &old,
            vec![QuotaSnapshot::error(Provider::Codex, "timeout")],
        );
        assert_eq!(merged[0].status, ProviderStatus::Stale);
        assert_eq!(merged[0].panel_metric, Some(18.0));
    }

    #[test]
    fn merge_passes_through_error_with_no_history() {
        let merged = merge(&[], vec![QuotaSnapshot::error(Provider::Gemini, "boom")]);
        assert_eq!(
            merged[0].status,
            ProviderStatus::Error("boom".into())
        );
        assert!(merged[0].windows.is_empty());
    }

    #[test]
    fn merge_replaces_on_fresh_ok() {
        let old = vec![ok_snapshot(Provider::Claude, 72.0)];
        let merged = merge(&old, vec![ok_snapshot(Provider::Claude, 10.0)]);
        assert_eq!(merged[0].panel_metric, Some(10.0));
        assert_eq!(merged[0].status, ProviderStatus::Ok);
    }

    #[test]
    fn worst_metric_picks_max_and_flags_stale() {
        let mut stale = ok_snapshot(Provider::Codex, 90.0);
        stale.status = ProviderStatus::Stale;
        let snapshots = vec![ok_snapshot(Provider::Claude, 72.0), stale];
        assert_eq!(worst_metric(&snapshots), Some((90.0, true)));
    }

    #[test]
    fn worst_metric_ignores_not_configured_and_empty() {
        let snapshots = vec![QuotaSnapshot::not_configured(Provider::Gemini)];
        assert_eq!(worst_metric(&snapshots), None);
    }

    #[test]
    fn parse_reset_handles_string_and_number() {
        let s = Value::String("2026-06-10T17:00:00Z".into());
        let n = Value::Number(1718040000.into());
        assert!(parse_reset(Some(&s)).is_some());
        assert_eq!(
            parse_reset(Some(&n)),
            DateTime::<Utc>::from_timestamp(1718040000, 0)
        );
        assert_eq!(parse_reset(None), None);
    }
}
