//! Domain types shared by api/, gateway/, cache/, state and ui/.
//!
//! These are *canonical* types: REST responses and most gateway payloads deserialize
//! straight into them (lenient: every field has a default, ids accept string or number).
//! Shapes that differ on the wire (READY guilds, DM recipients, member lists) are
//! normalized by gateway/ before they reach the state.
//!
//! OWNERSHIP: shared contract. Implementers may add helper `impl` blocks in their own
//! modules, but must not change existing field names/types here without the integrator.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

// ---------------------------------------------------------------------------
// Snowflake id
// ---------------------------------------------------------------------------

/// Discord snowflake. Serialized as a decimal string; deserializes from string or number.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Id(pub u64);

impl Id {
    pub const DISCORD_EPOCH_MS: u64 = 1_420_070_400_000;

    /// Creation time encoded in the snowflake (unix ms).
    pub fn timestamp_ms(self) -> u64 {
        (self.0 >> 22) + Self::DISCORD_EPOCH_MS
    }

    pub fn is_zero(self) -> bool {
        self.0 == 0
    }

    /// Smallest snowflake at unix ms `ms` (search `before:`/`after:` dates -> `max_id`/`min_id`).
    #[allow(dead_code, reason = "for the search date filters, which are not built yet")]
    pub fn from_timestamp_ms(ms: u64) -> Id {
        Id(ms.saturating_sub(Self::DISCORD_EPOCH_MS) << 22)
    }
}

impl fmt::Debug for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for Id {
    type Err = std::num::ParseIntError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse().map(Id)
    }
}

impl Serialize for Id {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Id {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        u64_lenient(d).map(Id)
    }
}

/// Accepts `"123"`, `123`, or `null` (-> 0). Used for ids and permission bitfields.
pub fn u64_lenient<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    struct V;
    impl serde::de::Visitor<'_> for V {
        type Value = u64;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a u64 as number or string")
        }
        fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<u64, E> {
            Ok(v)
        }
        fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<u64, E> {
            Ok(v as u64)
        }
        fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<u64, E> {
            Ok(v as u64)
        }
        fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<u64, E> {
            if v.is_empty() {
                return Ok(0);
            }
            v.parse().map_err(E::custom)
        }
        fn visit_unit<E: serde::de::Error>(self) -> Result<u64, E> {
            Ok(0)
        }
    }
    d.deserialize_any(V)
}

/// Permission bitfield, string on the wire, u64 in memory.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default, Debug)]
pub struct Perms(pub u64);

impl Serialize for Perms {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(&self.0)
    }
}
impl<'de> Deserialize<'de> for Perms {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        u64_lenient(d).map(Perms)
    }
}

/// `null` -> `T::default()` (serde's `default` only covers *missing* fields).
pub fn null_default<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

pub mod perm {
    //! Permission bits used by the client.
    #![allow(dead_code)] // complete table; not every bit is checked yet
    pub const ADD_REACTIONS: u64 = 1 << 6;
    pub const ADMINISTRATOR: u64 = 1 << 3;
    pub const MANAGE_CHANNELS: u64 = 1 << 4;
    pub const VIEW_CHANNEL: u64 = 1 << 10;
    pub const SEND_MESSAGES: u64 = 1 << 11;
    pub const MANAGE_MESSAGES: u64 = 1 << 13;
    pub const ATTACH_FILES: u64 = 1 << 15;
    pub const READ_MESSAGE_HISTORY: u64 = 1 << 16;
    pub const MENTION_EVERYONE: u64 = 1 << 17;
    pub const USE_EXTERNAL_EMOJIS: u64 = 1 << 18;
    pub const MANAGE_THREADS: u64 = 1 << 34;
    pub const CREATE_PUBLIC_THREADS: u64 = 1 << 35;
    pub const CREATE_PRIVATE_THREADS: u64 = 1 << 36;
    pub const SEND_MESSAGES_IN_THREADS: u64 = 1 << 38;
    pub const BYPASS_SLOWMODE: u64 = 1 << 52;
    // Server administration (FEATURES_NOTES Part 3).
    pub const CREATE_INSTANT_INVITE: u64 = 1 << 0;
    pub const KICK_MEMBERS: u64 = 1 << 1;
    pub const BAN_MEMBERS: u64 = 1 << 2;
    pub const MANAGE_GUILD: u64 = 1 << 5;
    pub const VIEW_AUDIT_LOG: u64 = 1 << 7;
    pub const CHANGE_NICKNAME: u64 = 1 << 26;
    pub const MANAGE_NICKNAMES: u64 = 1 << 27;
    pub const MANAGE_ROLES: u64 = 1 << 28;
    pub const MANAGE_WEBHOOKS: u64 = 1 << 29;
    pub const MANAGE_GUILD_EXPRESSIONS: u64 = 1 << 30;
    pub const MODERATE_MEMBERS: u64 = 1 << 40;
    pub const CREATE_GUILD_EXPRESSIONS: u64 = 1 << 43;
    pub const ALL: u64 = u64::MAX;
}

pub mod channel_type {
    pub const GUILD_TEXT: u8 = 0;
    pub const DM: u8 = 1;
    pub const GUILD_VOICE: u8 = 2;
    pub const GROUP_DM: u8 = 3;
    pub const GUILD_CATEGORY: u8 = 4;
    pub const GUILD_ANNOUNCEMENT: u8 = 5;
    pub const ANNOUNCEMENT_THREAD: u8 = 10;
    pub const PUBLIC_THREAD: u8 = 11;
    pub const PRIVATE_THREAD: u8 = 12;
    pub const GUILD_STAGE_VOICE: u8 = 13;
    pub const GUILD_DIRECTORY: u8 = 14;
    pub const GUILD_FORUM: u8 = 15;
    pub const GUILD_MEDIA: u8 = 16;
}

// ---------------------------------------------------------------------------
// Users, guilds, channels
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct User {
    pub id: Id,
    #[serde(deserialize_with = "null_default")]
    pub username: String,
    pub global_name: Option<String>,
    pub discriminator: Option<String>,
    pub avatar: Option<String>,
    pub banner: Option<String>,
    pub accent_color: Option<u32>,
    #[serde(deserialize_with = "null_default")]
    pub bot: bool,
    /// 0 none, 1 Nitro Classic, 2 Nitro, 3 Nitro Basic. Only sent for me (upload limit).
    #[serde(deserialize_with = "null_default")]
    pub premium_type: u8,
    /// 2FA on (only sent for me): needed for moderation in guilds with `mfa_level` 1.
    #[serde(deserialize_with = "null_default", skip_serializing_if = "std::ops::Not::not")]
    pub mfa_enabled: bool,
    /// Only sent for me (READY, USER_UPDATE): My Account's masked Email / Phone rows. Personal
    /// data: never written to the cache. Boxed: every cached user carries the slots.
    #[serde(skip_serializing)]
    #[allow(clippy::box_collection, reason = "8 bytes per cached user, not 24")]
    pub email: Option<Box<String>>,
    #[serde(skip_serializing)]
    #[allow(clippy::box_collection, reason = "8 bytes per cached user, not 24")]
    pub phone: Option<Box<String>>,
    /// Private flags (only sent for me): 1 << 4 = SMS 2FA on (My Account > SMS Backup).
    #[serde(deserialize_with = "null_default", skip_serializing_if = "is_zero")]
    pub flags: u64,
    // ----- cosmetics (docs/COSMETICS_NOTES.md §0.3). Boxed: most users have none, and every
    // cached user carries these slots.
    /// Badge fallback without a loaded profile (`ui::cosmetics::flag_badges`).
    #[serde(deserialize_with = "null_default", skip_serializing_if = "is_zero")]
    pub public_flags: u64,
    /// Guild ("clan") tag; `clan` is the old name of the same object, read when this is absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub primary_guild: Option<Box<PrimaryGuild>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clan: Option<Box<PrimaryGuild>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar_decoration_data: Option<Box<DecorationData>>,
    /// Nameplate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collectibles: Option<Box<Collectibles>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name_styles: Option<Box<NameStyle>>,
}

/// `primary_guild` / `clan` of a user (COSMETICS_NOTES §2).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PrimaryGuild {
    pub identity_guild_id: Option<Id>,
    pub identity_enabled: Option<bool>,
    /// ≤ 4 chars; null until the user re-affirms a changed tag.
    pub tag: Option<String>,
    /// Badge hash (`clan-badges/{guild}/{badge}.png`).
    pub badge: Option<String>,
}

/// `avatar_decoration_data` (§3).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DecorationData {
    /// Preset hash; `a_` = animated.
    #[serde(deserialize_with = "null_default")]
    pub asset: String,
    pub sku_id: Option<Id>,
    /// Unix seconds; past = hidden.
    #[serde(deserialize_with = "expiry")]
    pub expires_at: Option<i64>,
}

/// `collectibles` of a user or member (§4).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Collectibles {
    pub nameplate: Option<Nameplate>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Nameplate {
    /// `nameplates/<collection>/<name>/` (ends with `/`).
    #[serde(deserialize_with = "null_default")]
    pub asset: String,
    pub sku_id: Option<Id>,
    /// Gradient key (`crimson` … `white`, `none`).
    pub palette: Option<String>,
    pub label: Option<String>,
    /// Unix seconds.
    #[serde(deserialize_with = "expiry")]
    pub expires_at: Option<i64>,
}

/// `display_name_styles` (§7): colours 0xRRGGBB.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NameStyle {
    pub font_id: Option<u32>,
    pub effect_id: Option<u32>,
    #[serde(deserialize_with = "null_default")]
    pub colors: Vec<u32>,
}

/// An expiry as unix seconds from seconds, milliseconds or an ISO-8601 string (the profile
/// collectibles send it through `new Date()`); anything else is "never".
pub fn expiry<'de, D: Deserializer<'de>>(d: D) -> Result<Option<i64>, D::Error> {
    Ok(match serde_json::Value::deserialize(d)? {
        serde_json::Value::Number(n) => n.as_f64().map(|v| if v >= 1e11 { (v / 1000.0) as i64 } else { v as i64 }),
        serde_json::Value::String(s) => chrono::DateTime::parse_from_rfc3339(&s).ok().map(|t| t.timestamp()),
        _ => None,
    })
}

impl User {
    /// Name shown in the UI when no guild nickname applies.
    pub fn display_name(&self) -> &str {
        match &self.global_name {
            Some(n) if !n.is_empty() => n,
            _ => &self.username,
        }
    }
}

/// Partial guild member as attached to messages / READY merged_members / member lists.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Member {
    pub user: Option<User>,
    pub nick: Option<String>,
    pub avatar: Option<String>,
    #[serde(deserialize_with = "null_default")]
    pub roles: Vec<Id>,
    /// ISO-8601 end of a timeout; a past time means the timeout has expired.
    pub communication_disabled_until: Option<String>,
    /// Member flags; the automod-quarantine bits hide the guild tag (COSMETICS_NOTES §2).
    #[serde(deserialize_with = "null_default", skip_serializing_if = "is_zero")]
    pub flags: u64,
    /// Per-guild cosmetics: win over the user's (§0.2).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar_decoration_data: Option<Box<DecorationData>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collectibles: Option<Box<Collectibles>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name_styles: Option<Box<NameStyle>>,
}

