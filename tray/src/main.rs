// SPDX-License-Identifier: MIT
//! Cross-desktop system-tray indicator for AI subscription quota.
//!
//! Reuses the `quota-providers` core (the same fetch/parse code the COSMIC
//! applet uses) and exposes a StatusNotifierItem via `ksni`, so it shows up in
//! the system tray of KDE Plasma, COSMIC, GNOME (with AppIndicator), XFCE, etc.
//! Strictly read-only and never logs or prints credentials.

mod icon;

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Local, Utc};
use icon::IconState;
use ksni::menu::StandardItem;
use ksni::{Category, Icon, MenuItem, Status, ToolTip, Tray, TrayMethods};
use quota_providers::{
    fetch_all, merge, worst_metric, ProviderStatus, QuotaSnapshot, QuotaWindow,
};
use tokio::sync::mpsc::{self, UnboundedSender};

const REFRESH_INTERVAL: Duration = Duration::from_secs(120);
const PANEL_ICON_NAME: &str = "utilities-system-monitor-symbolic";
const ICON_SIZES: [i32; 2] = [22, 44];
const BAR_WIDTH: usize = 8;

/// Messages from the tray callbacks / timer into the async control loop.
enum Ctrl {
    Refresh,
    Fetched(Vec<QuotaSnapshot>),
    Quit,
}

struct AiQuotaTray {
    snapshots: Vec<QuotaSnapshot>,
    refreshing: bool,
    font: Option<Arc<fontdue::Font>>,
    tx: UnboundedSender<Ctrl>,
}

impl AiQuotaTray {
    fn icon_state(&self) -> IconState {
        match worst_metric(&self.snapshots) {
            Some((value, stale)) => IconState::Percent { value, stale },
            None => IconState::NoData,
        }
    }
}

impl Tray for AiQuotaTray {
    // Left click opens the menu (the SNI spec has no inline text label, so the
    // detail lives in the menu and tooltip).
    const MENU_ON_ACTIVATE: bool = true;

    fn id(&self) -> String {
        env!("CARGO_PKG_NAME").into()
    }

    fn title(&self) -> String {
        "AI Quota".into()
    }

    fn category(&self) -> Category {
        Category::SystemServices
    }

    fn status(&self) -> Status {
        Status::Active
    }

    // Fallback for hosts that ignore the pixmap; our rendered icon takes
    // priority when present.
    fn icon_name(&self) -> String {
        if self.font.is_some() {
            String::new()
        } else {
            PANEL_ICON_NAME.into()
        }
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        if self.font.is_none() {
            return Vec::new();
        }
        let state = self.icon_state();
        ICON_SIZES
            .iter()
            .map(|&s| icon::render(s, state, self.font.as_ref()))
            .collect()
    }

    fn tool_tip(&self) -> ToolTip {
        let title = match worst_metric(&self.snapshots) {
            Some((worst, stale)) => {
                format!("AI Quota — {:.0}%{}", worst, if stale { " (stale)" } else { "" })
            }
            None if self.snapshots.is_empty() => "AI Quota — loading…".into(),
            None => "AI Quota".into(),
        };
        let mut parts = Vec::new();
        for s in &self.snapshots {
            parts.push(format!("{}: {}", s.provider.name(), short_status(s)));
        }
        ToolTip {
            title,
            description: parts.join("\n"),
            icon_name: PANEL_ICON_NAME.into(),
            icon_pixmap: Vec::new(),
        }
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let mut items: Vec<MenuItem<Self>> = Vec::new();

        if self.snapshots.is_empty() {
            items.push(info_item("Loading quota data…".into()));
        }

        for snapshot in &self.snapshots {
            items.push(info_item(provider_header(snapshot)));

            if let ProviderStatus::Error(message) = &snapshot.status {
                items.push(info_item(format!("    error: {message}")));
            }
            for window in &snapshot.windows {
                items.push(info_item(window_row(window)));
            }
        }

        items.push(MenuItem::Separator);

        let refresh_label = if self.refreshing {
            "Refreshing…".to_string()
        } else {
            "Refresh now".to_string()
        };
        items.push(
            StandardItem {
                label: refresh_label,
                enabled: !self.refreshing,
                icon_name: "view-refresh-symbolic".into(),
                activate: Box::new(|t: &mut Self| {
                    let _ = t.tx.send(Ctrl::Refresh);
                }),
                ..Default::default()
            }
            .into(),
        );
        items.push(
            StandardItem {
                label: "Quit".into(),
                icon_name: "application-exit-symbolic".into(),
                activate: Box::new(|t: &mut Self| {
                    let _ = t.tx.send(Ctrl::Quit);
                }),
                ..Default::default()
            }
            .into(),
        );

        items
    }
}

