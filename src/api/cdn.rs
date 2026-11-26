//! CDN / media-proxy URL builders. Pure functions (used by ui/).
//! `size` must be a power of two between 16 and 4096 (rounded up internally).
//!
//! OWNERSHIP: api implementer.

use crate::model::{Activity, Attachment, Emoji, EmbedMedia, Guild, Id, User};
use reqwest::Url;

pub const CDN: &str = "https://cdn.discordapp.com";
pub const MEDIA_PROXY: &str = "https://media.discordapp.net";

/// Round up to a power of two in 16..=4096 (the CDN rejects other sizes).
pub fn cdn_size(size: u32) -> u32 {
    size.clamp(16, 4096).next_power_of_two()
}

/// `.gif` for animated hashes when animation is wanted, else a still `.webp`.
fn hashed_asset(path: &str, hash: &str, size: u32, animated: bool) -> String {
    let ext = if animated && hash.starts_with("a_") { "gif" } else { "webp" };
    format!("{CDN}/{path}/{hash}.{ext}?size={}", cdn_size(size))
}

/// Avatar (animated hashes -> gif when `animated`), or the default avatar.
pub fn avatar_url(user: &User, size: u32, animated: bool) -> String {
    match user.avatar.as_deref().filter(|h| !h.is_empty()) {
        Some(hash) => hashed_asset(&format!("avatars/{}", user.id), hash, size, animated),
        None => default_avatar_url(user),
    }
}

/// Guild-specific avatar of a member, if set.
pub fn member_avatar_url(guild_id: Id, user_id: Id, hash: &str, size: u32, animated: bool) -> String {
    hashed_asset(&format!("guilds/{guild_id}/users/{user_id}/avatars"), hash, size, animated)
}

pub fn default_avatar_url(user: &User) -> String {
    let legacy = user.discriminator.as_deref().and_then(|d| d.parse::<u64>().ok()).filter(|&d| d != 0);
    let index = match legacy {
        Some(d) => d % 5,
        // Migrated (discriminator "0") users. The parentheses matter: `%` binds tighter than `>>`.
        None => (user.id.0 >> 22) % 6,
    };
    format!("{CDN}/embed/avatars/{index}.png")
}

pub fn guild_icon_url(guild: &Guild, size: u32, animated: bool) -> Option<String> {
    let hash = guild.icon.as_deref().filter(|h| !h.is_empty())?;
    Some(hashed_asset(&format!("icons/{}", guild.id), hash, size, animated))
}

/// `guild_icon_url` from a bare hash (a guild profile's `icon_hash`).
pub fn guild_icon_hash_url(guild_id: Id, hash: &str, size: u32, animated: bool) -> Option<String> {
    plain(hash).then(|| hashed_asset(&format!("icons/{guild_id}"), hash, size, animated))
}

/// A role's icon image (`Role::icon`), drawn at `size` px.
pub fn role_icon_url(role_id: Id, hash: &str, size: u32) -> Option<String> {
    plain(hash).then(|| hashed_asset(&format!("role-icons/{role_id}"), hash, size, false))
}

/// A guild profile's banner (`custom_banner_hash`), the tag card's band.
pub fn discovery_splash_url(guild_id: Id, hash: &str, size: u32) -> Option<String> {
    plain(hash).then(|| format!("{CDN}/discovery-splashes/{guild_id}/{hash}.png?size={}", cdn_size(size)))
}

pub fn group_dm_icon_url(channel_id: Id, hash: &str, size: u32) -> String {
    hashed_asset(&format!("channel-icons/{channel_id}"), hash, size, false)
}

/// WebP is what Discord recommends for emoji (some are only served as WebP).
pub fn emoji_url(id: Id, animated: bool, size: u32) -> String {
    let anim = if animated { "&animated=true" } else { "" };
    format!("{CDN}/emojis/{id}.webp?size={}&quality=lossless{anim}", cdn_size(size))
}

/// Reaction emoji: Some(url) for custom emoji, None for unicode.
pub fn reaction_emoji_url(e: &Emoji, size: u32) -> Option<String> {
    e.id.map(|id| emoji_url(id, e.animated, size))
}

pub fn user_banner_url(user_id: Id, hash: &str, size: u32) -> String {
    hashed_asset(&format!("banners/{user_id}"), hash, size, true)
}

