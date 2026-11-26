//! Images and links: ImageZoom (magnifier lens), ReverseImageSearch (opens the
//! browser on a click), ViewIcons (full-size CDN URLs), and the link rewrite every opened link
//! goes through (ClearURLs, OpenInApp). Nothing here sends a request on its own.

use super::{Plugin, enabled, opt_bool, opt_choice, opt_int};
use crate::model::{Guild, Id, Member, User};
use egui::{Color32, Pos2, Rect, Stroke, Vec2, vec2};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};

fn encode(s: &str) -> String {
    utf8_percent_encode(s, NON_ALPHANUMERIC).to_string()
}

// ---------------------------------------------------------------------------
// Links (ClearURLs, OpenInApp)
// ---------------------------------------------------------------------------

/// A link about to be opened: tracking params stripped (ClearURLs), Spotify / Steam links
/// turned into their app's URI (OpenInApp).
pub fn rewrite_link(url: &str) -> String {
    let url = if enabled(Plugin::ClearUrls) { super::clear_urls::clean(url).into_owned() } else { url.to_owned() };
    if !enabled(Plugin::OpenInApp) {
        return url;
    }
    open_in_app(&url, opt_bool(Plugin::OpenInApp, "spotify"), opt_bool(Plugin::OpenInApp, "steam")).unwrap_or(url)
}

fn open_in_app(url: &str, spotify: bool, steam: bool) -> Option<String> {
    let rest = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://"))?;
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let path = path.split(['?', '#']).next().unwrap_or("");
    match host {
        "open.spotify.com" if spotify => {
            // open.spotify.com/intl-de/track/<id> -> spotify:track:<id>
            let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty() && !p.starts_with("intl-")).collect();
            match parts.as_slice() {
                [kind @ ("track" | "album" | "artist" | "playlist" | "episode" | "show" | "user"), id, ..] => Some(format!("spotify:{kind}:{id}")),
                _ => None,
            }
        }
        "store.steampowered.com" | "steamcommunity.com" | "help.steampowered.com" if steam => Some(format!("steam://openurl/{url}")),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// ReverseImageSearch
// ---------------------------------------------------------------------------

pub const ENGINES: [(&str, &str); 7] = [
    ("Google Lens", "https://lens.google.com/uploadbyurl?url="),
    ("Yandex", "https://yandex.com/images/search?rpt=imageview&url="),
    ("SauceNAO", "https://saucenao.com/search.php?url="),
    ("IQDB", "https://iqdb.org/?url="),
    ("Bing", "https://www.bing.com/images/search?view=detailv2&iss=sbi&q=imgurl:"),
    ("TinEye", "https://www.tineye.com/search?url="),
    ("ImgOps", "https://imgops.com/start?url="),
];

pub fn search_url(engine: &str, image_url: &str) -> String {
    // ImgOps takes the bare URL as its path.
    if engine.starts_with("https://imgops.com") { format!("{engine}{image_url}") } else { format!("{engine}{}", encode(image_url)) }
}

// ---------------------------------------------------------------------------
// ViewIcons
// ---------------------------------------------------------------------------

fn icon_size() -> u32 {
    if opt_choice(Plugin::ViewIcons, "size") == 1 { 4096 } else { 1024 }
}

/// Still images in the chosen format (animated ones stay gif).
fn with_format(url: String) -> String {
    if opt_choice(Plugin::ViewIcons, "format") == 1 { url.replacen(".webp?", ".png?", 1) } else { url }
}

pub fn avatar_full(user: &User) -> String {
    with_format(crate::api::cdn::avatar_url(user, icon_size(), true))
}

pub fn member_avatar_full(guild_id: Id, user_id: Id, member: &Member) -> Option<String> {
    let hash = member.avatar.as_deref()?;
    Some(with_format(crate::api::cdn::member_avatar_url(guild_id, user_id, hash, icon_size(), true)))
}

pub fn banner_full(user: &User) -> Option<String> {
    let hash = user.banner.as_deref().filter(|h| !h.is_empty())?;
    Some(with_format(crate::api::cdn::user_banner_url(user.id, hash, icon_size())))
}

pub fn guild_icon_full(g: &Guild) -> Option<String> {
    crate::api::cdn::guild_icon_url(g, icon_size(), true).map(with_format)
}

/// ViewIcons: `rect` (a profile avatar or banner, `aspect` = width / height) opens `url()` in the lightbox on a click.
pub fn icon_link(ui: &egui::Ui, cx: &mut crate::ui::UiCx<'_>, rect: Rect, tip: &str, aspect: f32, url: impl FnOnce() -> Option<String>) {
    if !enabled(Plugin::ViewIcons) {
        return;
    }
    let Some(u) = url() else { return };
    let resp = ui.interact(rect, ui.make_persistent_id(("rc_view_icon", tip)), egui::Sense::click()).on_hover_text(tip);
    if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        crate::ui::viewer::open_icon(ui.ctx(), cx, u, aspect);
    }
}

// ---------------------------------------------------------------------------
// ImageZoom
// ---------------------------------------------------------------------------

/// Lens zoom of this session (wheel), starting at the option.
fn zoom_id() -> egui::Id {
    egui::Id::unique("rc_plugin_zoom")
}