/// A non-interactive label row.
fn info_item(label: String) -> MenuItem<AiQuotaTray> {
    StandardItem {
        label,
        enabled: false,
        ..Default::default()
    }
    .into()
}

fn provider_header(snapshot: &QuotaSnapshot) -> String {
    let mut header = snapshot.provider.name().to_owned();
    if let Some(plan) = &snapshot.plan {
        header.push_str(" · ");
        header.push_str(plan);
    }
    match snapshot.status {
        ProviderStatus::Stale => header.push_str("  (stale)"),
        ProviderStatus::NotConfigured => header.push_str("  — not configured"),
        _ => {}
    }
    header
}

fn window_row(window: &QuotaWindow) -> String {
    let used = window.used_percent.clamp(0.0, 100.0);
    let mut row = format!("    {}  {}  {:>3.0}%", window.label, bar(used), used);
    if let Some(resets_at) = window.resets_at {
        row.push_str("  · ");
        row.push_str(&format_reset(resets_at));
    }
    row
}

fn bar(used: f32) -> String {
    let filled = ((used / 100.0) * BAR_WIDTH as f32).round() as usize;
    let filled = filled.min(BAR_WIDTH);
    let mut s = String::with_capacity(BAR_WIDTH * 3);
    for _ in 0..filled {
        s.push('█');
    }
    for _ in filled..BAR_WIDTH {
        s.push('░');
    }
    s
}

fn short_status(snapshot: &QuotaSnapshot) -> String {
    match &snapshot.status {
        ProviderStatus::NotConfigured => "not configured".into(),
        ProviderStatus::Error(_) => "error".into(),
        ProviderStatus::Ok | ProviderStatus::Stale => match snapshot.panel_metric {
            Some(m) => {
                let stale = matches!(snapshot.status, ProviderStatus::Stale);
                format!("{:.0}%{}", m, if stale { " (stale)" } else { "" })
            }
            None => "—".into(),
        },
    }
}

fn format_reset(resets_at: DateTime<Utc>) -> String {
    let local = resets_at.with_timezone(&Local);
    let now = Local::now();
    if local.date_naive() == now.date_naive() {
        format!("resets {}", local.format("%H:%M"))
    } else {
        format!("resets {}", local.format("%a %H:%M"))
    }
}

/// Load a bold sans-serif system font for rendering the percentage. Returns
/// None if no system font is found (the tray then falls back to a themed icon).
fn load_font() -> Option<fontdue::Font> {
    let mut db = fontdb::Database::new();
    db.load_system_fonts();

    let query = fontdb::Query {
        families: &[fontdb::Family::SansSerif],
        weight: fontdb::Weight::BOLD,
        stretch: fontdb::Stretch::Normal,
        style: fontdb::Style::Normal,
    };
    let id = db.query(&query).or_else(|| db.faces().next().map(|f| f.id))?;

    let bytes = db.with_face_data(id, |data, _index| data.to_vec())?;
    fontdue::Font::from_bytes(bytes, fontdue::FontSettings::default()).ok()
}

#[tokio::main]
async fn main() {
    let font = load_font().map(Arc::new);
    if font.is_none() {
        eprintln!("ai-quota-tray: no system font found; falling back to themed icon");
    }

    let (tx, mut rx) = mpsc::unbounded_channel::<Ctrl>();

    let tray = AiQuotaTray {
        snapshots: Vec::new(),
        refreshing: true,
        font,
        tx: tx.clone(),
    };

    let handle = match tray.spawn().await {
        Ok(handle) => handle,
        Err(e) => {
            eprintln!("ai-quota-tray: failed to register with the system tray: {e}");
            eprintln!("Your desktop needs a StatusNotifierItem host (KDE Plasma has one built in).");
            std::process::exit(1);
        }
    };

    let mut interval = tokio::time::interval(REFRESH_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut fetch_in_flight = false;

    loop {
        tokio::select! {
            // First tick fires immediately -> initial fetch.
            _ = interval.tick() => {
                let _ = tx.send(Ctrl::Refresh);
            }
            msg = rx.recv() => match msg {
                Some(Ctrl::Refresh) => {
                    if fetch_in_flight {
                        continue;
                    }
                    fetch_in_flight = true;
                    handle.update(|t| t.refreshing = true).await;
                    let tx2 = tx.clone();
                    tokio::spawn(async move {
                        let snaps = fetch_all().await;
                        let _ = tx2.send(Ctrl::Fetched(snaps));
                    });
                }
                Some(Ctrl::Fetched(snaps)) => {
                    fetch_in_flight = false;
                    handle
                        .update(move |t| {
                            t.snapshots = merge(&t.snapshots, snaps);
                            t.refreshing = false;
                        })
                        .await;
                }
                Some(Ctrl::Quit) | None => break,
            }
        }
    }

    handle.shutdown().await;
}
