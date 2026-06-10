// SPDX-License-Identifier: MIT

use std::sync::LazyLock;
use std::time::Duration;

use chrono::{DateTime, Local, Utc};
use cosmic::iced::platform_specific::shell::wayland::commands::popup::{destroy_popup, get_popup};
use cosmic::iced::{window::Id, Alignment, Length, Limits, Subscription};
use cosmic::prelude::*;
use cosmic::widget::{self, autosize};
use quota_providers::{
    fetch_all, merge, worst_metric, ProviderStatus, QuotaSnapshot, QuotaWindow,
};

const REFRESH_INTERVAL: Duration = Duration::from_secs(120);
const PANEL_ICON: &str = "utilities-system-monitor-symbolic";

static AUTOSIZE_MAIN_ID: LazyLock<widget::Id> = LazyLock::new(|| widget::Id::new("autosize-main"));

#[derive(Default)]
pub struct AppModel {
    core: cosmic::Core,
    popup: Option<Id>,
    snapshots: Vec<QuotaSnapshot>,
    refreshing: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    TogglePopup,
    PopupClosed(Id),
    Refresh,
    Updated(Vec<QuotaSnapshot>),
}

impl cosmic::Application for AppModel {
    type Executor = cosmic::executor::Default;
    type Flags = ();
    type Message = Message;

    const APP_ID: &'static str = "dev.thorsteinson.AiQuotaApplet";

    fn core(&self) -> &cosmic::Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut cosmic::Core {
        &mut self.core
    }

    fn init(
        core: cosmic::Core,
        _flags: Self::Flags,
    ) -> (Self, Task<cosmic::Action<Self::Message>>) {
        let app = AppModel {
            core,
            refreshing: true,
            ..Default::default()
        };
        (
            app,
            cosmic::task::future(async { Message::Updated(fetch_all().await) }),
        )
    }

    fn on_close_requested(&self, id: Id) -> Option<Message> {
        Some(Message::PopupClosed(id))
    }

    fn view(&self) -> Element<'_, Self::Message> {
        let suggested_size = self.core.applet.suggested_size(true);
        let padding = self.core.applet.suggested_padding(true);

        let mut children: Vec<Element<'_, Message>> = vec![widget::icon::from_name(PANEL_ICON)
            .size(suggested_size.0)
            .into()];

        if let Some((worst, any_stale)) = worst_metric(&self.snapshots) {
            let marker = if any_stale { "~" } else { "" };
            children.push(
                self.core
                    .applet
                    .text(format!("{worst:.0}%{marker}"))
                    .into(),
            );
        }

        let button = widget::button::custom(
            widget::Row::with_children(children)
                .spacing(4)
                .align_y(Alignment::Center),
        )
        .padding([0, padding.0])
        .on_press_down(Message::TogglePopup)
        .class(cosmic::theme::Button::AppletIcon);

        autosize::autosize(button, AUTOSIZE_MAIN_ID.clone()).into()
    }

    fn view_window(&self, _id: Id) -> Element<'_, Self::Message> {
        let mut content = widget::Column::new().spacing(12).padding([12, 16]);

        if self.snapshots.is_empty() {
            content = content.push(widget::text::body("Loading quota data…"));
        }

        for snapshot in &self.snapshots {
            content = content.push(provider_section(snapshot));
        }

        let refresh_label = if self.refreshing {
            "Refreshing…"
        } else {
            "Refresh now"
        };
        let mut refresh = widget::button::standard(refresh_label);
        if !self.refreshing {
            refresh = refresh.on_press(Message::Refresh);
        }
        content = content.push(
            widget::Row::new()
                .push(widget::Space::new().width(Length::Fill))
                .push(refresh),
        );

        self.core.applet.popup_container(content).into()
    }

    fn subscription(&self) -> Subscription<Self::Message> {
        cosmic::iced::time::every(REFRESH_INTERVAL).map(|_| Message::Refresh)
    }

    fn update(&mut self, message: Self::Message) -> Task<cosmic::Action<Self::Message>> {
        match message {
            Message::Refresh => {
                if self.refreshing {
                    return Task::none();
                }
                self.refreshing = true;
                return cosmic::task::future(async { Message::Updated(fetch_all().await) });
            }
            Message::Updated(new_snapshots) => {
                self.snapshots = merge(&self.snapshots, new_snapshots);
                self.refreshing = false;
            }
            Message::TogglePopup => {
                return if let Some(popup_id) = self.popup.take() {
                    destroy_popup(popup_id)
                } else {
                    let new_id = Id::unique();
                    self.popup.replace(new_id);
                    let mut popup_settings = self.core.applet.get_popup_settings(
                        self.core.main_window_id().unwrap(),
                        new_id,
                        None,
                        None,
                        None,
                    );
                    popup_settings.positioner.size_limits = Limits::NONE
                        .max_width(420.0)
                        .min_width(340.0)
                        .min_height(120.0)
                        .max_height(1080.0);
                    get_popup(popup_settings)
                };
            }
            Message::PopupClosed(id) => {
                if self.popup.as_ref() == Some(&id) {
                    self.popup = None;
                }
            }
        }
        Task::none()
    }

    fn style(&self) -> Option<cosmic::iced::theme::Style> {
        Some(cosmic::applet::style())
    }
}

fn provider_section(snapshot: &QuotaSnapshot) -> Element<'_, Message> {
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

    let mut section = widget::Column::new()
        .spacing(6)
        .push(widget::text::heading(header));

    if let ProviderStatus::Error(message) = &snapshot.status {
        section = section.push(widget::text::caption(message.clone()));
    }

    for window in &snapshot.windows {
        section = section.push(window_row(window));
    }

    section.into()
}

fn window_row(window: &QuotaWindow) -> Element<'_, Message> {
    let used = window.used_percent.clamp(0.0, 100.0);

    let mut row = widget::Row::new()
        .spacing(8)
        .align_y(Alignment::Center)
        .push(
            widget::text::body(window.label.clone()).width(Length::Fixed(96.0)),
        )
        .push(
            widget::determinate_linear(used / 100.0).width(Length::Fill),
        )
        .push(
            widget::text::body(format!("{used:.0}%")).width(Length::Fixed(44.0)),
        );

    if let Some(resets_at) = window.resets_at {
        row = row.push(widget::text::caption(format_reset(resets_at)));
    }

    row.into()
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