/// ImageZoom: while the mouse is held on an image (`resp` from `images::fit_image` of `url`),
/// a lens magnifies the part under the pointer; the wheel zooms (Shift + wheel: lens size).
/// True while the lens shows and on the click that ends it (the caller then opens nothing).
/// shortcut: magnifies the loaded preview, not the original file; load the original when blurry zooms bother.
pub fn image_zoom(ui: &egui::Ui, images: &mut crate::ui::images::ImageCache, resp: &egui::Response, url: &str) -> bool {
    if !enabled(Plugin::ImageZoom) {
        return false;
    }
    let ctx = ui.ctx();
    let shown_id = resp.id.with("rc_zoom_shown");
    if !resp.is_pointer_button_down_on() {
        let shown = ctx.data_mut(|d| d.remove_temp::<bool>(shown_id)).unwrap_or(false);
        return shown && resp.clicked();
    }
    let held = ctx.input(|i| i.pointer.press_start_time().map_or(0.0, |t| i.time - t));
    let Some(pointer) = ctx.pointer_latest_pos().filter(|p| resp.rect.contains(*p)) else { return false };
    if held < 0.15 {
        ctx.request_repaint_after(std::time::Duration::from_millis(160));
        return false;
    }
    let Some(tex) = images.get(ctx, url, 1, false) else { return false };
    ctx.data_mut(|d| d.insert_temp(shown_id, true));
    let (mut zoom, mut size) = ctx.data(|d| d.get_temp::<(f32, f32)>(zoom_id())).unwrap_or((opt_int(Plugin::ImageZoom, "zoom") as f32, opt_int(Plugin::ImageZoom, "size") as f32));
    let (scroll, shift) = ctx.input(|i| (i.smooth_scroll_delta.y, i.modifiers.shift));
    if scroll != 0.0 {
        if shift {
            size = (size + scroll).clamp(50.0, 600.0);
        } else {
            zoom = (zoom * (1.0 + scroll / 400.0)).clamp(1.0, 50.0);
        }
        ctx.data_mut(|d| d.insert_temp(zoom_id(), (zoom, size)));
        // The wheel zooms instead of scrolling the message list.
        ctx.input_mut(|i| i.smooth_scroll_delta = Vec2::ZERO);
    }
    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::unique("rc_plugin_zoom_lens")));
    paint_lens(&painter, tex.id, resp.rect, pointer, zoom, size, opt_bool(Plugin::ImageZoom, "square"));
    ctx.set_cursor_icon(egui::CursorIcon::None);
    true
}

/// The lens: the texture around `at` (inside `image`) scaled by `zoom`, `size` px wide.
fn paint_lens(p: &egui::Painter, tex: egui::TextureId, image: Rect, at: Pos2, zoom: f32, size: f32, square: bool) {
    let uv_at = ((at - image.min) / image.size()).to_pos2();
    let uv_per_px = vec2(1.0 / (image.width() * zoom), 1.0 / (image.height() * zoom));
    let r = size / 2.0;
    let border = Stroke::new(2.0, Color32::from_gray(230));
    if square {
        let rect = Rect::from_center_size(at, Vec2::splat(size));
        let uv = Rect::from_center_size(uv_at, vec2(size * uv_per_px.x, size * uv_per_px.y));
        p.rect_filled(rect, 4, Color32::BLACK);
        p.image(tex, rect, uv, Color32::WHITE);
        p.rect_stroke(rect, 4, border, egui::StrokeKind::Outside);
        return;
    }
    let mut mesh = egui::Mesh::with_texture(tex);
    mesh.colored_vertex(at, Color32::BLACK);
    mesh.vertices[0].uv = uv_at;
    const N: u32 = 48;
    for i in 0..=N {
        let d = Vec2::angled(std::f32::consts::TAU * i as f32 / N as f32) * r;
        let uv = uv_at + vec2(d.x * uv_per_px.x, d.y * uv_per_px.y);
        mesh.vertices.push(egui::epaint::Vertex { pos: at + d, uv, color: Color32::WHITE });
        if i > 0 {
            mesh.add_triangle(0, i, i + 1);
        }
    }
    // Outside the texture (edges) the lens shows black, not stretched pixels.
    for v in &mut mesh.vertices {
        if !(0.0..=1.0).contains(&v.uv.x) || !(0.0..=1.0).contains(&v.uv.y) {
            v.color = Color32::BLACK;
        }
    }
    mesh.vertices[0].color = Color32::WHITE;
    p.circle_filled(at, r, Color32::BLACK);
    p.add(mesh);
    p.circle_stroke(at, r, border);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_links_and_search_urls() {
        assert_eq!(open_in_app("https://open.spotify.com/intl-de/track/abc?si=1", true, true).as_deref(), Some("spotify:track:abc"));
        assert_eq!(open_in_app("https://open.spotify.com/track/abc", false, true), None);
        assert_eq!(open_in_app("https://store.steampowered.com/app/10/", true, true).as_deref(), Some("steam://openurl/https://store.steampowered.com/app/10/"));
        assert_eq!(open_in_app("https://example.com/track/1", true, true), None);
        assert_eq!(search_url(ENGINES[0].1, "https://cdn.x/a b.png"), "https://lens.google.com/uploadbyurl?url=https%3A%2F%2Fcdn%2Ex%2Fa%20b%2Epng");
        // Plugins off (test default): links pass through unchanged.
        assert_eq!(rewrite_link("https://a.com/?utm_source=1"), "https://a.com/?utm_source=1");
    }
}