impl Member {
    /// Timed out right now (only VIEW_CHANNEL and READ_MESSAGE_HISTORY remain).
    pub fn timed_out(&self) -> bool {
        // An unparsable time is ignored: the server still enforces the timeout.
        self.communication_disabled_until.as_deref().is_some_and(|t| {
            chrono::DateTime::parse_from_rfc3339(t).is_ok_and(|end| end > chrono::Utc::now())
        })
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Role {
    pub id: Id,
    #[serde(deserialize_with = "null_default")]
    pub name: String,
    /// 0 = no color.
    #[serde(deserialize_with = "null_default")]
    pub color: u32,
    #[serde(deserialize_with = "null_default")]
    pub position: i32,
    pub permissions: Perms,
    #[serde(deserialize_with = "null_default")]
    pub hoist: bool,
    /// Anyone may @mention it (otherwise only with MENTION_EVERYONE).
    #[serde(deserialize_with = "null_default")]
    pub mentionable: bool,
}

/// Custom (id = Some) or unicode (id = None, name = the emoji) emoji.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct Emoji {
    pub id: Option<Id>,
    pub name: Option<String>,
    #[serde(deserialize_with = "null_default")]
    pub animated: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Overwrite {
    pub id: Id,
    /// 0 = role, 1 = member.
    #[serde(rename = "type", deserialize_with = "overwrite_kind")]
    pub kind: u8,
    pub allow: Perms,
    pub deny: Perms,
}

fn overwrite_kind<'de, D: Deserializer<'de>>(d: D) -> Result<u8, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    Ok(match v {
        serde_json::Value::Number(n) => n.as_u64().unwrap_or(0) as u8,
        serde_json::Value::String(s) if s == "member" => 1,
        _ => 0,
    })
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Channel {
    pub id: Id,
    #[serde(rename = "type", deserialize_with = "null_default")]
    pub kind: u8,
    pub guild_id: Option<Id>,
    /// None for DMs (UI derives a name from recipients).
    pub name: Option<String>,
    #[serde(deserialize_with = "null_default")]
    pub position: i32,
    pub parent_id: Option<Id>,
    pub topic: Option<String>,
    pub last_message_id: Option<Id>,
    #[serde(deserialize_with = "null_default")]
    pub permission_overwrites: Vec<Overwrite>,
    #[serde(deserialize_with = "null_default")]
    pub nsfw: bool,
    /// Normalized DM recipients (ids into `AppState::users`).
    #[serde(deserialize_with = "null_default")]
    pub recipient_ids: Vec<Id>,
    /// Wire-only: full recipient objects (non-deduped payloads). State moves them into
    /// `AppState::users` and fills `recipient_ids`.
    #[serde(deserialize_with = "null_default", skip_serializing_if = "Vec::is_empty")]
    pub recipients: Vec<User>,
    /// Group DM icon hash.
    pub icon: Option<String>,
    pub owner_id: Option<Id>,
    /// Slowmode seconds (0 = off).
    #[serde(deserialize_with = "null_default")]
    pub rate_limit_per_user: u32,
    /// ISO-8601 time of the newest pin (CHANNEL_PINS_UPDATE keeps it current).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_pin_timestamp: Option<String>,
    // ----- threads (types 10-12; FEATURES_NOTES §2.1). `owner_id` = creator, `parent_id` =
    // the text/news/forum/media channel.
    /// Archive state; threads only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_metadata: Option<ThreadMetadata>,
    /// My thread membership: `Some` = joined (READY / THREAD_CREATE / thread lists; THREAD_UPDATE
    /// lacks it and the state keeps the old value).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub member: Option<ThreadMember>,
    /// Approximate (stops counting at 50).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub member_count: Option<u32>,
    /// Messages without the starter and deleted ones (forum card reply count).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_count: Option<u32>,
    /// Forum posts: ids from the parent's `available_tags` (≤ 5).
    #[serde(deserialize_with = "null_default", skip_serializing_if = "Vec::is_empty")]
    pub applied_tags: Vec<Id>,
    // ----- forum (15) / media (16) channels (§2.2)
    /// ≤ 20 tags posts can carry.
    #[serde(deserialize_with = "null_default", skip_serializing_if = "Vec::is_empty")]
    pub available_tags: Vec<ForumTag>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_reaction_emoji: Option<DefaultReaction>,
    /// None/0 latest activity, 1 creation time.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_sort_order: Option<u8>,
    /// 0 not set, 1 list, 2 gallery (media channels are always a gallery).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_forum_layout: Option<u8>,
    /// Minutes the UI preselects for new threads/posts (60, 1440, 4320, 10080).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_auto_archive_duration: Option<u32>,
    /// Channel flags: forum REQUIRE_TAG 1<<4, post PINNED 1<<1.
    #[serde(deserialize_with = "null_default", skip_serializing_if = "is_zero")]
    pub flags: u64,
    // ----- DM safety (§2.4)
    /// A DM waiting under Message Requests (not accepted yet).
    #[serde(deserialize_with = "null_default", skip_serializing_if = "std::ops::Not::not")]
    pub is_message_request: bool,
    /// A DM filed under Spam.
    #[serde(deserialize_with = "null_default", skip_serializing_if = "std::ops::Not::not")]
    pub is_spam: bool,
}

fn is_zero(v: &u64) -> bool {
    *v == 0
}

/// `thread_metadata` of a thread channel.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ThreadMetadata {
    /// Archived threads are read-only; sending unarchives (unless locked).
    #[serde(deserialize_with = "null_default")]
    pub archived: bool,
    /// Minutes: 60, 1440, 4320, 10080.
    #[serde(deserialize_with = "null_default")]
    pub auto_archive_duration: u32,
    /// ISO-8601; the archived lists page by it.
    pub archive_timestamp: Option<String>,
    /// Only MANAGE_THREADS may unarchive.
    #[serde(deserialize_with = "null_default")]
    pub locked: bool,
    /// Private threads: whether non-moderators may add people.
    pub invitable: Option<bool>,
    pub create_timestamp: Option<String>,
}

/// A thread member; on `Channel::member` always mine.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ThreadMember {
    /// Thread id (omitted inside READY/GUILD_CREATE `threads[]`).
    pub id: Option<Id>,
    pub user_id: Option<Id>,
    pub join_timestamp: Option<String>,
    /// 1 HAS_INTERACTED, 2 ALL_MESSAGES, 4 ONLY_MENTIONS, 8 NO_MESSAGES.
    #[serde(deserialize_with = "null_default")]
    pub flags: u32,
    #[serde(deserialize_with = "null_default")]
    pub muted: bool,
    pub mute_config: Option<MuteConfig>,
}

/// One of a forum channel's `available_tags`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ForumTag {
    pub id: Id,
    #[serde(deserialize_with = "null_default")]
    pub name: String,
    /// Only MANAGE_THREADS may apply it.
    #[serde(deserialize_with = "null_default")]
    pub moderated: bool,
    pub emoji_id: Option<Id>,
    pub emoji_name: Option<String>,
}

/// Forum `default_reaction_emoji`: custom (`emoji_id`) or unicode (`emoji_name`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DefaultReaction {
    pub emoji_id: Option<Id>,
    pub emoji_name: Option<String>,
}

impl Channel {
    pub fn is_dm_like(&self) -> bool {
        matches!(self.kind, channel_type::DM | channel_type::GROUP_DM)
    }

    /// Types 10, 11, 12.
    pub fn is_thread(&self) -> bool {
        matches!(self.kind, channel_type::ANNOUNCEMENT_THREAD | channel_type::PUBLIC_THREAD | channel_type::PRIVATE_THREAD)
    }

    /// An archived thread (read-only).
    pub fn is_archived(&self) -> bool {
        self.thread_metadata.as_ref().is_some_and(|m| m.archived)
    }

    /// Channels whose messages this client can open and read as a chat.
    pub fn is_text_chat(&self) -> bool {
        matches!(
            self.kind,
            channel_type::GUILD_TEXT
                | channel_type::DM
                | channel_type::GROUP_DM
                | channel_type::GUILD_ANNOUNCEMENT
                | channel_type::GUILD_VOICE
                | channel_type::GUILD_STAGE_VOICE
                | channel_type::ANNOUNCEMENT_THREAD
                | channel_type::PUBLIC_THREAD
                | channel_type::PRIVATE_THREAD
        )
    }
}

/// Normalized guild (gateway/ converts both READY guild shapes into this).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Guild {
    pub id: Id,
    #[serde(deserialize_with = "null_default")]
    pub name: String,
    pub icon: Option<String>,
    pub owner_id: Id,
    #[serde(deserialize_with = "null_default")]
    pub roles: Vec<Role>,
    #[serde(deserialize_with = "null_default")]
    pub emojis: Vec<CustomEmoji>,
    /// Present in READY/GUILD_CREATE and in the disk cache. The state keeps channels in
    /// `AppState::channels` and leaves this empty at runtime.
    #[serde(deserialize_with = "null_default")]
    pub channels: Vec<Channel>,
    /// 0 = all messages, 1 = only mentions.
    #[serde(deserialize_with = "null_default")]
    pub default_message_notifications: u8,
    #[serde(deserialize_with = "null_default")]
    pub unavailable: bool,
    /// Boost tier 0-3 (upload limit: 50 MiB at 2, 100 MiB at 3).
    #[serde(deserialize_with = "null_default")]
    pub premium_tier: u8,
    /// Only the features the UI reads (gateway/normalize.rs keeps VERIFIED / PARTNERED).
    #[serde(deserialize_with = "null_default", skip_serializing_if = "Vec::is_empty")]
    pub features: Vec<String>,
    /// Boosts (tooltip "Boost level 2, 9 boosts").
    #[serde(deserialize_with = "null_default")]
    pub premium_subscription_count: u32,
    /// 1 = moderation needs 2FA (`AppState::guild_permissions` drops the elevated bits).
    #[serde(deserialize_with = "null_default")]
    pub mfa_level: u8,
    /// The community's rules channel (its icon is `Icon::Rules`).
    pub rules_channel_id: Option<Id>,
    /// Who is in voice (GUILD_CREATE; READY_SUPPLEMENTAL arrives as `VoiceStateUpdate`s).
    /// Runtime-only like `channels`: the state moves them into `AppState::voice_states`, and
    /// they are never written to the disk cache.
    #[serde(deserialize_with = "null_default", skip_serializing)]
    pub voice_states: Vec<VoiceState>,
}

