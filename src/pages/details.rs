//! Application details page module

use std::collections::HashMap;
use std::sync::Arc;

use cosmic::iced::{Alignment, Length};
use cosmic::{Element, Task, cosmic_theme, theme, widget};

use crate::Message;
use crate::app_id::AppId;
use crate::app_info::{
    AppFramework, AppInfo, AppUrl, RiskLevel, WaylandCompatibility, WaylandSupport,
};
use crate::constants::ICON_SIZE_DETAILS;
use crate::fl;
use crate::icon_cache::icon_cache_handle;
use crate::ui::badges::wayland_compat_badge;
use crate::ui::cards::styled_icon;

#[derive(Clone, Debug)]
pub struct SelectedSource {
    pub(crate) backend_name: &'static str,
    pub(crate) source_id: String,
    pub(crate) source_name: String,
}

impl SelectedSource {
    pub fn new(backend_name: &'static str, info: &AppInfo, installed: bool) -> Self {
        Self {
            backend_name,
            source_id: info.source_id.clone(),
            source_name: if installed {
                fl!("source-installed", source = info.source_name.as_str())
            } else {
                info.source_name.clone()
            },
        }
    }
}

impl AsRef<str> for SelectedSource {
    fn as_ref(&self) -> &str {
        &self.source_name
    }
}

#[derive(Clone, Debug)]
pub struct DetailsPage {
    pub(crate) backend_name: &'static str,
    pub(crate) id: AppId,
    pub(crate) icon_opt: Option<widget::icon::Handle>,
    pub(crate) info: Arc<AppInfo>,
    pub(crate) screenshot_images: HashMap<usize, widget::image::Handle>,
    pub(crate) screenshot_shown: usize,
    pub(crate) sources: Vec<SelectedSource>,
    pub(crate) addons: Vec<(AppId, Arc<AppInfo>)>,
    pub(crate) addons_view_more: bool,
}

impl DetailsPage {
    pub fn new(
        backend_name: &'static str,
        id: AppId,
        icon_opt: Option<widget::icon::Handle>,
        info: Arc<AppInfo>,
        sources: Vec<SelectedSource>,
        addons: Vec<(AppId, Arc<AppInfo>)>,
    ) -> Self {
        Self {
            backend_name,
            id,
            icon_opt,
            info,
            screenshot_images: HashMap::new(),
            screenshot_shown: 0,
            sources,
            addons,
            addons_view_more: false,
        }
    }