/// Largest size <= (max_w, max_h) with the image's aspect ratio, never upscaled.
/// Unknown dimensions give the box itself (the proxy treats it as a maximum).
fn fit(width: Option<u32>, height: Option<u32>, max_w: u32, max_h: u32) -> (u32, u32) {
    let (max_w, max_h) = (max_w.max(1), max_h.max(1));
    match (width, height) {
        (Some(w), Some(h)) if w > 0 && h > 0 => {
            let scale = (max_w as f64 / w as f64).min(max_h as f64 / h as f64).min(1.0);
            (((w as f64 * scale).round() as u32).max(1), ((h as f64 * scale).round() as u32).max(1))
        }
        _ => (max_w, max_h),
    }
}

/// Append query pairs properly (signed URLs end with `&`, so no string concatenation).
fn with_params(url: &str, params: &[(&str, String)]) -> Option<String> {
    let mut u = Url::parse(url).ok()?;
    {
        let mut q = u.query_pairs_mut();
        for (k, v) in params {
            q.append_pair(k, v);
        }
    }
    Some(u.into())
}

/// Only Discord's own media hosts; anything else would leak the user's IP to third parties.
pub(crate) fn is_discord_media_host(url: &str) -> bool {
    let Ok(u) = Url::parse(url) else { return false };
    if u.scheme() != "https" {
        return false;
    }
    match u.host_str() {
        Some(h) => {
            h == "media.discordapp.net"
                || h == "cdn.discordapp.com"
                || (h.starts_with("images-ext-") && h.ends_with(".discordapp.net"))
        }
        None => false,
    }
}

/// Resized preview of an image attachment through the media proxy (fits in max_w x max_h,
/// keeps aspect). Falls back to `url` if there is no proxy_url.
pub fn attachment_preview_url(att: &Attachment, max_w: u32, max_h: u32) -> String {
    if att.proxy_url.is_empty() || !is_discord_media_host(&att.proxy_url) {
        return att.url.clone();
    }
    let (w, h) = fit(att.width, att.height, max_w, max_h);
    let is_gif = att.content_type.as_deref() == Some("image/gif") || att.filename.to_ascii_lowercase().ends_with(".gif");
    let mut params = Vec::with_capacity(4);
    // The proxy keeps GIFs animated when no format is forced; other images go to WebP.
    if !is_gif {
        params.push(("format", "webp".to_owned()));
        params.push(("quality", "lossless".to_owned()));
    }
    params.push(("width", w.to_string()));
    params.push(("height", h.to_string()));
    with_params(&att.proxy_url, &params).unwrap_or_else(|| att.url.clone())
}

/// Proxied embed media (thumbnail/image) – never hits third-party hosts directly.
pub fn embed_media_url(m: &EmbedMedia, max_w: u32, max_h: u32) -> Option<String> {
    let proxy = m.proxy_url.as_deref().filter(|p| is_discord_media_host(p))?;
    let (w, h) = fit(m.width, m.height, max_w, max_h);
    with_params(proxy, &[("width", w.to_string()), ("height", h.to_string())])
}

/// Rich-presence image (`assets.large_image` / `small_image`): `mp:<path>` through the media
/// proxy, an asset id as `app-assets/{application_id}/{asset}.png`, `spotify:<id>` as Spotify's
/// album art (`i.scdn.co`, where the official client loads it from too). None for the rest
/// (`twitch:`, `youtube:`, … live on other third-party hosts, which would see the user's IP).
pub fn activity_asset_url(application_id: Option<Id>, asset: &str, size: u32) -> Option<String> {
    if let Some(id) = asset.strip_prefix("spotify:") {
        // From other users: exactly an image id (40 hex digits), so the URL stays this one path.
        return (id.len() == 40 && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))).then(|| format!("https://i.scdn.co/image/{id}"));
    }
    if let Some(path) = asset.strip_prefix("mp:") {
        let side = cdn_size(size).to_string();
        // Presence data comes from other users: the result must stay on the proxy host.
        let url = with_params(&format!("{MEDIA_PROXY}/{path}"), &[("width", side.clone()), ("height", side)])?;
        return Some(url).filter(|u| is_discord_media_host(u));
    }
    let app = application_id?;
    (!asset.is_empty() && asset.bytes().all(|b| b.is_ascii_digit())).then(|| format!("{CDN}/app-assets/{app}/{asset}.png?size={}", cdn_size(size)))
}

/// `activity_asset_url` of an activity's large (or small) image.
pub fn activity_image_url(a: &Activity, small: bool, size: u32) -> Option<String> {
    let r = a.rich.as_deref()?;
    activity_asset_url(r.application_id, if small { r.small_image.as_deref() } else { r.large_image.as_deref() }?, size)
}