/// VOICE_STATE_UPDATE payload and guild `voice_states[]` entry (guild entries lack
/// `guild_id`/`member`; gateway/ fills `guild_id`).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct VoiceState {
    pub guild_id: Option<Id>,
    /// None = left voice.
    pub channel_id: Option<Id>,
    pub user_id: Id,
    pub member: Option<Member>,
    /// Main-gateway session of the voice connection.
    #[serde(deserialize_with = "null_default")]
    pub session_id: String,
    #[serde(deserialize_with = "null_default")]
    pub self_mute: bool,
    #[serde(deserialize_with = "null_default")]
    pub self_deaf: bool,
    /// Server mute / deafen by a moderator.
    #[serde(deserialize_with = "null_default")]
    pub mute: bool,
    #[serde(deserialize_with = "null_default")]
    pub deaf: bool,
    /// Camera on.
    #[serde(deserialize_with = "null_default")]
    pub self_video: bool,
    /// Go Live.
    #[serde(deserialize_with = "null_default")]
    pub self_stream: bool,
    /// Stage audience (may not speak).
    #[serde(deserialize_with = "null_default")]
    pub suppress: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CustomEmoji {
    pub id: Id,
    #[serde(deserialize_with = "null_default")]
    pub name: String,
    #[serde(deserialize_with = "null_default")]
    pub animated: bool,
    #[serde(deserialize_with = "null_default")]
    pub available: bool,
}

// ---------------------------------------------------------------------------
// Messages
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Attachment {
    pub id: Id,
    #[serde(deserialize_with = "null_default")]
    pub filename: String,
    #[serde(deserialize_with = "null_default")]
    pub url: String,
    #[serde(deserialize_with = "null_default")]
    pub proxy_url: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub content_type: Option<String>,
    #[serde(deserialize_with = "null_default")]
    pub size: u64,
    #[serde(deserialize_with = "null_default")]
    pub flags: u64,
}

