//! Plugin entries in context menus: message (Translate, ReverseImageSearch, ViewRaw,
//! CopyUserURLs, MessageLogger), user (CopyUserURLs, ViewIcons, PermissionsViewer) and server
//! (ServerInfo, ViewIcons). The caller passes its own menu-row widget (`item(ui, label, glyph)`,
//! true when clicked) so the rows look like the rest of its menu. Every entry acts on a click
//! only: copy, open the browser, translate in the chat, or open a local dialog.

use super::windows::{Window, open};
use super::{Plugin, enabled, media, message_logger};
use crate::model::{Id, Message};
use crate::ui::UiCx;

/// A menu row: (ui, label, icon glyph) -> clicked.
pub type Item<'a> = &'a mut dyn FnMut(&mut egui::Ui, &str, &'static str) -> bool;

fn open_url(ui: &egui::Ui, url: &str) {
    ui.ctx().open_url(egui::OpenUrl::new_tab(url));
}

pub fn user_url(id: Id) -> String {
    format!("https://discord.com/users/{}", id.0)
}

/// Image URLs of a message (attachments, then embed images and thumbnails).
fn image_urls(m: &Message) -> Vec<String> {
    let atts = m.attachments.iter().filter(|a| a.is_image()).map(|a| a.url.clone());
    let embeds = m.embeds.iter().flat_map(|e| [e.image.as_ref(), e.thumbnail.as_ref()]).flatten().map(|i| i.url.clone());
    atts.chain(embeds).filter(|u| u.starts_with("https://")).take(4).collect()
}

/// Entries of the message menu, before its "Copy Message ID".
pub fn message_items(ui: &mut egui::Ui, cx: &mut UiCx<'_>, m: &Message, item: Item<'_>) {
    if enabled(Plugin::Translate) && !m.content.is_empty() && item(ui, "Translate", "🌐") {
        // The text goes to the translation service on this click only (translate.rs).
        let locale = cx.state.account.settings.proto_str(12, &[1]);
        super::translate::start(ui.ctx(), m.id, &m.content, crate::ui::messages::readable_text(cx.state, m), locale.as_deref());
    }
    if enabled(Plugin::ReverseImageSearch) {
        let images = image_urls(m);
        if !images.is_empty() {
            ui.menu_button("Search Image", |ui| {
                ui.set_min_width(180.0);
                for (name, engine) in media::ENGINES {
                    if item(ui, name, "🔍") {
                        open_url(ui, &media::search_url(engine, &images[0]));
                    }
                }
                ui.separator();
                if item(ui, "All", "🔍") {
                    for (_, engine) in media::ENGINES {
                        open_url(ui, &media::search_url(engine, &images[0]));
                    }
                }
            });
        }
    }
    if enabled(Plugin::CopyUserUrls) && item(ui, "Copy User URL", "👤") {
        ui.ctx().copy_text(user_url(m.author.id));
    }
    if enabled(Plugin::ViewRaw) && item(ui, "View Raw", "{}") {
        let text = serde_json::to_string_pretty(m).unwrap_or_default();
        open(ui.ctx(), Window::Raw { title: "Raw Message".into(), text });
    }
    if message_logger::is_deleted(m.id) && item(ui, "Remove Deleted Message", "🗑") {
        message_logger::forget(cx.state, m.channel_id, m.id);
    }
}

/// Entries of a user's menu (member list, voice users, mentions), before "Copy User ID".
pub fn user_items(ui: &mut egui::Ui, cx: &mut UiCx<'_>, user_id: Id, guild_id: Option<Id>, item: Item<'_>) {
    if let Err(e) = super::voice::volume_slider(ui, cx.state, user_id) {
        cx.view.toast(format!("Could not save plugin settings: {e}"), true);
    }
    if enabled(Plugin::CopyUserUrls) && item(ui, "Copy User URL", "👤") {
        ui.ctx().copy_text(user_url(user_id));
    }
    if enabled(Plugin::ViewIcons) {
        let user = cx.state.users.get(&user_id).or(cx.state.me.as_ref().filter(|m| m.id == user_id)).cloned();
        if let Some(u) = user {
            if item(ui, "View Avatar", "🖼") {
                crate::ui::viewer::open_icon(ui.ctx(), cx, media::avatar_full(&u), 1.0);
            }
            let member = guild_id.and_then(|g| cx.state.find_member(g, user_id).map(|m| (g, m.clone())));
            if let Some(url) = member.and_then(|(g, m)| media::member_avatar_full(g, user_id, &m))
                && item(ui, "View Server Avatar", "🖼")
            {
                crate::ui::viewer::open_icon(ui.ctx(), cx, url, 1.0);
            }
            if let Some(url) = media::banner_full(&u)
                && item(ui, "View Banner", "🖼")
            {
                crate::ui::viewer::open_icon(ui.ctx(), cx, url, 2.5);
            }
        }
    }
    if enabled(Plugin::PermissionsViewer)
        && let Some(g) = guild_id
        && item(ui, "View Permissions", "🛡")
    {
        open(ui.ctx(), Window::Permissions { guild_id: g, user_id });
    }
}

/// Entries of a server's menu (server header / icon).
pub fn guild_items(ui: &mut egui::Ui, cx: &mut UiCx<'_>, guild_id: Id, item: Item<'_>) {
    if enabled(Plugin::ServerInfo) && item(ui, "Server Info", "ℹ") {
        open(ui.ctx(), Window::ServerInfo { guild_id });
    }
    if enabled(Plugin::ViewIcons)
        && let Some(url) = cx.state.guild(guild_id).and_then(media::guild_icon_full)
        && item(ui, "View Icon", "🖼")
    {
        crate::ui::viewer::open_icon(ui.ctx(), cx, url, 1.0);
    }
    if enabled(Plugin::PermissionsViewer)
        && let Some(me) = cx.state.me.as_ref().map(|m| m.id)
        && item(ui, "View My Permissions", "🛡")
    {
        open(ui.ctx(), Window::Permissions { guild_id, user_id: me });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Attachment, Embed, EmbedMedia};

    #[test]
    fn image_urls_take_attachments_then_embeds() {
        let m = Message {
            attachments: vec![
                Attachment { url: "https://cdn/a.png".into(), filename: "a.png".into(), ..Default::default() },
                Attachment { url: "https://cdn/b.txt".into(), filename: "b.txt".into(), ..Default::default() },
            ],
            embeds: vec![Embed { image: Some(EmbedMedia { url: "https://x/e.jpg".into(), ..Default::default() }), ..Default::default() }],
            ..Default::default()
        };
        assert_eq!(image_urls(&m), ["https://cdn/a.png", "https://x/e.jpg"]);
        assert_eq!(user_url(Id(42)), "https://discord.com/users/42");
    }
}