    pub fn view<'a>(
        &'a self,
        actions: &'a impl DetailsPageActions,
        spacing: cosmic_theme::Spacing,
        grid_width: usize,
        app_stats: &'a HashMap<AppId, (u64, Option<WaylandCompatibility>)>,
    ) -> Element<'a, Message> {
        let cosmic_theme::Spacing {
            space_l: _,
            space_m,
            space_s,
            space_xs,
            space_xxs,
            space_xxxs,
            ..
        } = spacing;

        let mut selected_source = None;
        for (i, source) in self.sources.iter().enumerate() {
            if source.backend_name == self.backend_name && source.source_id == self.info.source_id {
                selected_source = Some(i);
                break;
            }
        }

        let mut column = widget::column::with_capacity(2)
            .padding([0, space_s, space_m, space_s])
            .spacing(space_m)
            .width(Length::Fill);
        column = column.push(
            widget::button::text(fl!("back"))
                .leading_icon(icon_cache_handle("go-previous-symbolic", 16))
                .on_press(Message::SelectNone),
        );

        let buttons = actions.selected_buttons(self.backend_name, &self.id, &self.info, false);

        let mut title_row_children = vec![widget::text::title2(&self.info.name).into()];
        if self.info.source_id == "flathub"
            && let Some(badge) = wayland_compat_badge(&self.info, 24, app_stats)
        {
            title_row_children
                .push(widget::Space::with_width(Length::Fixed(space_xs.into())).into());
            title_row_children.push(badge);
        }

        column = column.push(
            widget::row::with_children(vec![
                match &self.icon_opt {
                    Some(icon) => styled_icon(icon.clone(), ICON_SIZE_DETAILS),
                    None => {
                        widget::Space::with_width(Length::Fixed(ICON_SIZE_DETAILS as f32)).into()
                    }
                },
                widget::column::with_children(vec![
                    widget::row::with_children(title_row_children)
                        .align_y(Alignment::Center)
                        .into(),
                    widget::text(&self.info.summary).into(),
                    widget::Space::with_height(Length::Fixed(space_s.into())).into(),
                    widget::row::with_children(buttons).spacing(space_xs).into(),
                ])
                .into(),
            ])
            .align_y(Alignment::Center)
            .spacing(space_m),
        );

        let sources_widget = widget::column::with_children(vec![if self.sources.len() == 1 {
            widget::text(self.sources[0].as_ref()).into()
        } else {
            widget::dropdown(&self.sources, selected_source, Message::SelectedSource).into()
        }])
        .align_x(Alignment::Center)
        .width(Length::Fill);
        let developers_widget = widget::column::with_children(vec![
            if self.info.developer_name.is_empty() {
                widget::text::heading(fl!("app-developers", app = self.info.name.as_str())).into()
            } else {
                widget::text::heading(&self.info.developer_name).into()
            },
            widget::text::body(fl!("developer")).into(),
        ])
        .align_x(Alignment::Center)
        .width(Length::Fill);
        let downloads_widget =
            (self.info.source_id == "flathub" && self.info.monthly_downloads > 0).then(|| {
                widget::column::with_children(vec![
                    widget::text::heading(self.info.monthly_downloads.to_string()).into(),
                    widget::text::body(fl!("monthly-downloads")).into(),
                ])
                .align_x(Alignment::Center)
                .width(Length::Fill)
            });
        if grid_width < 416 {
            let size = 4 + if downloads_widget.is_some() { 3 } else { 0 };
            let downloads_widget_space = downloads_widget
                .is_some()
                .then(widget::divider::horizontal::default);
            column = column.push(
                widget::column::with_capacity(size)
                    .push(widget::divider::horizontal::default())
                    .push(sources_widget)
                    .push(widget::divider::horizontal::default())
                    .push(developers_widget)
                    .push(widget::divider::horizontal::default())
                    .push_maybe(downloads_widget)
                    .push_maybe(downloads_widget_space)
                    .spacing(space_xxs),
            );
        } else {
            let row_size = 4 + if downloads_widget.is_some() { 2 } else { 0 };
            let downloads_widget_space = downloads_widget
                .is_some()
                .then(|| widget::divider::vertical::default().height(Length::Fixed(32.0)));
            column = column.push(
                widget::column::with_children(vec![
                    widget::divider::horizontal::default().into(),
                    widget::row::with_capacity(row_size)
                        .push(sources_widget)
                        .push(widget::divider::vertical::default().height(Length::Fixed(32.0)))
                        .push(developers_widget)
                        .push_maybe(downloads_widget_space)
                        .push_maybe(downloads_widget)
                        .align_y(Alignment::Center)
                        .into(),
                    widget::divider::horizontal::default().into(),
                ])
                .spacing(space_xxs),
            );
        }

        if let Some(screenshot) = self.info.screenshots.get(self.screenshot_shown) {
            let image_height = Length::Fixed(320.0);
            let mut row = widget::row::with_capacity(3).align_y(Alignment::Center);
            {
                let mut button =
                    widget::button::icon(widget::icon::from_name("go-previous-symbolic").size(16));
                let index = self
                    .screenshot_shown
                    .checked_sub(1)
                    .unwrap_or_else(|| self.info.screenshots.len().saturating_sub(1));
                if index != self.screenshot_shown {
                    button = button.on_press(Message::SelectedScreenshotShown(index));
                }
                row = row.push(button);
            }
            let image_element =
                if let Some(image) = self.screenshot_images.get(&self.screenshot_shown) {
                    widget::container(widget::image(image.clone()))
                        .center_x(Length::Fill)
                        .center_y(image_height)
                        .into()
                } else {
                    widget::Space::new(Length::Fill, image_height).into()
                };
            row = row.push(
                widget::column::with_children(vec![
                    image_element,
                    widget::text::caption(&screenshot.caption).into(),
                ])
                .align_x(Alignment::Center),
            );
            {
                let mut button =
                    widget::button::icon(widget::icon::from_name("go-next-symbolic").size(16));
                let index = if self.screenshot_shown + 1 == self.info.screenshots.len() {
                    0
                } else {
                    self.screenshot_shown + 1
                };
                if index != self.screenshot_shown {
                    button = button.on_press(Message::SelectedScreenshotShown(index));
                }
                row = row.push(button);
            }
            column = column.push(row);
        }
        column = column.push(widget::text::body(&self.info.description));

        if self.info.source_id == "flathub"
            && let Some(compat) = self.info.wayland_compat_lazy()
            && (compat.risk_level == RiskLevel::Critical || compat.risk_level == RiskLevel::High)
        {
            let (title, description, icon_name) =
                if matches!(compat.support, WaylandSupport::X11Only) {
                    (
                        fl!("compatibility-warning"),
                        fl!("x11-only-description"),
                        "dialog-warning-symbolic",
                    )
                } else {
                    let framework_name = match compat.framework {
                        AppFramework::QtWebEngine => fl!("framework-qtwebengine"),
                        AppFramework::Electron => fl!("framework-electron"),
                        _ => fl!("wayland-issues-warning"),
                    };
                    (
                        fl!("wayland-issues-warning"),
                        fl!("wayland-issues-description", framework = framework_name),
                        "dialog-warning-symbolic",
                    )
                };

            let warning_container = widget::container(
                widget::column::with_children(vec![
                    widget::row::with_children(vec![
                        widget::icon::from_name(icon_name).size(24).into(),
                        widget::text::heading(title).width(Length::Fill).into(),
                    ])
                    .spacing(space_s)
                    .into(),
                    widget::text::body(description).into(),
                ])
                .spacing(space_xxs),
            )
            .padding(space_s)
            .class(theme::Container::Card);

            column = column.push(warning_container);
            column = column.push(widget::Space::with_height(Length::Fixed(space_s.into())));
        }

        if !self.addons.is_empty() {
            let mut addon_col = widget::column::with_capacity(2).spacing(space_xxxs);
            addon_col = addon_col.push(widget::text::title4(fl!("addons")));
            let mut list = widget::list_column()
                .divider_padding(0)
                .list_item_padding([space_xxs, 0])
                .style(theme::Container::Transparent);
            let addon_cnt = self.addons.len();
            let take = if self.addons_view_more { addon_cnt } else { 4 };
            for (addon_id, addon_info) in self.addons.iter().take(take) {
                let buttons =
                    actions.selected_buttons(self.backend_name, addon_id, addon_info, true);
                list = list.add(
                    widget::settings::item::builder(&addon_info.name)
                        .description(&addon_info.summary)
                        .control(widget::row::with_children(buttons).spacing(space_xs)),
                );
            }
            if addon_cnt > 4 && !self.addons_view_more {
                list = list.add(
                    widget::button::text(fl!("view-more"))
                        .on_press(Message::SelectedAddonsViewMore(true)),
                );
            }
            addon_col = addon_col.push(list);
            column = column.push(addon_col);
        }

        if let Some(release) = self.info.releases.first() {
            let mut release_col = widget::column::with_capacity(2).spacing(space_xxxs);
            release_col = release_col.push(widget::text::title4(fl!(
                "version",
                version = release.version.as_str()
            )));
            if let Some(timestamp) = release.timestamp
                && let Some(utc) = chrono::DateTime::<chrono::Utc>::from_timestamp(timestamp, 0)
            {
                let local = chrono::DateTime::<chrono::Local>::from(utc);
                release_col = release_col.push(widget::text::body(format!(
                    "{}",
                    local.format("%b %-d, %-Y")
                )));
            }
            if let Some(description) = &release.description {
                release_col = release_col.push(widget::text::body(description));
            }
            column = column.push(release_col);
        }

        if let Some(license) = &self.info.license_opt {
            let mut license_col = widget::column::with_capacity(2).spacing(space_xxxs);
            license_col = license_col.push(widget::text::title4(fl!("licenses")));
            match spdx::Expression::parse_mode(license, spdx::ParseMode::LAX) {
                Ok(expr) => {
                    for item in expr.requirements() {
                        match &item.req.license {
                            spdx::LicenseItem::Spdx { id, .. } => {
                                license_col = license_col.push(widget::text::body(id.full_name));
                            }
                            spdx::LicenseItem::Other { lic_ref, .. } => {
                                let mut parts = lic_ref.splitn(2, '=');
                                parts.next();
                                if let Some(url) = parts.next() {
                                    license_col = license_col.push(
                                        widget::button::link(fl!("proprietary"))
                                            .on_press(Message::LaunchUrl(url.to_string()))
                                            .padding(0),
                                    )
                                } else {
                                    license_col =
                                        license_col.push(widget::text::body(fl!("proprietary")));
                                }
                            }
                        }
                    }
                }
                Err(_) => {
                    license_col = license_col.push(widget::text::body(license));
                }
            }
            column = column.push(license_col);
        }

        if !self.info.urls.is_empty() {
            let mut url_items = Vec::with_capacity(self.info.urls.len());
            for app_url in &self.info.urls {
                let (name, url) = match app_url {
                    AppUrl::BugTracker(url) => (fl!("bug-tracker"), url),
                    AppUrl::Contact(url) => (fl!("contact"), url),
                    AppUrl::Donation(url) => (fl!("donation"), url),
                    AppUrl::Faq(url) => (fl!("faq"), url),
                    AppUrl::Help(url) => (fl!("help"), url),
                    AppUrl::Homepage(url) => (fl!("homepage"), url),
                    AppUrl::Translate(url) => (fl!("translate"), url),
                };
                url_items.push(
                    widget::button::link(name)
                        .on_press(Message::LaunchUrl(url.to_string()))
                        .padding(0)
                        .into(),
                );
            }
            if grid_width < 416 {
                column = column.push(widget::column::with_children(url_items).spacing(space_xxxs));
            } else {
                column = column.push(
                    widget::row::with_children(url_items)
                        .spacing(space_s)
                        .align_y(Alignment::Center),
                );
            }
        }

        column.into()
    }

    pub fn update(&mut self, message: &Message) -> Task<cosmic::Action<Message>> {
        match message {
            Message::SelectedAddonsViewMore(v) => {
                self.addons_view_more = *v;
                Task::none()
            }
            Message::SelectedScreenshot(i, url, data) => {
                if let Some(screenshot) = self.info.screenshots.get(*i)
                    && screenshot.url == *url
                {
                    self.screenshot_images
                        .insert(*i, widget::image::Handle::from_bytes(data.clone()));
                }
                Task::none()
            }
            Message::SelectedScreenshotShown(i) => {
                self.screenshot_shown = *i;
                Task::none()
            }
            _ => Task::none(),
        }
    }
}

pub trait DetailsPageActions {
    fn selected_buttons<'a>(
        &'a self,
        backend_name: &'static str,
        id: &AppId,
        info: &Arc<AppInfo>,
        addon: bool,
    ) -> Vec<Element<'a, Message>>;
}