impl Attachment {
    pub fn is_image(&self) -> bool {
        match &self.content_type {
            Some(ct) => ct.starts_with("image/"),
            None => {
                let f = self.filename.to_ascii_lowercase();
                [".png", ".jpg", ".jpeg", ".gif", ".webp"].iter().any(|e| f.ends_with(e))
            }
        }
    }
    pub fn is_spoiler(&self) -> bool {
        self.filename.starts_with("SPOILER_") || self.flags & (1 << 3) != 0
    }
    /// A video upload (media: plays inline when MP4/MOV, else the can't-play card).
    pub fn is_video(&self) -> bool {
        match &self.content_type {
            Some(ct) => ct.starts_with("video/"),
            None => {
                let f = self.filename.to_ascii_lowercase();
                [".mp4", ".mov", ".m4v", ".webm", ".mkv"].iter().any(|e| f.ends_with(e))
            }
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct EmbedMedia {
    #[serde(deserialize_with = "null_default")]
    pub url: String,
    pub proxy_url: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// `video/mp4` … on video media (media: the can't-play pre-check).
    pub content_type: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct EmbedAuthor {
    #[serde(deserialize_with = "null_default")]
    pub name: String,
    pub url: Option<String>,
    pub icon_url: Option<String>,
    pub proxy_icon_url: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct EmbedFooter {
    #[serde(deserialize_with = "null_default")]
    pub text: String,
    pub icon_url: Option<String>,
    pub proxy_icon_url: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct EmbedField {
    #[serde(deserialize_with = "null_default")]
    pub name: String,
    #[serde(deserialize_with = "null_default")]
    pub value: String,
    #[serde(deserialize_with = "null_default")]
    pub inline: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct EmbedProvider {
    pub name: Option<String>,
    pub url: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Embed {
    /// rich, image, video, gifv, link, article, ...
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub url: Option<String>,
    pub color: Option<u32>,
    pub timestamp: Option<String>,
    pub author: Option<EmbedAuthor>,
    pub footer: Option<EmbedFooter>,
    #[serde(deserialize_with = "null_default")]
    pub fields: Vec<EmbedField>,
    pub thumbnail: Option<EmbedMedia>,
    pub image: Option<EmbedMedia>,
    pub video: Option<EmbedMedia>,
    pub provider: Option<EmbedProvider>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ReactionCountDetails {
    #[serde(deserialize_with = "null_default")]
    pub burst: u32,
    #[serde(deserialize_with = "null_default")]
    pub normal: u32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Reaction {
    pub emoji: Emoji,
    #[serde(deserialize_with = "null_default")]
    pub count: u32,
    #[serde(deserialize_with = "null_default")]
    pub me: bool,
    #[serde(deserialize_with = "null_default")]
    pub me_burst: bool,
    pub count_details: ReactionCountDetails,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MessageReference {
    pub message_id: Option<Id>,
    pub channel_id: Option<Id>,
    pub guild_id: Option<Id>,
    /// 0 = default (reply), 1 = forward.
    #[serde(rename = "type", deserialize_with = "null_default")]
    pub kind: u8,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct StickerItem {
    pub id: Id,
    #[serde(deserialize_with = "null_default")]
    pub name: String,
    /// 1 png, 2 apng, 3 lottie, 4 gif.
    #[serde(deserialize_with = "null_default")]
    pub format_type: u8,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Message {
    pub id: Id,
    pub channel_id: Id,
    pub guild_id: Option<Id>,
    pub author: User,
    pub member: Option<Member>,
    #[serde(deserialize_with = "null_default")]
    pub content: String,
    /// ISO-8601.
    #[serde(deserialize_with = "null_default")]
    pub timestamp: String,
    pub edited_timestamp: Option<String>,
    #[serde(rename = "type", deserialize_with = "null_default")]
    pub kind: u8,
    #[serde(deserialize_with = "null_default")]
    pub flags: u64,
    #[serde(deserialize_with = "null_default")]
    pub mentions: Vec<User>,
    #[serde(deserialize_with = "null_default")]
    pub mention_roles: Vec<Id>,
    #[serde(deserialize_with = "null_default")]
    pub mention_everyone: bool,
    #[serde(deserialize_with = "null_default")]
    pub attachments: Vec<Attachment>,
    #[serde(deserialize_with = "null_default")]
    pub embeds: Vec<Embed>,
    #[serde(deserialize_with = "null_default")]
    pub reactions: Vec<Reaction>,
    /// Some(None) on the wire means "replied-to message was deleted"; we flatten to None.
    pub referenced_message: Option<Box<Message>>,
    pub message_reference: Option<MessageReference>,
    #[serde(deserialize_with = "null_default")]
    pub sticker_items: Vec<StickerItem>,
    /// String or integer on the wire.
    pub nonce: Option<serde_json::Value>,
    #[serde(deserialize_with = "null_default")]
    pub pinned: bool,
    /// A forward carries the forwarded message here (one snapshot is used).
    #[serde(deserialize_with = "null_default", skip_serializing_if = "Vec::is_empty")]
    pub message_snapshots: Vec<MessageSnapshot>,
    /// Boxed: a poll is rare, and every cached message carries the slot.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub poll: Option<Box<Poll>>,
}

/// A poll's question or answer text, with an optional emoji.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PollMedia {
    #[serde(deserialize_with = "null_default")]
    pub text: String,
    pub emoji: Option<Emoji>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PollAnswer {
    #[serde(deserialize_with = "null_default")]
    pub answer_id: u32,
    pub poll_media: PollMedia,
}

/// Votes for one answer; an answer without an entry has none.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PollAnswerCount {
    #[serde(deserialize_with = "null_default")]
    pub id: u32,
    #[serde(deserialize_with = "null_default")]
    pub count: u32,
    #[serde(deserialize_with = "null_default")]
    pub me_voted: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PollResults {
    #[serde(deserialize_with = "null_default")]
    pub is_finalized: bool,
    #[serde(deserialize_with = "null_default")]
    pub answer_counts: Vec<PollAnswerCount>,
}

/// `results` missing means unknown (not zero): keep the last ones.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Poll {
    pub question: PollMedia,
    #[serde(deserialize_with = "null_default")]
    pub answers: Vec<PollAnswer>,
    /// ISO-8601.
    pub expiry: Option<String>,
    #[serde(deserialize_with = "null_default")]
    pub allow_multiselect: bool,
    pub results: Option<PollResults>,
}

/// A forwarded message. Its fields are a subset of `Message`; the missing id/author/channel_id
/// stay at their defaults.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MessageSnapshot {
    pub message: Message,
}

impl Message {
    /// Its author may edit its text: not a forward or a poll.
    pub fn can_edit(&self) -> bool {
        matches!(self.kind, 0 | 19) && self.message_snapshots.is_empty() && self.poll.is_none()
    }

    /// Discord forwards plain messages only: no poll, components, ephemeral or forward of a
    /// forward (unconfirmed).
    pub fn forwardable(&self) -> bool {
        matches!(self.kind, 0 | 19 | 20 | 23) && self.poll.is_none() && self.message_snapshots.is_empty() && self.flags & (1 << 15 | 1 << 6) == 0
    }

    pub fn nonce_str(&self) -> Option<String> {
        match &self.nonce {
            Some(serde_json::Value::String(s)) => Some(s.clone()),
            Some(serde_json::Value::Number(n)) => Some(n.to_string()),
            _ => None,
        }
    }
}

/// MESSAGE_UPDATE is partial: only `id`/`channel_id` are guaranteed.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct MessageUpdate {
    pub id: Id,
    pub channel_id: Id,
    pub guild_id: Option<Id>,
    pub content: Option<String>,
    pub edited_timestamp: Option<String>,
    pub flags: Option<u64>,
    pub mentions: Option<Vec<User>>,
    pub mention_roles: Option<Vec<Id>>,
    pub mention_everyone: Option<bool>,
    pub attachments: Option<Vec<Attachment>>,
    pub embeds: Option<Vec<Embed>>,
    pub pinned: Option<bool>,
    pub poll: Option<Box<Poll>>,
}

// ---------------------------------------------------------------------------
// Read states & notification settings
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ReadState {
    /// Channel id.
    pub id: Id,
    pub last_message_id: Option<Id>,
    #[serde(deserialize_with = "null_default")]
    pub mention_count: u32,
    /// 0 = channel read state; others (guild events, notification center) are ignored.
    #[serde(deserialize_with = "null_default")]
    pub read_state_type: u8,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MuteConfig {
    pub end_time: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ChannelOverride {
    pub channel_id: Id,
    #[serde(deserialize_with = "null_default")]
    pub muted: bool,
    pub mute_config: Option<MuteConfig>,
    /// 0 all, 1 only mentions, 2 nothing, 3 inherit.
    #[serde(deserialize_with = "message_notifications_default")]
    pub message_notifications: u8,
}

/// One entry of `user_guild_settings`; `guild_id == None` is the DM/"@me" entry.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GuildSettings {
    pub guild_id: Option<Id>,
    #[serde(deserialize_with = "null_default")]
    pub muted: bool,
    pub mute_config: Option<MuteConfig>,
    /// 0 all, 1 only mentions, 2 nothing, 3 inherit guild default.
    #[serde(deserialize_with = "message_notifications_default")]
    pub message_notifications: u8,
    #[serde(deserialize_with = "null_default")]
    pub suppress_everyone: bool,
    #[serde(deserialize_with = "null_default")]
    pub suppress_roles: bool,
    #[serde(deserialize_with = "null_default")]
    pub channel_overrides: Vec<ChannelOverride>,
}

fn message_notifications_default<'de, D: Deserializer<'de>>(d: D) -> Result<u8, D::Error> {
    Ok(Option::<u8>::deserialize(d)?.unwrap_or(3))
}

// ---------------------------------------------------------------------------
// Lazy member list (normalized by gateway/)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
#[allow(clippy::large_enum_variant)] // nearly all items are members; boxing would add an allocation each
pub enum MemberListItem {
    /// Role id or "online"/"offline".
    Group { id: String, count: u32 },
    Member { member: Member, status: Presence },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Presence {
    Online,
    Idle,
    Dnd,
    #[default]
    Offline,
}

#[derive(Clone, Debug)]
pub enum MemberListOp {
    Sync { range: (u32, u32), items: Vec<MemberListItem> },
    Insert { index: u32, item: MemberListItem },
    Update { index: u32, item: MemberListItem },
    Delete { index: u32 },
    Invalidate { range: (u32, u32) },
}

#[derive(Clone, Debug)]
pub struct MemberListUpdate {
    pub guild_id: Id,
    /// "everyone" or a hash identifying the list (shared by channels with equal visibility).
    pub list_id: String,
    pub member_count: u32,
    pub online_count: u32,
    pub ops: Vec<MemberListOp>,
    /// Complete current `(group id, count)` list; replaces the counts of `Group` items.
    pub groups: Vec<(String, u32)>,
}

/// READY `relationships[]` entry, RELATIONSHIP_ADD/UPDATE/REMOVE payload.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Relationship {
    /// The other user's id.
    pub id: Id,
    /// 1 friend, 2 blocked, 3 incoming request, 4 outgoing request, 5 implicit.
    #[serde(rename = "type", deserialize_with = "null_default")]
    pub kind: u8,
    #[serde(deserialize_with = "null_default")]
    pub user_ignored: bool,
    /// Full user (RELATIONSHIP_ADD; READY sends `user_id` and the user in `users[]`). The state
    /// moves it into `AppState::users`, so stored relationships have None here.
    pub user: Option<User>,
    /// My nickname for this friend.
    pub nickname: Option<String>,
    /// ISO-8601 start of the relationship.
    pub since: Option<String>,
    /// Incoming request filed as spam (Pending tab: separate Spam section).
    #[serde(deserialize_with = "null_default")]
    pub is_spam_request: bool,
}

pub mod relationship_type {
    pub const FRIEND: u8 = 1;
    pub const BLOCKED: u8 = 2;
    pub const INCOMING: u8 = 3;
    pub const OUTGOING: u8 = 4;
}

// ---------------------------------------------------------------------------
// Threads, forums, profiles (FEATURES_NOTES Part 2)
// ---------------------------------------------------------------------------

/// One page of a thread list (thread search or an archived list).
#[derive(Clone, Debug, Default)]
pub struct ThreadPage {
    /// In the order the API sorted them; `member` is filled (from the answer's `members[]`)
    /// for threads I joined.
    pub threads: Vec<Channel>,
    pub has_more: bool,
    /// Thread search only (approximate).
    pub total_results: Option<u32>,
    /// Forum search: first message of posts (`message.id == thread.id`); some may be missing.
    pub first_messages: Vec<Message>,
}

/// `GET /users/{id}/profile` (+ my note). Display rules: `bio()` / `pronouns()` /
/// `theme_colors()` / `banner()` prefer the guild profile's non-empty fields.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct UserProfile {
    /// With `banner` / `accent_color`.
    pub user: User,
    #[serde(deserialize_with = "null_default")]
    pub user_profile: ProfileFields,
    /// Asked with `guild_id`: this guild's profile.
    pub guild_member_profile: Option<ProfileFields>,
    /// Their member object in that guild (nick, roles).
    pub guild_member: Option<Member>,
    #[serde(deserialize_with = "null_default")]
    pub badges: Vec<ProfileBadge>,
    #[serde(deserialize_with = "null_default")]
    pub guild_badges: Vec<ProfileBadge>,
    #[serde(deserialize_with = "null_default")]
    pub mutual_guilds: Vec<MutualGuild>,
    #[serde(deserialize_with = "null_default")]
    pub mutual_friends: Vec<User>,
    pub mutual_friends_count: Option<u32>,
    #[serde(deserialize_with = "null_default")]
    pub connected_accounts: Vec<ConnectedAccount>,
    /// ISO-8601.
    pub premium_since: Option<String>,
    pub premium_guild_since: Option<String>,
    /// My note about them (`GET /users/@me/notes/{id}`, filled by the backend; None = no note).
    #[serde(skip)]
    pub note: Option<String>,
    /// Profile widgets: the game collection (COSMETICS_NOTES §11); their games come from
    /// `SocialCommand::FetchGames`.
    #[serde(deserialize_with = "null_default")]
    pub widgets: Vec<ProfileWidget>,
}

/// One `widgets[]` entry (§11).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct ProfileWidget {
    pub data: WidgetData,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct WidgetData {
    /// `favorite_games`, `current_games`, `played_games`, `want_to_play_games`, …
    #[serde(rename = "type", deserialize_with = "null_default")]
    pub kind: String,
    #[serde(deserialize_with = "null_default")]
    pub games: Vec<WidgetGame>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct WidgetGame {
    pub game_id: Id,
    pub comment: Option<String>,
    #[serde(deserialize_with = "null_default")]
    pub tags: Vec<String>,
}

/// `GET /games` entry.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Game {
    pub id: Id,
    #[serde(deserialize_with = "null_default")]
    pub name: String,
    pub icon_hash: Option<String>,
    pub supplemental_game_data: Option<GameExtra>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct GameExtra {
    /// Portrait 3:4 cover (shown only from a Discord media host).
    pub cover_image_url: Option<String>,
}

/// `user_profile.collectibles[]` (§5): `kind` 1 effect, 3 frame.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct ProfileCollectible {
    pub sku_id: Id,
    #[serde(rename = "type", deserialize_with = "null_default")]
    pub kind: u16,
    #[serde(deserialize_with = "expiry")]
    pub expires_at: Option<i64>,
}

/// Legacy `user_profile.profile_effect`.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct ProfileEffectRef {
    pub id: Id,
    #[serde(deserialize_with = "expiry")]
    pub expires_at: Option<i64>,
}

/// `GET /collectibles-products/{sku}` (§12): what the client keeps of a product.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CollectibleProduct {
    pub sku_id: Id,
    #[serde(deserialize_with = "null_default")]
    pub name: String,
    /// 0 decoration, 1 effect, 2 nameplate, 3 frame, 1000 bundle, 2000 variants, 3000 external.
    #[serde(rename = "type", deserialize_with = "null_default")]
    pub kind: u16,
    #[serde(deserialize_with = "null_default")]
    pub items: Vec<CollectibleItem>,
}

/// One product item; the fields of every kind side by side (§3-§6).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CollectibleItem {
    /// 0 decoration, 1 effect, 2 nameplate, 3 frame.
    #[serde(rename = "type", deserialize_with = "null_default")]
    pub kind: u16,
    pub sku_id: Id,
    /// Decoration / nameplate asset.
    pub asset: Option<String>,
    pub label: Option<String>,
    pub palette: Option<String>,
    // ----- effect (1)
    pub title: Option<String>,
    #[serde(rename = "reducedMotionSrc")]
    pub reduced_motion_src: Option<String>,
    #[serde(rename = "staticFrameSrc")]
    pub static_frame_src: Option<String>,
    #[serde(rename = "thumbnailPreviewSrc")]
    pub thumbnail_preview_src: Option<String>,
    #[serde(deserialize_with = "null_default")]
    pub effects: Vec<EffectLayer>,
    // ----- frame (3)
    pub inner_width: Option<f32>,
    pub overflow_top: Option<f32>,
    pub overflow_bottom: Option<f32>,
    pub overflow_horizontal: Option<f32>,
    #[serde(deserialize_with = "null_default")]
    pub layers: Vec<FrameLayer>,
}

/// A profile-effect layer (APNG; times in ms, geometry for a 450 px wide card).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct EffectLayer {
    #[serde(deserialize_with = "null_default")]
    pub src: String,
    #[serde(rename = "loop", deserialize_with = "null_default")]
    pub looping: bool,
    pub width: Option<f32>,
    pub height: Option<f32>,
    #[serde(deserialize_with = "null_default")]
    pub duration: f32,
    #[serde(deserialize_with = "null_default")]
    pub start: f32,
    #[serde(rename = "loopDelay", deserialize_with = "null_default")]
    pub loop_delay: f32,
    #[serde(deserialize_with = "null_default")]
    pub position: EffectPos,
    #[serde(rename = "zIndex", deserialize_with = "null_default")]
    pub z_index: i32,
    #[serde(rename = "randomizedSources", deserialize_with = "null_default")]
    pub randomized_sources: Vec<EffectSrc>,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct EffectPos {
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct EffectSrc {
    #[serde(deserialize_with = "null_default")]
    pub src: String,
}

/// A profile-frame layer (§6). `kind` / `order` / `anchor` arrive as strings or ints; stored as
/// the int (declaration order: staple 0 / rail 1 / border 2, front 0 / back 1, top 0 / bottom 1 /
/// center 2).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct FrameLayer {
    #[serde(deserialize_with = "null_default")]
    pub id: String,
    #[serde(rename = "type", deserialize_with = "frame_enum")]
    pub kind: u8,
    #[serde(deserialize_with = "frame_enum")]
    pub order: u8,
    #[serde(deserialize_with = "frame_enum")]
    pub anchor: u8,
    #[serde(deserialize_with = "null_default")]
    pub responsive: bool,
}

fn frame_enum<'de, D: Deserializer<'de>>(d: D) -> Result<u8, D::Error> {
    Ok(match serde_json::Value::deserialize(d)? {
        serde_json::Value::Number(n) => n.as_u64().unwrap_or(0) as u8,
        serde_json::Value::String(s) => match s.to_ascii_lowercase().as_str() {
            "rail" | "back" | "bottom" => 1,
            "border" | "center" => 2,
            _ => 0,
        },
        _ => 0,
    })
}

/// `user_profile` / `guild_member_profile`.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct ProfileFields {
    /// ≤ 190 chars, markdown.
    pub bio: Option<String>,
    /// ≤ 40 chars.
    pub pronouns: Option<String>,
    /// Banner hash (user: `banners/{user}/{hash}`, guild: `guilds/{g}/users/{user}/banners/{hash}`).
    pub banner: Option<String>,
    pub accent_color: Option<u32>,
    /// `[primary, accent]` as 0xRRGGBB.
    pub theme_colors: Option<Vec<u32>>,
    /// Legacy profile effect; `collectibles` is the newer shape (COSMETICS_NOTES §5).
    pub profile_effect: Option<ProfileEffectRef>,
    #[serde(deserialize_with = "null_default")]
    pub collectibles: Vec<ProfileCollectible>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct ProfileBadge {
    #[serde(deserialize_with = "null_default")]
    pub id: String,
    #[serde(deserialize_with = "null_default")]
    pub description: String,
    /// `https://cdn.discordapp.com/badge-icons/{icon}.png`.
    #[serde(deserialize_with = "null_default")]
    pub icon: String,
    pub link: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct MutualGuild {
    pub id: Id,
    pub nick: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct ConnectedAccount {
    /// `github`, `steam`, `spotify`, …
    #[serde(rename = "type", deserialize_with = "null_default")]
    pub kind: String,
    #[serde(deserialize_with = "null_default")]
    pub id: String,
    #[serde(deserialize_with = "null_default")]
    pub name: String,
    #[serde(deserialize_with = "null_default")]
    pub verified: bool,
    // ----- my own list (`GET /users/@me/connections`); absent on profiles.
    /// 1 = shown on my profile.
    pub visibility: Option<u8>,
    pub show_activity: Option<bool>,
}

impl UserProfile {
    /// SKU of the shown profile effect (`kind` 1) or frame (3): the guild profile's first, unexpired.
    pub fn collectible_sku(&self, kind: u16, now_s: i64) -> Option<Id> {
        let live = |e: Option<i64>| e.is_none_or(|t| t > now_s);
        let pick = |p: &ProfileFields| {
            let new = p.collectibles.iter().find(|c| c.kind == kind && live(c.expires_at) && !c.sku_id.is_zero()).map(|c| c.sku_id);
            new.or_else(|| p.profile_effect.as_ref().filter(|e| kind == 1 && live(e.expires_at) && !e.id.is_zero()).map(|e| e.id))
        };
        self.guild_member_profile.as_ref().and_then(pick).or_else(|| pick(&self.user_profile))
    }
    fn field<'a, T: ?Sized>(&'a self, get: impl Fn(&'a ProfileFields) -> Option<&'a T>, empty: impl Fn(&T) -> bool) -> Option<&'a T> {
        let guild = self.guild_member_profile.as_ref().and_then(&get).filter(|v| !empty(v));
        guild.or_else(|| get(&self.user_profile).filter(|v| !empty(v)))
    }

    pub fn bio(&self) -> Option<&str> {
        self.field(|p| p.bio.as_deref(), str::is_empty)
    }

    pub fn pronouns(&self) -> Option<&str> {
        self.field(|p| p.pronouns.as_deref(), str::is_empty)
    }

    /// `(primary, accent)`.
    pub fn theme_colors(&self) -> Option<(u32, u32)> {
        match self.field(|p| p.theme_colors.as_deref(), |c: &[u32]| c.len() < 2)? {
            [a, b, ..] => Some((*a, *b)),
            _ => None,
        }
    }

    /// Banner hash and whether it is the guild one: guild banner, else user banner. None = use
    /// `user.accent_color` (then the avatar's colour).
    pub fn banner(&self) -> Option<(&str, bool)> {
        let guild = self.guild_member_profile.as_ref().and_then(|p| p.banner.as_deref()).filter(|b| !b.is_empty());
        if let Some(b) = guild {
            return Some((b, true));
        }
        let user = self.user_profile.banner.as_deref().or(self.user.banner.as_deref());
        user.filter(|b| !b.is_empty()).map(|b| (b, false))
    }
}

impl Relationship {
    /// "Blocked or ignored": the user's messages neither notify nor count as unread.
    pub fn hides_author(&self) -> bool {
        self.kind == 2 || self.user_ignored
    }
}

// ---------------------------------------------------------------------------
// Chat essentials: presence, pins, search, GIFs (FEATURES_NOTES Part 1)
// ---------------------------------------------------------------------------

/// A status I can choose (op 3). Others see `Invisible` as offline.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UserStatus {
    #[default]
    Online,
    Idle,
    Dnd,
    Invisible,
}

impl UserStatus {
    /// Wire value for op 3 and the user settings.
    pub fn as_str(self) -> &'static str {
        match self {
            UserStatus::Online => "online",
            UserStatus::Idle => "idle",
            UserStatus::Dnd => "dnd",
            UserStatus::Invisible => "invisible",
        }
    }

    /// Wire value -> status; `offline`/`unknown`/anything else -> None.
    pub fn parse(s: &str) -> Option<Self> {
        [UserStatus::Online, UserStatus::Idle, UserStatus::Dnd, UserStatus::Invisible].into_iter().find(|v| v.as_str() == s)
    }

    /// Status-dot class for my own status (others see invisible as offline).
    pub fn presence(self) -> Presence {
        match self {
            UserStatus::Online => Presence::Online,
            UserStatus::Idle => Presence::Idle,
            UserStatus::Dnd => Presence::Dnd,
            UserStatus::Invisible => Presence::Offline,
        }
    }
}

/// Someone's presence as the UI shows it. Users without an entry in
/// `AppState::chat.presences` are offline.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UserPresence {
    pub status: Presence,
    /// Online only on a phone (`client_status` has `mobile` and nothing else): phone glyph.
    pub mobile: bool,
    /// `client_status` platforms as bits (`plugins::people::platform`: desktop, mobile, web,
    /// embedded, vr); 0 = unknown. Read by the PlatformIndicators plugin.
    pub platforms: u8,
    /// Activity type 4 ("Custom Status").
    pub custom_status: Option<CustomStatus>,
    /// First activity that is not a custom status (types 0-3, 5).
    pub activity: Option<Activity>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CustomStatus {
    /// At most 128 chars.
    pub text: Option<String>,
    pub emoji: Option<Emoji>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Activity {
    /// 0 playing, 1 streaming, 2 listening, 3 watching, 5 competing.
    pub kind: u8,
    pub name: String,
    /// Boxed: every presence carries an `Option<Activity>`, most without rich presence.
    pub rich: Option<Box<RichPresence>>,
}

/// The optional parts of an activity (rich presence).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RichPresence {
    pub details: Option<String>,
    pub state: Option<String>,
    /// `timestamps.start`, Unix ms.
    pub start: Option<u64>,
    pub application_id: Option<Id>,
    /// `assets.large_image` / `small_image`: an app-asset id or `mp:<media proxy path>`.
    pub large_image: Option<String>,
    /// Hover text of the large image (Spotify: the album).
    pub large_text: Option<String>,
    pub small_image: Option<String>,
    pub small_text: Option<String>,
    /// `timestamps.end`, Unix ms (Spotify: when the song ends).
    pub end: Option<u64>,
    /// Spotify track id (`sync_id`).
    pub sync_id: Option<String>,
    /// Stream URL (type 1).
    pub url: Option<String>,
    /// `party.size`: [current, max].
    pub party: Option<[u32; 2]>,
}

impl Activity {
    /// "Playing", "Listening to", … for activity type `kind`.
    pub fn verb(kind: u8) -> &'static str {
        match kind {
            1 => "Live on",
            2 => "Listening to",
            3 => "Watching",
            5 => "Competing in",
            _ => "Playing",
        }
    }

    /// The member-list line: "Playing X", "Listening to X", …
    pub fn label(&self) -> String {
        format!("{} {}", Self::verb(self.kind), self.name)
    }

    /// "for 12 min" / "for 2 h 5 min" / "for 3 d" since `timestamps.start`; None without one
    /// (or one in the future).
    pub fn elapsed(&self, now_ms: u64) -> Option<String> {
        let mins = now_ms.checked_sub(self.rich.as_ref()?.start?)? / 60_000;
        Some(match (mins / 1440, mins / 60 % 24, mins % 60) {
            (0, 0, m) => format!("for {m} min"),
            (0, h, 0) => format!("for {h} h"),
            (0, h, m) => format!("for {h} h {m} min"),
            (d, _, _) => format!("for {d} d"),
        })
    }

    /// Card heading: "Playing", "Listening to Spotify", "Live on Twitch", "Watching", …
    pub fn heading(&self) -> String {
        match self.kind {
            1 | 2 => self.label(),
            k => Self::verb(k).to_owned(),
        }
    }

    /// A Spotify song (album art, progress, "Play on Spotify").
    pub fn is_spotify(&self) -> bool {
        self.kind == 2 && (self.name == "Spotify" || self.rich.as_ref().and_then(|r| r.large_image.as_deref()).is_some_and(|i| i.starts_with("spotify:")))
    }

    /// `open.spotify.com` link of the song; its id comes from other users, so it must be a plain
    /// base62 track id.
    pub fn spotify_track_url(&self) -> Option<String> {
        let id = self.rich.as_ref()?.sync_id.as_deref().filter(|_| self.is_spotify())?;
        (id.len() == 22 && id.bytes().all(|b| b.is_ascii_alphanumeric())).then(|| format!("https://open.spotify.com/track/{id}"))
    }

    /// The stream's page (type 1) when it is an https Twitch or YouTube link: it comes from other
    /// users and opens in the browser on a click.
    pub fn stream_url(&self) -> Option<&str> {
        let url = self.rich.as_ref()?.url.as_deref().filter(|_| self.kind == 1)?;
        let u = reqwest::Url::parse(url).ok()?;
        let host = u.host_str()?;
        let known = ["twitch.tv", "www.twitch.tv", "m.twitch.tv", "youtube.com", "www.youtube.com", "m.youtube.com"].contains(&host);
        (u.scheme() == "https" && u.port().is_none() && u.username().is_empty() && known).then_some(url)
    }

    /// (played, length) in ms of a song with start and end times.
    pub fn progress(&self, now_ms: u64) -> Option<(u64, u64)> {
        let r = self.rich.as_ref()?;
        let total = r.end?.checked_sub(r.start?).filter(|&t| t > 0)?;
        Some((now_ms.saturating_sub(r.start?).min(total), total))
    }
}

/// "4:07" / "1:02:33": a song position or a running timer.
pub fn clock(ms: u64) -> String {
    let s = ms / 1000;
    match s / 3600 {
        0 => format!("{}:{:02}", s / 60, s % 60),
        h => format!("{h}:{:02}:{:02}", s / 60 % 60, s % 60),
    }
}

/// One entry of `GET /channels/{c}/messages/pins` (pinned messages carry no reactions).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct PinnedMessage {
    /// ISO-8601; the last item's value is the next page's `before`.
    #[serde(deserialize_with = "null_default")]
    pub pinned_at: String,
    pub message: Message,
}

/// `GET /channels/{c}/messages/pins` response.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct PinsPage {
    #[serde(deserialize_with = "null_default")]
    pub items: Vec<PinnedMessage>,
    #[serde(deserialize_with = "null_default")]
    pub has_more: bool,
}

/// One page of message search results (`GET …/messages/search`, 25 hits).
#[derive(Clone, Debug, Default)]
pub struct SearchPage {
    /// Approximate.
    pub total_results: u32,
    /// The hits (the wire's inner arrays flattened), in the requested order. No `reactions`,
    /// no `member`, no `guild_id`.
    pub messages: Vec<Message>,
    /// Threads the hits are in (guild search), so the UI can name them.
    pub threads: Vec<Channel>,
    /// Old history is still being indexed: results may be incomplete.
    #[allow(dead_code, reason = "the search panel does not show this notice yet")]
    pub doing_deep_historical_index: bool,
}

/// A GIF from Discord's `/gifs/*` endpoints (`media_format=gif`; provider Klipy). Serialized
/// for the local GIF favourites (settings).
#[derive(Clone, Debug, Default, Deserialize, serde::Serialize)]
#[serde(default)]
pub struct GifItem {
    /// Set on trending's category tiles (`{name, src}`): the search term; `src` is its GIF.
    #[serde(skip)]
    pub category: Option<String>,
    /// Page URL: sent as the message content; the server unfurls it into a `gifv` embed.
    #[serde(deserialize_with = "null_default")]
    pub url: String,
    /// The animated GIF shown in the picker.
    #[serde(deserialize_with = "null_default")]
    pub src: String,
    /// Still preview image.
    #[serde(deserialize_with = "null_default")]
    pub preview: String,
    #[serde(deserialize_with = "null_default")]
    pub width: u32,
    #[serde(deserialize_with = "null_default")]
    pub height: u32,
}

// ---------------------------------------------------------------------------
// Account & server (FEATURES_NOTES Part 3; docs/ACCOUNT_SERVER_DESIGN.md)
// ---------------------------------------------------------------------------

/// One server-bar entry of the settings proto (`guild_folders`, field 14.1): a folder (`id`
/// Some) or a single positioned guild (`id` None, one guild id).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GuildFolder {
    /// Proto `Int64Value` (14.1.2); new folders get a random unused u32.
    pub id: Option<i64>,
    /// 14.1.3; None = the UI joins the guild names with ", ".
    pub name: Option<String>,
    /// 0xRRGGBB (14.1.4, `UInt64Value`).
    pub color: Option<u32>,
    /// 14.1.1 (`repeated fixed64`). May name guilds I left: skip unknown ids.
    pub guild_ids: Vec<Id>,
}

/// Settings-proto field 11 (`status`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StatusSettings {
    /// 11.1; None = not set.
    pub status: Option<UserStatus>,
    /// 11.2: text (11.2.1) and emoji (id 11.2.2 / name 11.2.3).
    pub custom_status: Option<CustomStatus>,
    /// 11.2.4 `expires_at_ms` (unix ms); None = never.
    pub custom_status_expires_at_ms: Option<u64>,
}

/// The part of `PreloadedUserSettings` the client reads (READY `user_settings_proto`,
/// USER_SETTINGS_PROTO_UPDATE type 1). `None` = that top-level field was absent.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UserSettingsProto {
    pub status: Option<StatusSettings>,
    pub guild_folders: Option<Vec<GuildFolder>>,
    /// Every top-level field's bytes (merged), for the Settings pages' other options
    /// (`api::settings_proto::read_varint`, docs/SETTINGS_NOTES.md).
    pub raw: std::collections::BTreeMap<u32, Vec<u8>>,
}