pub fn sticker_url(id: Id, format_type: u8, size: u32) -> Option<String> {
    let size = cdn_size(size);
    match format_type {
        // PNG and APNG.
        1 | 2 => Some(format!("{CDN}/stickers/{id}.png?size={size}")),
        // GIF stickers are not available through the CDN.
        4 => Some(format!("{MEDIA_PROXY}/stickers/{id}.gif?size={size}")),
        // 3 = Lottie (vector JSON): not rendered.
        _ => None,
    }
}

// ----- profile cosmetics (docs/COSMETICS_NOTES.md). The pieces come from other users' data:
// each is checked so it stays one path segment (or a plain relative path) on the CDN host.

/// `[A-Za-z0-9_]+`: hashes, SKU-like ids.
fn plain(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// Relative CDN path of letters, digits, `_ - . /`, without `..` or a leading `/`.
fn plain_path(s: &str) -> bool {
    !s.is_empty() && !s.starts_with('/') && !s.contains("..") && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"_-./".contains(&b))
}

/// Guild tag badge (§2), drawn at `size` px.
pub fn guild_tag_badge_url(guild_id: Id, badge: &str, size: u32) -> Option<String> {
    plain(badge).then(|| format!("{CDN}/clan-badges/{guild_id}/{badge}.png?size={}", cdn_size(size)))
}

/// Avatar decoration (§3) for a `px`-wide square: static PNG, or the APNG (`passthrough=true`)
/// when `animated` and the asset is an animated one.
pub fn decoration_url(asset: &str, px: u32, animated: bool) -> Option<String> {
    let pass = animated && asset.starts_with("a_");
    plain(asset).then(|| format!("{CDN}/avatar-decoration-presets/{asset}.png?size={}&passthrough={pass}", cdn_size(px)))
}

/// Nameplate image (§4): `static.png` at rest, the APNG `img.png` when `animated` (`size` is
/// ignored on this path).
pub fn nameplate_url(asset: &str, animated: bool) -> Option<String> {
    let file = if animated { "img.png" } else { "static.png" };
    (plain_path(asset) && asset.ends_with('/')).then(|| format!("{CDN}/assets/collectibles/{asset}{file}"))
}

/// Profile badge icon (§9), 72×72.
pub fn badge_icon_url(icon: &str) -> Option<String> {
    plain(icon).then(|| format!("{CDN}/badge-icons/{icon}.png"))
}

/// Application (game) icon.
pub fn app_icon_url(app_id: Id, hash: &str, size: u32) -> Option<String> {
    plain(hash).then(|| format!("{CDN}/app-icons/{app_id}/{hash}.png?size={}", cdn_size(size)))
}

/// A collectible asset path (effect layers, §5): relative paths live on the CDN, absolute ones
/// only on Discord's media hosts.
pub fn collectible_asset_url(src: &str) -> Option<String> {
    if src.starts_with("https://") {
        return is_discord_media_host(src).then(|| src.to_owned());
    }
    let rel = src.trim_start_matches('/');
    plain_path(rel).then(|| format!("{CDN}/{rel}"))
}

/// A profile-frame layer image (§6), by SKU and layer id.
pub fn frame_layer_url(sku_id: Id, layer: &str) -> Option<String> {
    plain(layer).then(|| format!("{CDN}/media/v1/collectibles-shop/{sku_id}/{layer}/static"))
}

/// A game cover from `/games` (only from a Discord media host).
pub fn game_cover_url(url: &str) -> Option<String> {
    is_discord_media_host(url).then(|| url.to_owned())
}

/// A quest asset (`assets.hero`, `game_tile`, a task video), built like the web client's own
/// function (read from discord.com's public JS bundle, QUESTS_NOTES §1.5): an absolute URL only
/// on Discord's media hosts; a name with a `/` is a CDN-root path (`{CDN}/{asset}`); a bare name
/// lives under `{CDN}/quests/{id}/`, with `{theme}/` between for the game tile (`light`/`dark`).
/// (userdoccers' `/assets/quests/` is not what the client requests.) Only with one of `exts`
/// (lowercase, the path's end, query ignored).
fn quest_url(quest_id: Id, asset: &str, theme: Option<&str>, exts: &[&str]) -> Option<String> {
    let url = if asset.starts_with("https://") {
        is_discord_media_host(asset).then(|| asset.to_owned())?
    } else if !plain_path(asset) {
        return None;
    } else if asset.contains('/') {
        format!("{CDN}/{asset}")
    } else {
        let theme = theme.map(|t| format!("{t}/")).unwrap_or_default();
        format!("{CDN}/quests/{quest_id}/{theme}{asset}")
    };
    let path = Url::parse(&url).ok()?.path().to_ascii_lowercase();
    exts.iter().any(|e| path.ends_with(e)).then_some(url)
}