/// `GET /auth/sessions` entry (Settings → Devices).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct AuthSession {
    #[serde(deserialize_with = "null_default")]
    pub id_hash: String,
    /// ISO 8601.
    pub approx_last_used_time: Option<String>,
    #[serde(deserialize_with = "null_default")]
    pub client_info: SessionClientInfo,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct SessionClientInfo {
    pub os: Option<String>,
    pub platform: Option<String>,
    pub location: Option<String>,
}

/// `GET /oauth2/tokens` entry (Settings → Authorized Apps).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct AuthorizedApp {
    /// The token id (`DELETE /oauth2/tokens/{id}`).
    pub id: Id,
    #[serde(deserialize_with = "null_default")]
    pub scopes: Vec<String>,
    #[serde(deserialize_with = "null_default")]
    pub application: OAuthApplication,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct OAuthApplication {
    pub id: Id,
    #[serde(deserialize_with = "null_default")]
    pub name: String,
    pub icon: Option<String>,
    #[serde(deserialize_with = "null_default")]
    pub description: String,
}

/// `GET /users/@me/consent` (Settings → Data & Privacy).
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Consents {
    #[serde(deserialize_with = "null_default")]
    pub usage_statistics: Consent,
    #[serde(deserialize_with = "null_default")]
    pub personalization: Consent,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Consent {
    pub consented: bool,
}

/// `GET /guilds/{g}/profile`: what the server tag card shows (docs/GUILD_CARD_NOTES.md).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct GuildProfile {
    pub id: Id,
    #[serde(deserialize_with = "null_default")]
    pub name: String,
    pub icon_hash: Option<String>,
    #[serde(deserialize_with = "null_default")]
    pub member_count: u32,
    #[serde(deserialize_with = "null_default")]
    pub online_count: u32,
    pub description: Option<String>,
    /// "#rrggbb".
    pub brand_color_primary: Option<String>,
    pub tag: Option<String>,
    /// `clan-badges/{g}/{hash}.png`.
    pub badge_hash: Option<String>,
    #[serde(deserialize_with = "null_default")]
    pub traits: Vec<GuildTrait>,
    /// Community features only: COMMUNITY, DISCOVERABLE, PARTNERED, VERIFIED,
    /// MEMBER_VERIFICATION_GATE_ENABLED, MEMBER_VERIFICATION_MANUAL_APPROVAL.
    #[serde(deserialize_with = "null_default")]
    pub features: Vec<String>,
    /// 1 public, 2 invite only, 3 public, anyone may apply.
    #[serde(deserialize_with = "null_default")]
    pub visibility: u8,
    /// `discovery-splashes/{g}/{hash}.png`.
    pub custom_banner_hash: Option<String>,
    #[serde(deserialize_with = "null_default")]
    pub premium_subscription_count: u32,
    #[serde(deserialize_with = "null_default")]
    pub premium_tier: u8,
}

/// A guild profile's trait chip ("Rust" with its emoji): a unicode emoji (`emoji_name` is the
/// emoji, or its shortcode name like "speaking_head") or a custom one (`emoji_id`, `emoji_name`
/// its name).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct GuildTrait {
    pub emoji_id: Option<Id>,
    #[serde(deserialize_with = "null_default")]
    pub emoji_animated: bool,
    pub emoji_name: Option<String>,
    #[serde(deserialize_with = "null_default")]
    pub label: String,
    #[serde(deserialize_with = "null_default")]
    pub position: u32,
}

/// `GET /guilds/{g}/member-verification`: the rules / application form.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct MemberVerification {
    pub version: Option<String>,
    pub description: Option<String>,
    /// Kept raw: `PUT /guilds/{g}/requests/@me` sends every field object back unchanged, plus
    /// its `response`.
    #[serde(deserialize_with = "null_default")]
    pub form_fields: Vec<serde_json::Value>,
}

/// `GET /invites/{code}`, `GET /guilds/{g}/invites` entries, `POST /channels/{c}/invites`.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Invite {
    #[serde(deserialize_with = "null_default")]
    pub code: String,
    /// 0 guild, 1 group DM, 2 friend.
    #[serde(rename = "type", deserialize_with = "null_default")]
    pub kind: u8,
    /// Partial guild (id, name, icon, …).
    pub guild: Option<Guild>,
    /// Partial channel (id, name, type).
    pub channel: Option<Channel>,
    pub inviter: Option<User>,
    /// `with_counts=true` only.
    pub approximate_member_count: Option<u32>,
    pub approximate_presence_count: Option<u32>,
    /// ISO-8601; None = never.
    pub expires_at: Option<String>,
    // Invite metadata (create answer, guild invite list).
    pub uses: Option<u32>,
    /// 0 = unlimited.
    pub max_uses: Option<u32>,
    /// Seconds; 0 = never expires.
    pub max_age: Option<u32>,
    pub temporary: Option<bool>,
    pub created_at: Option<String>,
}

/// `GET /guilds/{g}/bans` entry.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Ban {
    pub reason: Option<String>,
    pub user: User,
}

// ---------------------------------------------------------------------------
// Normalized gateway events (gateway/ -> state)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub struct ReadyData {
    pub user: User,
    pub guilds: Vec<Guild>,
    pub private_channels: Vec<Channel>,
    /// Users referenced by DMs etc.
    pub users: Vec<User>,
    /// My member object per guild (roles, nick, guild avatar).
    pub my_members: Vec<(Id, Member)>,
    pub read_states: Vec<ReadState>,
    pub guild_settings: Vec<GuildSettings>,
    pub relationships: Vec<Relationship>,
    /// `auth_session_id_hash`: this login's entry in `GET /auth/sessions` (Settings → Devices).
    pub auth_session_id_hash: String,
}