const IMAGE_EXTS: [&str; 5] = [".png", ".jpg", ".jpeg", ".webp", ".gif"];

/// A quest image (the hero still) the image loader can decode; `.webm` / `.mp4` -> None.
pub fn quest_asset_url(quest_id: Id, asset: &str) -> Option<String> {
    quest_url(quest_id, asset, None, &IMAGE_EXTS)
}

/// The game tile: like the client, a bare name sits in a `dark/` or `light/` folder.
pub fn quest_tile_url(quest_id: Id, asset: &str, dark: bool) -> Option<String> {
    quest_url(quest_id, asset, Some(if dark { "dark" } else { "light" }), &IMAGE_EXTS)
}

/// A quest video for the in-app player (MP4 only; never HLS).
pub fn quest_video_url(quest_id: Id, url: &str) -> Option<String> {
    quest_url(quest_id, url, None, &[".mp4"])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_icon_urls() {
        assert_eq!(role_icon_url(Id(7), "abc_1", 32).as_deref(), Some("https://cdn.discordapp.com/role-icons/7/abc_1.webp?size=32"));
        assert_eq!(role_icon_url(Id(7), "a/b", 32), None);
        assert_eq!(role_icon_url(Id(7), "..", 32), None);
    }

    fn user(id: u64, avatar: Option<&str>, discriminator: Option<&str>) -> User {
        User {
            id: Id(id),
            avatar: avatar.map(Into::into),
            discriminator: discriminator.map(Into::into),
            ..User::default()
        }
    }

    #[test]
    fn sizes_round_up_to_power_of_two() {
        assert_eq!(cdn_size(0), 16);
        assert_eq!(cdn_size(16), 16);
        assert_eq!(cdn_size(40), 64);
        assert_eq!(cdn_size(128), 128);
        assert_eq!(cdn_size(129), 256);
        assert_eq!(cdn_size(10_000), 4096);
    }

    #[test]
    fn avatars() {
        let u = user(42, Some("abc"), Some("0"));
        assert_eq!(avatar_url(&u, 40, true), "https://cdn.discordapp.com/avatars/42/abc.webp?size=64");
        let a = user(42, Some("a_abc"), Some("0"));
        assert_eq!(avatar_url(&a, 128, true), "https://cdn.discordapp.com/avatars/42/a_abc.gif?size=128");
        assert_eq!(avatar_url(&a, 128, false), "https://cdn.discordapp.com/avatars/42/a_abc.webp?size=128");
        assert_eq!(
            member_avatar_url(Id(1), Id(2), "a_x", 64, true),
            "https://cdn.discordapp.com/guilds/1/users/2/avatars/a_x.gif?size=64"
        );
    }

    #[test]
    fn default_avatars() {
        // New username system: (id >> 22) % 6.
        let id = 80351110224678912u64;
        let u = user(id, None, Some("0"));
        let expected = (id >> 22) % 6;
        assert_eq!(avatar_url(&u, 64, false), format!("https://cdn.discordapp.com/embed/avatars/{expected}.png"));
        assert_eq!(default_avatar_url(&user(id, None, None)), format!("https://cdn.discordapp.com/embed/avatars/{expected}.png"));
        // Legacy: discriminator % 5.
        assert_eq!(default_avatar_url(&user(id, None, Some("1337"))), "https://cdn.discordapp.com/embed/avatars/2.png");
    }

    #[test]
    fn guild_icon_emoji_banner_sticker() {
        let mut g = Guild { id: Id(7), ..Guild::default() };
        assert_eq!(guild_icon_url(&g, 48, false), None);
        g.icon = Some("a_ff".into());
        assert_eq!(guild_icon_url(&g, 48, true).unwrap(), "https://cdn.discordapp.com/icons/7/a_ff.gif?size=64");
        assert_eq!(
            emoji_url(Id(5), true, 48),
            "https://cdn.discordapp.com/emojis/5.webp?size=64&quality=lossless&animated=true"
        );
        assert_eq!(emoji_url(Id(5), false, 32), "https://cdn.discordapp.com/emojis/5.webp?size=32&quality=lossless");
        assert_eq!(reaction_emoji_url(&Emoji { id: None, name: Some("👍".into()), animated: false }, 32), None);
        assert_eq!(user_banner_url(Id(3), "h", 600), "https://cdn.discordapp.com/banners/3/h.webp?size=1024");
        assert_eq!(group_dm_icon_url(Id(9), "h", 32), "https://cdn.discordapp.com/channel-icons/9/h.webp?size=32");
        assert_eq!(sticker_url(Id(1), 1, 160).unwrap(), "https://cdn.discordapp.com/stickers/1.png?size=256");
        assert_eq!(sticker_url(Id(1), 4, 160).unwrap(), "https://media.discordapp.net/stickers/1.gif?size=256");
        assert_eq!(sticker_url(Id(1), 3, 160), None);
    }

    #[test]
    fn attachment_preview_goes_through_proxy() {
        let att = Attachment {
            filename: "cat.png".into(),
            url: "https://cdn.discordapp.com/attachments/1/2/cat.png?ex=1&is=2&hm=3&".into(),
            proxy_url: "https://media.discordapp.net/attachments/1/2/cat.png?ex=1&is=2&hm=3&".into(),
            width: Some(2000),
            height: Some(1000),
            content_type: Some("image/png".into()),
            ..Attachment::default()
        };
        let u = attachment_preview_url(&att, 400, 300);
        assert!(u.starts_with("https://media.discordapp.net/attachments/1/2/cat.png?ex=1&is=2&hm=3"));
        assert!(u.ends_with("format=webp&quality=lossless&width=400&height=200"), "{u}");

        let gif = Attachment { filename: "a.gif".into(), content_type: Some("image/gif".into()), width: Some(100), height: Some(50), ..att.clone() };
        assert!(attachment_preview_url(&gif, 400, 300).ends_with("&width=100&height=50"));

        let no_proxy = Attachment { proxy_url: String::new(), ..att };
        assert_eq!(attachment_preview_url(&no_proxy, 400, 300), no_proxy.url);
    }

    #[test]
    fn activity_assets() {
        assert_eq!(activity_asset_url(Some(Id(7)), "123", 60).unwrap(), "https://cdn.discordapp.com/app-assets/7/123.png?size=64");
        assert_eq!(
            activity_asset_url(None, "mp:external/abc/https/example.com/a.png", 128).unwrap(),
            "https://media.discordapp.net/external/abc/https/example.com/a.png?width=128&height=128"
        );
        assert!(activity_asset_url(None, "mp:@evil.example/x", 64).unwrap().starts_with("https://media.discordapp.net/@evil.example/x?"));
        assert_eq!(activity_asset_url(None, "123", 64), None, "app assets need the application id");
        let album = "ab67616d0000b2734f8ef2c2c1b3a9a1c4d5e6f7";
        assert_eq!(activity_asset_url(None, &format!("spotify:{album}"), 64).unwrap(), format!("https://i.scdn.co/image/{album}"));
        for other in [
            "spotify:ab67616d",
            "spotify:AB67616D0000B2734F8EF2C2C1B3A9A1C4D5E6F7",
            "spotify:ab67616d0000b2734f8ef2c2c1b3a9a1c4d5e6f7/../x",
            "spotify:ab67616d0000b2734f8ef2c2c1b3a9a1c4d5e6g7",
            "spotify:",
            "twitch:name",
            "https://example.com/a.png",
            "12a",
            "",
        ] {
            assert_eq!(activity_asset_url(Some(Id(7)), other, 64), None, "{other}");
        }
    }

    #[test]
    fn cosmetic_urls() {
        assert_eq!(guild_tag_badge_url(Id(8), "7d17", 12).unwrap(), "https://cdn.discordapp.com/clan-badges/8/7d17.png?size=16");
        assert_eq!(guild_tag_badge_url(Id(8), "../x", 12), None);
        assert_eq!(decoration_url("a_fed", 48, true).unwrap(), "https://cdn.discordapp.com/avatar-decoration-presets/a_fed.png?size=64&passthrough=true");
        assert!(decoration_url("a_fed", 48, false).unwrap().ends_with("passthrough=false"));
        assert!(decoration_url("fed", 48, true).unwrap().ends_with("passthrough=false"), "static asset");
        assert_eq!(decoration_url("x?y=1", 48, true), None);
        assert_eq!(nameplate_url("nameplates/n/angel/", true).unwrap(), "https://cdn.discordapp.com/assets/collectibles/nameplates/n/angel/img.png");
        assert!(nameplate_url("nameplates/n/angel/", false).unwrap().ends_with("/static.png"));
        for bad in ["nameplates/../x/", "/abs/", "no-slash", "a b/"] {
            assert_eq!(nameplate_url(bad, false), None, "{bad}");
        }
        assert_eq!(badge_icon_url("5e74e9b6").unwrap(), "https://cdn.discordapp.com/badge-icons/5e74e9b6.png");
        assert_eq!(app_icon_url(Id(3), "abc", 100).unwrap(), "https://cdn.discordapp.com/app-icons/3/abc.png?size=128");
        assert_eq!(collectible_asset_url("assets/profile_effects/x/intro.png").unwrap(), "https://cdn.discordapp.com/assets/profile_effects/x/intro.png");
        assert_eq!(collectible_asset_url("https://cdn.discordapp.com/a.png").unwrap(), "https://cdn.discordapp.com/a.png");
        assert_eq!(collectible_asset_url("https://example.com/a.png"), None);
        assert_eq!(collectible_asset_url("../etc"), None);
        assert_eq!(frame_layer_url(Id(9), "abc").unwrap(), "https://cdn.discordapp.com/media/v1/collectibles-shop/9/abc/static");
        assert_eq!(game_cover_url("https://images.igdb.com/x.jpg"), None);
    }

    #[test]
    fn embed_media_only_via_discord_proxy() {
        let m = EmbedMedia {
            url: "https://example.com/x.png".into(),
            proxy_url: Some("https://images-ext-1.discordapp.net/external/abc/https/example.com/x.png".into()),
            width: Some(800),
            height: Some(800),
            content_type: None,
        };
        assert_eq!(
            embed_media_url(&m, 400, 300).unwrap(),
            "https://images-ext-1.discordapp.net/external/abc/https/example.com/x.png?width=300&height=300"
        );
        assert_eq!(embed_media_url(&EmbedMedia { proxy_url: None, ..m.clone() }, 400, 300), None);
        let evil = EmbedMedia { proxy_url: Some("https://example.com/x.png".into()), ..m };
        assert_eq!(embed_media_url(&evil, 400, 300), None);
    }

    #[test]
    fn quest_asset_urls() {
        let q = Id(101);
        assert_eq!(quest_asset_url(q, "hero.png").as_deref(), Some("https://cdn.discordapp.com/quests/101/hero.png"));
        assert_eq!(quest_asset_url(q, "quests/101/hero.jpg").as_deref(), Some("https://cdn.discordapp.com/quests/101/hero.jpg"));
        assert_eq!(quest_tile_url(q, "tile.webp", true).as_deref(), Some("https://cdn.discordapp.com/quests/101/dark/tile.webp"));
        assert_eq!(quest_tile_url(q, "quests/101/tile.webp", false).as_deref(), Some("https://cdn.discordapp.com/quests/101/tile.webp"));
        let abs = "https://cdn.discordapp.com/quests/101/tile.JPG?x=1";
        assert_eq!(quest_asset_url(q, abs).as_deref(), Some(abs));
        assert_eq!(quest_asset_url(q, "https://evil.example/hero.png"), None);
        assert_eq!(quest_asset_url(q, "../../avatars/hero.png"), None);
        assert_eq!(quest_asset_url(q, "quests/../../avatars/hero.png"), None);
        assert_eq!(quest_asset_url(q, "hero.webm"), None);
        assert_eq!(quest_asset_url(q, "trailer.mp4"), None);
        assert_eq!(quest_video_url(q, "trailer.mp4").as_deref(), Some("https://cdn.discordapp.com/quests/101/trailer.mp4"));
        assert_eq!(quest_video_url(q, "https://cdn.discordapp.com/quests/101/hls/master.m3u8"), None);
        assert_eq!(quest_video_url(q, "hero.png"), None);
    }

    #[test]
    fn guild_profile_urls() {
        assert_eq!(guild_icon_hash_url(Id(7), "b47f", 100, false).unwrap(), "https://cdn.discordapp.com/icons/7/b47f.webp?size=128");
        assert_eq!(guild_icon_hash_url(Id(7), "../x", 100, false), None);
        assert_eq!(discovery_splash_url(Id(7), "c0ffee", 600).unwrap(), "https://cdn.discordapp.com/discovery-splashes/7/c0ffee.png?size=1024");
    }
}