#[derive(Clone, Debug)]
pub enum GatewayEvent {
    Ready(Box<ReadyData>),
    Resumed,
    MessageCreate(Box<Message>),
    MessageUpdate(Box<MessageUpdate>),
    MessageDelete { id: Id, channel_id: Id },
    MessageDeleteBulk { ids: Vec<Id>, channel_id: Id },
    ReactionAdd { channel_id: Id, message_id: Id, user_id: Id, emoji: Emoji, burst: bool },
    ReactionRemove { channel_id: Id, message_id: Id, user_id: Id, emoji: Emoji, burst: bool },
    ReactionRemoveAll { channel_id: Id, message_id: Id },
    ReactionRemoveEmoji { channel_id: Id, message_id: Id, emoji: Emoji },
    /// MESSAGE_POLL_VOTE_ADD / _REMOVE, one per answer.
    PollVote { channel_id: Id, message_id: Id, user_id: Id, answer_id: u32, add: bool },
    /// Read on another device (or echo of our own ack).
    MessageAck { channel_id: Id, message_id: Id, mention_count: Option<u32> },
    ChannelCreate(Box<Channel>),
    ChannelUpdate(Box<Channel>),
    ChannelDelete {
        id: Id,
        #[allow(dead_code)] // channel ids are global; kept for completeness
        guild_id: Option<Id>,
    },
    GuildCreate(Box<Guild>),
    /// Guild metadata changed (name/icon/roles/owner); `channels` is empty.
    GuildUpdate(Box<Guild>),
    GuildDelete { id: Id, unavailable: bool },
    GuildRoleUpsert { guild_id: Id, role: Role },
    GuildRoleDelete { guild_id: Id, role_id: Id },
    GuildEmojisUpdate { guild_id: Id, emojis: Vec<CustomEmoji> },
    /// Only forwarded for the current user.
    MyMemberUpdate { guild_id: Id, member: Member },
    UserUpdate(User),
    GuildSettingsUpdate(GuildSettings),
    MemberListUpdate(Box<MemberListUpdate>),
    /// CHANNEL_UNREAD_UPDATE (unsubscribed large guilds): `(channel id, last_message_id)`.
    /// Only raises `Channel::last_message_id`; the rest of the channel is untouched.
    ChannelLastMessage { updates: Vec<(Id, Option<Id>)> },
    /// RELATIONSHIP_ADD / RELATIONSHIP_UPDATE.
    RelationshipUpsert(Relationship),
    /// RELATIONSHIP_REMOVE (`id` = the other user).
    RelationshipRemove { id: Id },
    /// VOICE_STATE_UPDATE of anyone (also one per READY_SUPPLEMENTAL guild voice state).
    VoiceStateUpdate(Box<VoiceState>),
    /// VOICE_SERVER_UPDATE. The gateway routes it to the voice module only, never to the UI.
    VoiceServerUpdate {
        guild_id: Option<Id>,
        /// Set for DM/group calls.
        channel_id: Option<Id>,
        /// `host:port` without a scheme; None = the server is being reallocated.
        endpoint: Option<String>,
        /// Voice token: secret, redacted in Debug.
        token: crate::auth::Secret,
    },
    // ----- Go Live (VOICE_NOTES §5.4). The gateway routes these to the voice module only.
    /// STREAM_CREATE: my stream or one I watch got its RTC server (Identify `server_id` /
    /// `channel_id` of the stream connection; DAVE group `rtc_server_id - 1`).
    #[allow(dead_code, reason = "viewer_ids and paused are for STREAM_UPDATE / viewer UI, not handled yet")]
    StreamCreate { stream_key: String, rtc_server_id: Id, rtc_channel_id: Option<Id>, viewer_ids: Vec<Id>, paused: bool },
    /// STREAM_SERVER_UPDATE: the stream connection's voice server.
    StreamServerUpdate {
        stream_key: String,
        /// `host:port` without a scheme; None = the server is being reallocated.
        endpoint: Option<String>,
        /// Stream token: secret, redacted in Debug.
        token: crate::auth::Secret,
    },
    /// STREAM_DELETE: the stream ended or watching was refused. `reason`: user_requested,
    /// stream_ended, stream_full, unauthorized, safety_guild_rate_limited, …
    #[allow(dead_code, reason = "`unavailable` is not shown yet")]
    StreamDelete { stream_key: String, reason: Option<String>, unavailable: bool },
    // ----- DM / group DM calls (VOICE_NOTES §1.4). CALL_CREATE's `voice_states` arrive as
    // `VoiceStateUpdate`s (guild_id None) right before its `CallUpdate`.
    /// CALL_CREATE / CALL_UPDATE: a call is going on in this DM; `ringing` = users being rung.
    CallUpdate { channel_id: Id, ringing: Vec<Id> },
    /// CALL_DELETE: the call ended.
    CallDelete { channel_id: Id },
    /// TYPING_START (never for me; in guilds only while subscribed with op 37 `typing:true`).
    TypingStart {
        channel_id: Id,
        #[allow(dead_code, reason = "channel ids are global; kept for completeness")]
        guild_id: Option<Id>,
        user_id: Id,
        /// Unix seconds (the indicator expires 10 s after *receipt*).
        #[allow(dead_code, reason = "the indicator uses the receipt time")]
        timestamp: u64,
        /// Guild member with `user` (display name).
        member: Option<Box<Member>>,
    },
    /// Presences by user: PRESENCE_UPDATE (one entry), READY_SUPPLEMENTAL `merged_presences`
    /// (friends + guild members) and member-list items. `Presence::Offline` = forget the user.
    PresenceUpdate(Vec<(Id, UserPresence)>),
    /// My chosen status and custom status, complete (READY `user_settings`/`sessions`,
    /// SESSIONS_REPLACE, USER_SETTINGS_UPDATE merged by the gateway).
    MyStatus { status: UserStatus, custom_status: Option<CustomStatus> },
    /// CHANNEL_PINS_UPDATE (not sent when a pinned message is deleted).
    ChannelPinsUpdate {
        channel_id: Id,
        #[allow(dead_code, reason = "channel ids are global; kept for completeness")]
        guild_id: Option<Id>,
        last_pin_timestamp: Option<String>,
    },
    // ----- social (docs/SOCIAL_DESIGN.md). READY / GUILD_CREATE `threads[]` (joined active
    // threads, each with `member`) are appended to `Guild::channels` by the gateway.
    /// THREAD_CREATE / THREAD_UPDATE (upsert; `guild_id` set). THREAD_CREATE also fires when I
    /// am added to a thread (`member` set); THREAD_UPDATE has no `member`.
    ThreadUpsert(Box<Channel>),
    /// THREAD_DELETE.
    ThreadDelete { id: Id },
    /// THREAD_LIST_SYNC: active threads I can see, `member` filled from `members[]` (mine).
    /// `channel_ids` None = the guild's whole set (known threads missing from it are gone),
    /// else only threads under those parents.
    ThreadListSync { guild_id: Id, channel_ids: Option<Vec<Id>>, threads: Vec<Channel> },
    /// THREAD_MEMBER_UPDATE: my membership of `thread_id` (joined / notification flags / mute).
    ThreadMemberUpdate { thread_id: Id, member: ThreadMember },
    /// THREAD_MEMBERS_UPDATE: new `member_count`; `joined` = I am in `added_members`, `left` =
    /// my id is in `removed_member_ids`.
    ThreadMembersUpdate { thread_id: Id, member_count: u32, joined: Option<ThreadMember>, left: bool },
    /// CHANNEL_RECIPIENT_ADD (group DM).
    ChannelRecipientAdd { channel_id: Id, user: User },
    /// CHANNEL_RECIPIENT_REMOVE; my own id = I left / was removed (drop the channel).
    ChannelRecipientRemove { channel_id: Id, user_id: Id },
    /// RECENT_MENTION_DELETE (removed from the inbox, here or on another device).
    RecentMentionDelete { message_id: Id },
    /// USER_NOTE_UPDATE (`id`, `note`; empty = cleared).
    UserNoteUpdate { user_id: Id, note: String },
    // ----- account & server (docs/ACCOUNT_SERVER_DESIGN.md)
    /// READY `user_settings_proto` (sent right after `Ready`, `partial: false`: absent fields
    /// reset) and USER_SETTINGS_PROTO_UPDATE type 1 (`partial: true`: only present top-level
    /// fields replace).
    UserSettingsProto { settings: UserSettingsProto, partial: bool },
    /// GUILD_BAN_ADD (delivered with BAN_MEMBERS).
    GuildBanAdd { guild_id: Id, user: User },
    /// GUILD_BAN_REMOVE.
    GuildBanRemove { guild_id: Id, user_id: Id },
    /// QUESTS_USER_STATUS_UPDATE, QUESTS_USER_COMPLETION_UPDATE, VIRTUAL_CURRENCY_BALANCE_UPDATE
    /// (docs/QUESTS_NOTES.md).
    Quests(crate::quests::QuestEvent),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn member_timeout_only_in_future() {
        let m = |t: Option<&str>| Member { communication_disabled_until: t.map(Into::into), ..Default::default() };
        assert!(m(Some("2999-01-01T00:00:00.000000+00:00")).timed_out());
        assert!(!m(Some("2020-01-01T00:00:00+00:00")).timed_out());
        assert!(!m(Some("garbage")).timed_out());
        assert!(!m(None).timed_out());
        let wire: Member = serde_json::from_str(r#"{"roles":[],"communication_disabled_until":null}"#).unwrap();
        assert!(!wire.timed_out());
    }

    #[test]
    fn relationship_blocked_or_ignored() {
        let r = |json: &str| serde_json::from_str::<Relationship>(json).unwrap();
        assert!(r(r#"{"id":"2","type":2,"user_id":"2","nickname":null}"#).hides_author());
        assert!(r(r#"{"id":"3","type":1,"user_ignored":true}"#).hides_author());
        let friend = r(r#"{"id":"4","type":1,"user_ignored":false,"user":{"id":"4"}}"#);
        assert!(!friend.hides_author());
        assert_eq!(friend.id, Id(4));
    }

    #[test]
    fn chat_essentials_types() {
        for s in [UserStatus::Online, UserStatus::Idle, UserStatus::Dnd, UserStatus::Invisible] {
            assert_eq!(UserStatus::parse(s.as_str()), Some(s));
        }
        assert_eq!(UserStatus::parse("offline"), None);
        assert_eq!((UserStatus::Dnd.presence(), UserStatus::Invisible.presence()), (Presence::Dnd, Presence::Offline));
        assert_eq!(Activity { kind: 2, name: "Spotify".into(), rich: None }.label(), "Listening to Spotify");
        assert_eq!(Activity { kind: 0, name: "Chess".into(), rich: None }.label(), "Playing Chess");
        let started = |start| Activity { rich: Some(Box::new(RichPresence { start, ..Default::default() })), ..Default::default() };
        let now = 10 * 86_400_000;
        let ago = |mins: u64| started(Some(now - mins * 60_000 - 59_000)).elapsed(now);
        assert_eq!([ago(0), ago(12), ago(120), ago(125), ago(3 * 1440 + 5)].map(Option::unwrap), ["for 0 min", "for 12 min", "for 2 h", "for 2 h 5 min", "for 3 d"]);
        assert_eq!(started(Some(now + 1)).elapsed(now), None, "start in the future");
        assert_eq!((started(None).elapsed(now), Activity::default().elapsed(now)), (None, None));
        assert_eq!(Activity { kind: 1, name: "Twitch".into(), rich: None }.heading(), "Live on Twitch");
        assert_eq!(Activity { kind: 3, name: "YouTube".into(), rich: None }.heading(), "Watching");
        assert_eq!([clock(0), clock(247_900), clock(3_753_000)], ["0:00", "4:07", "1:02:33"]);
        let song = |sync: &str| Activity {
            kind: 2,
            name: "Spotify".into(),
            rich: Some(Box::new(RichPresence { sync_id: Some(sync.into()), start: Some(1_000), end: Some(181_000), ..Default::default() })),
        };
        assert_eq!(song("4uLU6hMCjMI75M1A2tKUQC").spotify_track_url().as_deref(), Some("https://open.spotify.com/track/4uLU6hMCjMI75M1A2tKUQC"));
        for bad in ["4uLU6hMCjMI75M1A2tKUQ", "4uLU6hMCjMI75M1A2tKU/C", "../../evil.example/xx", ""] {
            assert_eq!(song(bad).spotify_track_url(), None, "{bad}");
        }
        assert_eq!(Activity { name: "Chess".into(), ..song("4uLU6hMCjMI75M1A2tKUQC") }.spotify_track_url(), None, "not Spotify");
        assert_eq!(song("x").progress(61_000), Some((60_000, 180_000)));
        assert_eq!(song("x").progress(999_000), Some((180_000, 180_000)), "capped at the end");
        assert_eq!(Activity::default().progress(5), None);
        let stream = |url: &str| Activity { kind: 1, name: "Twitch".into(), rich: Some(Box::new(RichPresence { url: Some(url.into()), ..Default::default() })) };
        assert_eq!(stream("https://www.twitch.tv/rustcord").stream_url(), Some("https://www.twitch.tv/rustcord"));
        assert_eq!(stream("https://www.youtube.com/watch?v=x").stream_url(), Some("https://www.youtube.com/watch?v=x"));
        for bad in ["http://www.twitch.tv/x", "https://www.twitch.tv.evil.example/x", "https://evil@www.twitch.tv/x", "javascript:alert(1)", "file:///c:/x"] {
            assert_eq!(stream(bad).stream_url(), None, "{bad}");
        }
        assert_eq!(Activity { kind: 0, ..stream("https://www.twitch.tv/x") }.stream_url(), None, "not a stream");
        let id = Id::from_timestamp_ms(1_700_000_000_000);
        assert_eq!(id.timestamp_ms(), 1_700_000_000_000);
        assert_eq!(Id::from_timestamp_ms(0), Id(0));
        let pins: PinsPage = serde_json::from_str(
            r#"{"items":[{"pinned_at":"2026-01-01T00:00:00+00:00","message":{"id":"5","channel_id":"1","author":{"id":"2"}}}],"has_more":true}"#,
        )
        .unwrap();
        assert_eq!((pins.items[0].message.id, pins.has_more), (Id(5), true));
        let gif: GifItem =
            serde_json::from_str(r#"{"id":"1","url":"https://klipy.com/gifs/x","src":"https://static.klipy.com/x.gif","preview":null,"width":200,"height":100}"#)
                .unwrap();
        assert_eq!((gif.url.as_str(), gif.preview.as_str(), gif.width), ("https://klipy.com/gifs/x", "", 200));
        let me: User = serde_json::from_str(r#"{"id":"1","premium_type":2}"#).unwrap();
        assert_eq!(me.premium_type, 2);
    }

    #[test]
    fn cosmetics_parse_and_round_trip() {
        let u: User = serde_json::from_str(
            r#"{"id":"1","public_flags":64,"clan":null,
            "primary_guild":{"identity_guild_id":"80351110224678913","identity_enabled":true,"tag":"MEOW","badge":"7d17"},
            "avatar_decoration_data":{"sku_id":"1144058844004233369","asset":"a_fed4","expires_at":null},
            "collectibles":{"nameplate":{"asset":"nameplates/nameplatetest/angel/","palette":"bubble_gum","label":"x","sku_id":"1344802364934062152","expires_at":1700000000}},
            "display_name_styles":{"font_id":12,"effect_id":2,"colors":[2797222,16762000]}}"#,
        )
        .unwrap();
        assert_eq!(u.public_flags, 64);
        assert_eq!(u.primary_guild.as_ref().unwrap().tag.as_deref(), Some("MEOW"));
        assert_eq!(u.avatar_decoration_data.as_ref().unwrap().asset, "a_fed4");
        let np = u.collectibles.as_ref().unwrap().nameplate.as_ref().unwrap();
        assert_eq!((np.palette.as_deref(), np.expires_at), (Some("bubble_gum"), Some(1_700_000_000)));
        assert_eq!(u.display_name_styles.as_ref().unwrap().colors, [2797222, 16762000]);
        // Snapshot round trip keeps them; users without any stay small.
        let back: User = serde_json::from_str(&serde_json::to_string(&u).unwrap()).unwrap();
        assert_eq!((back.primary_guild, back.collectibles), (u.primary_guild, u.collectibles));
        assert!(!serde_json::to_string(&User::default()).unwrap().contains("primary_guild"));
        let m: Member = serde_json::from_str(r#"{"roles":[],"flags":1024,"avatar_decoration_data":null,"display_name_styles":{"font_id":3,"effect_id":1,"colors":null}}"#).unwrap();
        assert_eq!((m.flags, m.display_name_styles.unwrap().colors.len()), (1024, 0));
    }

    #[test]
    fn profile_collectibles_widgets_and_products() {
        let p: UserProfile = serde_json::from_str(
            r#"{"user":{"id":"1"},"user_profile":{"profile_effect":{"id":"5","expires_at":null},
              "collectibles":[{"sku_id":"7","type":3,"expires_at":"2001-01-01T00:00:00Z"},{"sku_id":"8","type":1,"expires_at":4102444800000}]},
            "guild_member_profile":null,
            "widgets":[{"id":"9","data":{"type":"favorite_games","games":[{"game_id":"42","comment":"Best","tags":["expert"]}]}}],
            "connected_accounts":[{"type":"github","id":"1","name":"ferris","verified":true}]}"#,
        )
        .unwrap();
        let now = 1_800_000_000;
        assert_eq!(p.collectible_sku(1, now), Some(Id(8)), "new shape first; ms expiry in 2100");
        assert_eq!(p.collectible_sku(3, now), None, "expired frame");
        assert_eq!(p.widgets[0].data.games[0].game_id, Id(42));
        let legacy: UserProfile = serde_json::from_str(r#"{"user_profile":{"profile_effect":{"id":"5"}}}"#).unwrap();
        assert_eq!(legacy.collectible_sku(1, now), Some(Id(5)));
        let prod: CollectibleProduct = serde_json::from_str(
            r#"{"sku_id":"3","name":"Frame","type":3,"items":[{"type":3,"sku_id":"3","inner_width":1200,"overflow_top":304,
              "layers":[{"id":"a","type":"staple","order":"back","anchor":2,"responsive":false}]},
              {"type":1,"sku_id":"4","reducedMotionSrc":"x/r.png","effects":[{"src":"x/i.png","loop":true,"width":450,"height":880,
               "duration":2880,"start":0,"loopDelay":0,"position":{"x":1,"y":2},"zIndex":100,"randomizedSources":[]}]}]}"#,
        )
        .unwrap();
        let l = &prod.items[0].layers[0];
        assert_eq!((l.kind, l.order, l.anchor), (0, 1, 2));
        let e = &prod.items[1].effects[0];
        assert!(e.looping && e.z_index == 100 && e.position.y == 2.0 && prod.items[1].reduced_motion_src.is_some());
    }

    #[test]
    fn voice_state_lenient_and_guild_cache_skips_it() {
        let v: VoiceState = serde_json::from_str(
            r#"{"guild_id":"1","channel_id":null,"user_id":"2","session_id":"s","self_mute":true,
            "self_deaf":null,"mute":false,"deaf":false,"self_video":false,"suppress":false,
            "request_to_speak_timestamp":null,"member":{"user":{"id":"2"},"roles":[]}}"#,
        )
        .unwrap();
        assert_eq!((v.guild_id, v.channel_id, v.user_id), (Some(Id(1)), None, Id(2)));
        assert!(v.self_mute && !v.self_deaf && !v.self_stream);
        let g = Guild { voice_states: vec![v], ..Default::default() };
        assert!(!serde_json::to_string(&g).unwrap().contains("voice_states"));
    }

    #[test]
    fn attachment_videos() {
        let a = |ct: Option<&str>, name: &str| Attachment { content_type: ct.map(Into::into), filename: name.into(), ..Default::default() };
        assert!(a(Some("video/mp4"), "x").is_video());
        assert!(a(Some("video/quicktime"), "x.mov").is_video());
        assert!(!a(Some("image/gif"), "x.mp4").is_video(), "the content type wins");
        assert!(a(None, "Clip.MOV").is_video() && a(None, "a.webm").is_video() && a(None, "a.m4v").is_video());
        assert!(!a(None, "a.mp3").is_video());
    }

    #[test]
    fn forward_with_a_snapshot_without_id_or_author() {
        let m: Message = serde_json::from_str(
            r#"{"id":"5","channel_id":"2","type":0,"content":"","flags":16384,"message_reference":{"type":1,"message_id":"4","channel_id":"3"},
            "message_snapshots":[{"message":{"content":"hello","timestamp":"2026-01-01T10:00:00+00:00","type":0,"attachments":[{"id":"9","filename":"a.png"}]}}]}"#,
        )
        .unwrap();
        let s = &m.message_snapshots[0].message;
        assert_eq!((s.content.as_str(), s.id, s.author.id, s.attachments.len()), ("hello", Id(0), Id(0), 1));
        assert!(!m.can_edit());
        // A plain message carries no snapshot key into the cache.
        assert!(!serde_json::to_string(&Message::default()).unwrap().contains("message_snapshots"));
    }

    #[test]
    fn only_text_and_replies_can_be_edited() {
        let m = |kind| Message { kind, ..Default::default() };
        assert!(m(0).can_edit() && m(19).can_edit());
        assert!(!m(7).can_edit() && !m(20).can_edit());
        let fwd = Message { message_snapshots: vec![MessageSnapshot::default()], ..Default::default() };
        assert!(!fwd.can_edit());
    }

    #[test]
    fn forwardable_messages() {
        let m = |kind, flags| Message { kind, flags, ..Default::default() };
        assert!(m(0, 0).forwardable() && m(19, 0).forwardable() && m(20, 0).forwardable() && m(23, 0).forwardable());
        assert!(!m(7, 0).forwardable() && !m(46, 0).forwardable());
        assert!(!m(0, 1 << 15).forwardable(), "components v2");
        assert!(!m(0, 1 << 6).forwardable(), "ephemeral");
        assert!(m(0, 1 << 2).forwardable(), "suppressed embeds are fine");
        let fwd = Message { message_snapshots: vec![MessageSnapshot::default()], ..Default::default() };
        assert!(!fwd.forwardable());
        let poll = Message { poll: Some(Box::default()), ..Default::default() };
        assert!(!poll.forwardable() && !poll.can_edit());
    }

    #[test]
    fn polls_parse_with_and_without_results() {
        let wire = |results: &str| {
            format!(
                r#"{{"id":"5","poll":{{"question":{{"text":"Best?"}},"answers":[{{"answer_id":1,"poll_media":{{"text":"A","emoji":{{"name":"x"}}}}}},{{"answer_id":2,"poll_media":{{"text":"B"}}}}],
                "expiry":"2026-01-02T00:00:00.000000+00:00","allow_multiselect":false,"layout_type":1{results}}}}}"#
            )
        };
        let m: Message = serde_json::from_str(&wire("")).unwrap();
        let p = m.poll.as_deref().unwrap();
        assert_eq!((p.question.text.as_str(), p.answers.len(), p.answers[1].answer_id), ("Best?", 2, 2));
        assert!(p.results.is_none() && p.answers[0].poll_media.emoji.is_some());
        let m: Message = serde_json::from_str(&wire(r#","results":{"is_finalized":true,"answer_counts":[{"id":2,"count":7,"me_voted":true}]}"#)).unwrap();
        let r = m.poll.as_deref().unwrap().results.as_ref().unwrap();
        assert!(r.is_finalized && (r.answer_counts[0].id, r.answer_counts[0].count, r.answer_counts[0].me_voted) == (2, 7, true));
        // A plain message carries no poll key into the cache.
        assert!(!serde_json::to_string(&Message::default()).unwrap().contains("\"poll\""));
        let u: MessageUpdate = serde_json::from_str(r#"{"id":"1","channel_id":"2"}"#).unwrap();
        assert!(u.poll.is_none());
    }
}