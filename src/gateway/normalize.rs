//! Wire payloads -> `model::GatewayEvent`.
//!
//! Every payload is parsed twice straight from the decompressed bytes: once for the envelope
//! (`op`/`s`/`t`, skipping `d`), then once into the typed `d` of the events we use. No
//! intermediate `serde_json::Value`, so READY is never held as a JSON tree.

use crate::model::*;
use serde::de::{self, Deserializer, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::Deserialize;
use std::collections::HashMap;
use std::fmt;
use std::marker::PhantomData;

/// Envelope without `d`.
#[derive(Deserialize)]
pub(super) struct Head {
    pub op: u8,
    #[serde(default)]
    pub s: Option<u64>,
    #[serde(default)]
    pub t: Option<String>,
}

#[derive(Deserialize)]
struct Data<T> {
    d: T,
}

/// Parses the payload's `d` as `T`.
pub(super) fn data<'a, T: Deserialize<'a>>(bytes: &'a [u8]) -> serde_json::Result<T> {
    serde_json::from_slice::<Data<T>>(bytes).map(|x| x.d)
}

// ---------------------------------------------------------------------------
// Wire shapes
// ---------------------------------------------------------------------------

/// Guild member as found in merged_members (`user_id`), GUILD_CREATE / member lists (`user`)
/// and GUILD_MEMBER_UPDATE (`guild_id` + `user`).
#[derive(Deserialize, Default)]
#[serde(default)]
struct MemberWire {
    guild_id: Id,
    user_id: Option<Id>,
    user: Option<User>,
    nick: Option<String>,
    avatar: Option<String>,
    #[serde(deserialize_with = "null_default")]
    roles: Vec<Id>,
    presence: Option<PresenceWire>,
    communication_disabled_until: Option<String>,
    // Cosmetics (COSMETICS_NOTES §0.2).
    #[serde(deserialize_with = "null_default")]
    flags: u64,
    avatar_decoration_data: Option<Box<crate::model::DecorationData>>,
    collectibles: Option<Box<crate::model::Collectibles>>,
    display_name_styles: Option<Box<crate::model::NameStyle>>,
}

impl MemberWire {
    fn user_id(&self) -> Id {
        self.user_id.or(self.user.as_ref().map(|u| u.id)).unwrap_or_default()
    }

    fn into_member(self) -> Member {
        Member {
            user: self.user,
            nick: self.nick,
            avatar: self.avatar,
            roles: self.roles,
            communication_disabled_until: self.communication_disabled_until,
            flags: self.flags,
            avatar_decoration_data: self.avatar_decoration_data,
            collectibles: self.collectibles,
            display_name_styles: self.display_name_styles,
        }
    }
}

/// PRESENCE_UPDATE (`user`), merged_presences entries (`user_id`) and member-list `presence`.
/// UD: the field types "are not validated", so every field is lenient.
#[derive(Deserialize, Default)]
#[serde(default)]
struct PresenceWire {
    user_id: Option<Id>,
    user: Option<UserRef>,
    #[serde(deserialize_with = "lenient_string")]
    status: String,
    #[serde(deserialize_with = "lenient_vec")]
    activities: Vec<ActivityWire>,
    client_status: serde_json::Value,
}

impl PresenceWire {
    fn user_id(&self) -> Id {
        self.user_id.or(self.user.as_ref().map(|u| u.id)).unwrap_or_default()
    }

    fn into_presence(self) -> UserPresence {
        let mobile = matches!(&self.client_status, serde_json::Value::Object(m) if m.len() == 1 && m.contains_key("mobile"));
        // Offline (or invisible) shows no platform, whatever client_status still lists.
        let offline = presence(&self.status) == Presence::Offline;
        let platforms = if offline { 0 } else { crate::plugins::people::platform::from_client_status(&self.client_status) };
        let mut p = UserPresence { status: presence(&self.status), mobile, platforms, ..Default::default() };
        for a in self.activities {
            match a.kind {
                4 if p.custom_status.is_none() => p.custom_status = custom_status(a.state, a.emoji),
                0..=3 | 5 if p.activity.is_none() => p.activity = Some(a.into_activity()),
                _ => {}
            }
        }
        p
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct UserRef {
    id: Id,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ActivityWire {
    #[serde(rename = "type", deserialize_with = "null_default")]
    kind: u8,
    #[serde(deserialize_with = "null_default")]
    name: String,
    /// Custom status text (type 4).
    state: Option<String>,
    emoji: Option<Emoji>,
    // Rich presence, read leniently (a bad field only loses itself).
    details: serde_json::Value,
    timestamps: serde_json::Value,
    application_id: serde_json::Value,
    assets: serde_json::Value,
    sync_id: serde_json::Value,
    url: serde_json::Value,
    party: serde_json::Value,
}

impl ActivityWire {
    fn into_activity(self) -> Activity {
        let text = |v: &serde_json::Value| v.as_str().filter(|s| !s.is_empty()).map(str::to_owned);
        let ms = |v: &serde_json::Value| v.as_u64().or_else(|| v.as_str()?.parse().ok());
        let size = &self.party["size"];
        let party = size.as_array().filter(|s| s.len() == 2).and_then(|s| Some([u32::try_from(s[0].as_u64()?).ok()?, u32::try_from(s[1].as_u64()?).ok()?]));
        let rich = RichPresence {
            details: text(&self.details),
            state: self.state.filter(|s| !s.is_empty()),
            start: ms(&self.timestamps["start"]),
            application_id: Id::deserialize(&self.application_id).ok().filter(|id| !id.is_zero()),
            large_image: text(&self.assets["large_image"]),
            large_text: text(&self.assets["large_text"]),
            small_image: text(&self.assets["small_image"]),
            small_text: text(&self.assets["small_text"]),
            end: ms(&self.timestamps["end"]),
            sync_id: text(&self.sync_id),
            url: text(&self.url),
            party: party.filter(|p| p[1] > 0),
        };
        Activity { kind: self.kind, name: self.name, rich: (rich != RichPresence::default()).then(|| Box::new(rich)) }
    }
}

/// `user_settings.custom_status` (READY, USER_SETTINGS_UPDATE).
#[derive(Deserialize, Default)]
#[serde(default)]
struct CustomStatusWire {
    text: Option<String>,
    emoji_id: Option<Id>,
    emoji_name: Option<String>,
    expires_at: Option<String>,
}

impl CustomStatusWire {
    /// The custom status (None once expired) and when it expires.
    fn into_custom(self) -> (Option<CustomStatus>, Option<chrono::DateTime<chrono::Utc>>) {
        let expires = self.expires_at.as_deref().and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok()).map(|t| t.to_utc());
        if expires.is_some_and(|t| t <= chrono::Utc::now()) {
            return (None, None);
        }
        (custom_status(self.text, Some(Emoji { id: self.emoji_id, name: self.emoji_name, animated: false })), expires)
    }
}

/// READY `sessions[]` / SESSIONS_REPLACE entry.
#[derive(Deserialize, Default)]
#[serde(default)]
struct SessionWire {
    #[serde(deserialize_with = "lenient_string")]
    session_id: String,
    #[serde(deserialize_with = "lenient_string")]
    status: String,
    #[serde(deserialize_with = "lenient_vec")]
    activities: Vec<ActivityWire>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct TypingWire {
    channel_id: Id,
    guild_id: Option<Id>,
    user_id: Id,
    #[serde(deserialize_with = "u64_lenient")]
    timestamp: u64,
    member: Option<Box<Member>>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct PinsWire {
    channel_id: Id,
    guild_id: Option<Id>,
    last_pin_timestamp: Option<String>,
}

/// GUILD_MEMBERS_CHUNK (answer to op 8).
#[derive(Deserialize, Default)]
#[serde(default)]
pub(super) struct MembersChunk {
    #[serde(deserialize_with = "null_default")]
    pub members: Vec<Member>,
    #[serde(deserialize_with = "null_default")]
    pub chunk_index: u32,
    #[serde(deserialize_with = "null_default")]
    pub chunk_count: u32,
    pub nonce: Option<String>,
}

/// RATE_LIMITED dispatch `{opcode, retry_after (float s), meta {guild_id, nonce?}}`.
#[derive(Deserialize, Default)]
#[serde(default)]
pub(super) struct RateLimited {
    #[serde(deserialize_with = "null_default")]
    pub opcode: u8,
    #[serde(deserialize_with = "null_default")]
    pub retry_after: f64,
    #[serde(deserialize_with = "null_default")]
    pub meta: RateLimitMeta,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub(super) struct RateLimitMeta {
    pub nonce: Option<String>,
}

/// What a payload says about my own status; `None` = not mentioned.
#[derive(Debug, Default, PartialEq)]
pub(super) struct StatusPatch {
    pub status: Option<UserStatus>,
    /// `Some(None)` = no custom status.
    pub custom: Option<Option<CustomStatus>>,
    /// When `custom` expires; only user settings carry it.
    pub expires: Option<chrono::DateTime<chrono::Utc>>,
    /// From user settings (READY `user_settings`, USER_SETTINGS_UPDATE), not a session list.
    pub settings: bool,
}

/// Gateway guild: flat REST fields, or (CLIENT_STATE_V2) REST fields nested in `properties`.
#[derive(Deserialize, Default)]
#[serde(default)]
struct GuildWire {
    id: Id,
    #[serde(deserialize_with = "null_default")]
    name: String,
    icon: Option<String>,
    owner_id: Id,
    #[serde(deserialize_with = "null_default")]
    default_message_notifications: u8,
    #[serde(deserialize_with = "null_default")]
    unavailable: bool,
    #[serde(deserialize_with = "null_default")]
    premium_tier: u8,
    #[serde(deserialize_with = "null_default")]
    mfa_level: u8,
    rules_channel_id: Option<Id>,
    #[serde(deserialize_with = "null_default")]
    features: Vec<String>,
    /// Top level even with CLIENT_STATE_V2 (not in `properties`).
    #[serde(deserialize_with = "null_default")]
    premium_subscription_count: u32,
    #[serde(deserialize_with = "null_default")]
    roles: Vec<Role>,
    #[serde(deserialize_with = "null_default")]
    emojis: Vec<CustomEmoji>,
    #[serde(deserialize_with = "null_default")]
    channels: Vec<Channel>,
    /// My joined active threads (each with `member`); appended to `channels`.
    #[serde(deserialize_with = "null_default")]
    threads: Vec<Channel>,
    properties: Option<Guild>,
    /// Only without DEDUPE_USER_OBJECTS (and in GUILD_CREATE): inline `user` objects.
    #[serde(deserialize_with = "null_default")]
    members: Vec<MemberWire>,
    #[serde(deserialize_with = "null_default")]
    voice_states: Vec<VoiceState>,
}

impl GuildWire {
    fn into_guild(self) -> (Guild, Vec<MemberWire>) {
        let mut g = Guild {
            id: self.id,
            name: self.name,
            icon: self.icon,
            owner_id: self.owner_id,
            roles: self.roles,
            emojis: self.emojis,
            channels: self.channels.into_iter().chain(self.threads).collect(),
            default_message_notifications: self.default_message_notifications,
            unavailable: self.unavailable,
            premium_tier: self.premium_tier,
            features: self.features,
            premium_subscription_count: self.premium_subscription_count,
            mfa_level: self.mfa_level,
            rules_channel_id: self.rules_channel_id,
            voice_states: self.voice_states,
        };
        if let Some(p) = self.properties {
            if g.id.is_zero() {
                g.id = p.id;
            }
            g.name = p.name;
            g.icon = p.icon;
            g.owner_id = p.owner_id;
            g.default_message_notifications = p.default_message_notifications;
            g.premium_tier = p.premium_tier;
            g.features = p.features;
            g.mfa_level = p.mfa_level;
            g.rules_channel_id = p.rules_channel_id;
        }
        // Only what the UI reads: a big guild lists ~40 features.
        g.features.retain(|f| matches!(f.as_str(), "VERIFIED" | "PARTNERED"));
        // Channels and voice states inside guild dispatches may lack guild_id.
        for c in &mut g.channels {
            c.guild_id = Some(g.id);
        }
        for v in &mut g.voice_states {
            v.guild_id = Some(g.id);
        }
        (g, self.members)
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ReadyWire {
    user: User,
    #[serde(deserialize_with = "null_default")]
    session_id: String,
    resume_gateway_url: Option<String>,
    #[serde(deserialize_with = "null_default")]
    guilds: Vec<GuildWire>,
    /// Index-aligned with `guilds`; with PRIORITIZED_READY_PAYLOAD only my own member.
    #[serde(deserialize_with = "null_default")]
    merged_members: Vec<Vec<MemberWire>>,
    #[serde(deserialize_with = "null_default")]
    private_channels: Vec<Channel>,
    #[serde(deserialize_with = "null_default")]
    users: Vec<User>,
    #[serde(deserialize_with = "entries")]
    read_state: Vec<ReadState>,
    #[serde(deserialize_with = "entries")]
    user_guild_settings: Vec<GuildSettings>,
    user_settings: Option<UserSettingsWire>,
    /// Base64 `PreloadedUserSettings` (gateway/account.rs decodes it).
    user_settings_proto: Option<String>,
    #[serde(deserialize_with = "null_default")]
    auth_session_id_hash: String,
    #[serde(deserialize_with = "null_default")]
    relationships: Vec<Relationship>,
    #[serde(deserialize_with = "null_default")]
    sessions: Vec<SessionWire>,
}

/// READY `user_settings`, or the partial object of USER_SETTINGS_UPDATE.
#[derive(Deserialize, Default)]
#[serde(default)]
struct UserSettingsWire {
    #[serde(deserialize_with = "null_default")]
    guild_folders: Vec<FolderWire>,
    status: Option<String>,
    #[serde(deserialize_with = "present")]
    custom_status: Option<Option<CustomStatusWire>>,
}

impl UserSettingsWire {
    fn status_patch(self) -> StatusPatch {
        let (custom, expires) = match self.custom_status {
            Some(c) => {
                let (c, e) = c.map_or((None, None), CustomStatusWire::into_custom);
                (Some(c), e)
            }
            None => (None, None),
        };
        StatusPatch { status: self.status.as_deref().and_then(UserStatus::parse), custom, expires, settings: true }
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct FolderWire {
    #[serde(deserialize_with = "null_default")]
    guild_ids: Vec<Id>,
}

/// Small dispatches; each event reads the subset of fields it carries.
#[derive(Deserialize, Default)]
#[serde(default)]
struct Fields {
    id: Id,
    channel_id: Id,
    message_id: Id,
    user_id: Id,
    guild_id: Id,
    role_id: Id,
    #[serde(deserialize_with = "null_default")]
    ids: Vec<Id>,
    #[serde(deserialize_with = "null_default")]
    emoji: Emoji,
    #[serde(deserialize_with = "null_default")]
    answer_id: u32,
    burst: Option<bool>,
    /// Reaction type: 0 normal, 1 burst.
    #[serde(rename = "type", deserialize_with = "null_default")]
    kind: u8,
    mention_count: Option<u32>,
    #[serde(deserialize_with = "null_default")]
    unavailable: bool,
    #[serde(deserialize_with = "null_default")]
    role: Role,
    #[serde(deserialize_with = "null_default")]
    emojis: Vec<CustomEmoji>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct MemberListWire {
    guild_id: Id,
    #[serde(deserialize_with = "lenient_string")]
    id: String,
    #[serde(deserialize_with = "null_default")]
    member_count: u32,
    #[serde(deserialize_with = "null_default")]
    online_count: u32,
    #[serde(deserialize_with = "null_default")]
    ops: Vec<OpWire>,
    #[serde(deserialize_with = "null_default")]
    groups: Vec<GroupWire>,
}

/// READY_SUPPLEMENTAL: the per-guild voice states and `merged_presences` are used.
#[derive(Deserialize, Default)]
#[serde(default)]
struct SupplementalWire {
    #[serde(deserialize_with = "null_default")]
    guilds: Vec<SupplementalGuildWire>,
    #[serde(deserialize_with = "null_default")]
    merged_presences: MergedPresencesWire,
}

/// Friends, then per guild (index-aligned with READY guilds; status is global, so unused).
#[derive(Deserialize, Default)]
#[serde(default)]
struct MergedPresencesWire {
    #[serde(deserialize_with = "null_default")]
    friends: Vec<PresenceWire>,
    #[serde(deserialize_with = "null_default")]
    guilds: Vec<Vec<PresenceWire>>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct SupplementalGuildWire {
    id: Id,
    #[serde(deserialize_with = "null_default")]
    voice_states: Vec<VoiceState>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct VoiceServerWire {
    guild_id: Option<Id>,
    channel_id: Option<Id>,
    endpoint: Option<String>,
    #[serde(deserialize_with = "null_default")]
    token: String,
}

/// CHANNEL_UNREAD_UPDATE.
#[derive(Deserialize, Default)]
#[serde(default)]
struct UnreadWire {
    #[serde(deserialize_with = "null_default")]
    channel_unread_updates: Vec<UnreadEntryWire>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct UnreadEntryWire {
    id: Id,
    last_message_id: Option<Id>,
}

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "SCREAMING_SNAKE_CASE")]
enum OpWire {
    Sync {
        range: [u32; 2],
        #[serde(default, deserialize_with = "null_default")]
        items: Vec<ItemWire>,
    },
    Invalidate {
        range: [u32; 2],
    },
    Insert {
        index: u32,
        item: ItemWire,
    },
    Update {
        index: u32,
        item: ItemWire,
    },
    Delete {
        index: u32,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum ItemWire {
    Group(GroupWire),
    Member(Box<MemberWire>),
}

#[derive(Deserialize)]
struct GroupWire {
    #[serde(deserialize_with = "lenient_string")]
    id: String,
    #[serde(default)]
    count: Option<u32>,
}

// ---------------------------------------------------------------------------
// serde helpers
// ---------------------------------------------------------------------------

/// A plain array, `null`, or the versioned `{entries, partial, version}` object.
fn entries<'de, D, T>(d: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct V<T>(PhantomData<T>);
    impl<'de, T: Deserialize<'de>> Visitor<'de> for V<T> {
        type Value = Vec<T>;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("an array or an object with `entries`")
        }
        fn visit_unit<E: de::Error>(self) -> Result<Vec<T>, E> {
            Ok(Vec::new())
        }
        fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<Vec<T>, A::Error> {
            Vec::<T>::deserialize(de::value::SeqAccessDeserializer::new(seq))
        }
        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Vec<T>, A::Error> {
            let mut out = Vec::new();
            while let Some(key) = map.next_key::<String>()? {
                if key == "entries" {
                    out = map.next_value::<Option<Vec<T>>>()?.unwrap_or_default();
                } else {
                    map.next_value::<IgnoredAny>()?;
                }
            }
            Ok(out)
        }
    }
    d.deserialize_any(V(PhantomData))
}

/// String, or a number rendered as a string (member-list ids are "snowflake | string").
fn lenient_string<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    Ok(match serde_json::Value::deserialize(d)? {
        serde_json::Value::String(s) => s,
        serde_json::Value::Number(n) => n.to_string(),
        _ => String::new(),
    })
}

/// An array whose unreadable elements are skipped; anything else is empty.
fn lenient_vec<'de, D: Deserializer<'de>, T: de::DeserializeOwned>(d: D) -> Result<Vec<T>, D::Error> {
    Ok(match serde_json::Value::deserialize(d)? {
        serde_json::Value::Array(a) => a.into_iter().filter_map(|v| T::deserialize(v).ok()).collect(),
        _ => Vec::new(),
    })
}

/// With `#[serde(default)]`: missing -> None, `null` -> Some(None).
fn present<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Option<T>, D::Error> {
    T::deserialize(d).map(Some)
}

// ---------------------------------------------------------------------------
// Normalization
// ---------------------------------------------------------------------------

pub(super) struct Ready {
    pub data: ReadyData,
    pub session_id: String,
    pub resume_url: Option<String>,
    /// From `user_settings`, else `sessions`.
    pub status: StatusPatch,
    /// `user_settings_proto`; None when absent or undecodable (logged).
    pub settings: Option<UserSettingsProto>,
}

pub(super) fn ready(bytes: &[u8]) -> serde_json::Result<Ready> {
    let w: ReadyWire = data(bytes)?;
    let me = w.user.id;

    // Non-deduped private channels carry full recipients: move them into `users`.
    let mut users = w.users;
    let mut private_channels = w.private_channels;
    for c in &mut private_channels {
        for u in c.recipients.drain(..) {
            if !c.recipient_ids.contains(&u.id) {
                c.recipient_ids.push(u.id);
            }
            users.push(u);
        }
    }
    users.sort_unstable_by_key(|u| u.id);
    users.dedup_by_key(|u| u.id);

    let mut merged = w.merged_members.into_iter();
    let mut guilds = Vec::with_capacity(w.guilds.len());
    let mut my_members = Vec::new();
    for gw in w.guilds {
        let slot = merged.next().unwrap_or_default();
        let (g, members) = gw.into_guild();
        if let Some(m) = slot.into_iter().chain(members).find(|m| m.user_id() == me) {
            my_members.push((g.id, m.into_member()));
        }
        guilds.push(g);
    }
    if let Some(s) = &w.user_settings {
        sort_guilds(&mut guilds, &s.guild_folders);
    }
    let status = match w.user_settings {
        Some(s) if s.status.is_some() => {
            let mut p = s.status_patch();
            p.custom.get_or_insert(None); // READY is complete: missing = none
            p
        }
        _ => sessions_status(w.sessions, &w.session_id),
    };

    let mut read_states = w.read_state;
    read_states.retain(|r| r.read_state_type == 0);

    Ok(Ready {
        status,
        data: ReadyData {
            user: w.user,
            guilds,
            private_channels,
            users,
            my_members,
            read_states,
            guild_settings: w.user_guild_settings,
            relationships: w.relationships,
            auth_session_id_hash: w.auth_session_id_hash,
        },
        session_id: w.session_id,
        resume_url: w.resume_gateway_url.filter(|u| !u.is_empty()),
        settings: w.user_settings_proto.as_deref().and_then(super::account::settings),
    })
}

/// My status from a session list: the aggregate `all` entry (what others see), else this
/// session's (DPS).
fn sessions_status(mut sessions: Vec<SessionWire>, mine: &str) -> StatusPatch {
    let i = sessions.iter().position(|s| s.session_id == "all").or_else(|| sessions.iter().position(|s| s.session_id == mine));
    let Some(s) = i.map(|i| sessions.swap_remove(i)) else { return StatusPatch::default() };
    let custom = s.activities.into_iter().find(|a| a.kind == 4).and_then(|a| custom_status(a.state, a.emoji));
    StatusPatch { status: UserStatus::parse(&s.status), custom: Some(custom), ..Default::default() }
}

/// USER_SETTINGS_UPDATE (partial: only changed fields).
pub(super) fn settings_update(b: &[u8]) -> serde_json::Result<StatusPatch> {
    Ok(data::<UserSettingsWire>(b)?.status_patch())
}

/// SESSIONS_REPLACE (`d` is the session array). `mine` = this session's id.
pub(super) fn sessions_replace(b: &[u8], mine: &str) -> serde_json::Result<StatusPatch> {
    Ok(sessions_status(data(b)?, mine))
}

/// SESSIONS_REPLACE: my presence as others see it (the `all` session, e.g. a game another client
/// of mine detected); PRESENCE_UPDATE never comes for me. None without an `all` entry.
pub(super) fn my_presence(b: &[u8]) -> serde_json::Result<Option<UserPresence>> {
    let sessions: Vec<SessionWire> = data(b)?;
    let all = sessions.into_iter().find(|s| s.session_id == "all");
    Ok(all.map(|s| PresenceWire { status: s.status, activities: s.activities, ..Default::default() }.into_presence()))
}

pub(super) fn members_chunk(b: &[u8]) -> serde_json::Result<MembersChunk> {
    data(b)
}

pub(super) fn rate_limited(b: &[u8]) -> serde_json::Result<RateLimited> {
    data(b)
}

/// Text and/or emoji; None when both are empty.
fn custom_status(text: Option<String>, emoji: Option<Emoji>) -> Option<CustomStatus> {
    let text = text.filter(|t| !t.is_empty());
    let emoji = emoji.filter(|e| e.id.is_some() || e.name.as_deref().is_some_and(|n| !n.is_empty()));
    (text.is_some() || emoji.is_some()).then_some(CustomStatus { text, emoji })
}

fn presences(list: impl IntoIterator<Item = PresenceWire>) -> Vec<(Id, UserPresence)> {
    list.into_iter()
        .filter_map(|p| {
            let id = p.user_id();
            (!id.is_zero()).then(|| (id, p.into_presence()))
        })
        .collect()
}

/// Sidebar order: guilds outside every folder first (newest first), then folder order.
fn sort_guilds(guilds: &mut [Guild], folders: &[FolderWire]) {
    let pos: HashMap<Id, usize> =
        folders.iter().flat_map(|f| &f.guild_ids).enumerate().map(|(i, id)| (*id, i)).collect();
    if pos.is_empty() {
        return;
    }
    guilds.sort_by_key(|g| match pos.get(&g.id) {
        None => (0, std::cmp::Reverse(g.id), 0),
        Some(&i) => (1, std::cmp::Reverse(Id(0)), i),
    });
}

/// Normalizes one dispatch (other than READY/RESUMED). Unknown events yield nothing.
pub(super) fn dispatch(t: &str, b: &[u8], me: Id) -> serde_json::Result<Vec<GatewayEvent>> {
    use GatewayEvent as E;
    let ev = match t {
        "MESSAGE_CREATE" => E::MessageCreate(data(b)?),
        "MESSAGE_UPDATE" => E::MessageUpdate(data(b)?),
        "MESSAGE_DELETE" => {
            let f: Fields = data(b)?;
            E::MessageDelete { id: f.id, channel_id: f.channel_id }
        }
        "MESSAGE_DELETE_BULK" => {
            let f: Fields = data(b)?;
            E::MessageDeleteBulk { ids: f.ids, channel_id: f.channel_id }
        }
        "MESSAGE_REACTION_ADD" | "MESSAGE_REACTION_REMOVE" => {
            let f: Fields = data(b)?;
            let burst = f.burst.unwrap_or(f.kind == 1);
            let (channel_id, message_id, user_id, emoji) = (f.channel_id, f.message_id, f.user_id, f.emoji);
            if t == "MESSAGE_REACTION_ADD" {
                E::ReactionAdd { channel_id, message_id, user_id, emoji, burst }
            } else {
                E::ReactionRemove { channel_id, message_id, user_id, emoji, burst }
            }
        }
        "MESSAGE_REACTION_REMOVE_ALL" => {
            let f: Fields = data(b)?;
            E::ReactionRemoveAll { channel_id: f.channel_id, message_id: f.message_id }
        }
        "MESSAGE_REACTION_REMOVE_EMOJI" => {
            let f: Fields = data(b)?;
            E::ReactionRemoveEmoji { channel_id: f.channel_id, message_id: f.message_id, emoji: f.emoji }
        }
        "MESSAGE_POLL_VOTE_ADD" | "MESSAGE_POLL_VOTE_REMOVE" => {
            let f: Fields = data(b)?;
            E::PollVote { channel_id: f.channel_id, message_id: f.message_id, user_id: f.user_id, answer_id: f.answer_id, add: t == "MESSAGE_POLL_VOTE_ADD" }
        }
        "MESSAGE_ACK" => {
            let f: Fields = data(b)?;
            E::MessageAck { channel_id: f.channel_id, message_id: f.message_id, mention_count: f.mention_count }
        }
        "CHANNEL_CREATE" => E::ChannelCreate(data(b)?),
        "CHANNEL_UPDATE" => E::ChannelUpdate(data(b)?),
        "CHANNEL_DELETE" => {
            let c: Channel = data(b)?;
            E::ChannelDelete { id: c.id, guild_id: c.guild_id }
        }
        "GUILD_CREATE" => {
            let (g, members) = data::<GuildWire>(b)?.into_guild();
            let guild_id = g.id;
            let mut out = vec![E::GuildCreate(Box::new(g))];
            if let Some(m) = members.into_iter().find(|m| !me.is_zero() && m.user_id() == me) {
                out.push(E::MyMemberUpdate { guild_id, member: m.into_member() });
            }
            return Ok(out);
        }
        "GUILD_UPDATE" => {
            let (mut g, _) = data::<GuildWire>(b)?.into_guild();
            g.channels.clear();
            E::GuildUpdate(Box::new(g))
        }
        "GUILD_DELETE" => {
            let f: Fields = data(b)?;
            E::GuildDelete { id: f.id, unavailable: f.unavailable }
        }
        "GUILD_ROLE_CREATE" | "GUILD_ROLE_UPDATE" => {
            let f: Fields = data(b)?;
            E::GuildRoleUpsert { guild_id: f.guild_id, role: f.role }
        }
        "GUILD_ROLE_DELETE" => {
            let f: Fields = data(b)?;
            E::GuildRoleDelete { guild_id: f.guild_id, role_id: f.role_id }
        }
        "GUILD_EMOJIS_UPDATE" => {
            let f: Fields = data(b)?;
            E::GuildEmojisUpdate { guild_id: f.guild_id, emojis: f.emojis }
        }
        // For user accounts GUILD_MEMBER_ADD only fires for ourselves (joining a guild).
        "GUILD_MEMBER_UPDATE" | "GUILD_MEMBER_ADD" => {
            let m: MemberWire = data(b)?;
            if me.is_zero() || m.user_id() != me {
                return Ok(Vec::new());
            }
            E::MyMemberUpdate { guild_id: m.guild_id, member: m.into_member() }
        }
        "USER_UPDATE" => E::UserUpdate(data(b)?),
        "USER_GUILD_SETTINGS_UPDATE" => E::GuildSettingsUpdate(data(b)?),
        "GUILD_MEMBER_LIST_UPDATE" => {
            let (u, presences) = member_list(data(b)?);
            let mut out = vec![E::MemberListUpdate(Box::new(u))];
            if !presences.is_empty() {
                out.push(E::PresenceUpdate(presences));
            }
            return Ok(out);
        }
        "PRESENCE_UPDATE" => {
            let list = presences([data::<PresenceWire>(b)?]);
            if list.is_empty() {
                return Ok(Vec::new());
            }
            E::PresenceUpdate(list)
        }
        "TYPING_START" => {
            let w: TypingWire = data(b)?;
            E::TypingStart { channel_id: w.channel_id, guild_id: w.guild_id, user_id: w.user_id, timestamp: w.timestamp, member: w.member }
        }
        "CHANNEL_PINS_UPDATE" => {
            let w: PinsWire = data(b)?;
            E::ChannelPinsUpdate { channel_id: w.channel_id, guild_id: w.guild_id, last_pin_timestamp: w.last_pin_timestamp }
        }
        "CHANNEL_UNREAD_UPDATE" => {
            let w: UnreadWire = data(b)?;
            E::ChannelLastMessage {
                updates: w.channel_unread_updates.into_iter().map(|u| (u.id, u.last_message_id)).collect(),
            }
        }
        "RELATIONSHIP_ADD" | "RELATIONSHIP_UPDATE" => E::RelationshipUpsert(data(b)?),
        "RELATIONSHIP_REMOVE" => {
            let r: Relationship = data(b)?;
            E::RelationshipRemove { id: r.id }
        }
        "VOICE_STATE_UPDATE" => E::VoiceStateUpdate(data(b)?),
        "VOICE_SERVER_UPDATE" => {
            let w: VoiceServerWire = data(b)?;
            let endpoint = w.endpoint.filter(|e| !e.is_empty());
            let token = crate::auth::Secret::new(w.token);
            E::VoiceServerUpdate { guild_id: w.guild_id, channel_id: w.channel_id, endpoint, token }
        }
        // Voice states of all guilds and the presences (PRIORITIZED_READY_PAYLOAD moves them
        // out of READY).
        "READY_SUPPLEMENTAL" => {
            let w: SupplementalWire = data(b)?;
            let states = w.guilds.into_iter().flat_map(|g| {
                let guild_id = Some(g.id);
                g.voice_states.into_iter().map(move |mut v| {
                    v.guild_id = guild_id;
                    E::VoiceStateUpdate(Box::new(v))
                })
            });
            let mut out: Vec<E> = states.collect();
            let p = w.merged_presences;
            let list = presences(p.friends.into_iter().chain(p.guilds.into_iter().flatten()));
            if !list.is_empty() {
                out.push(E::PresenceUpdate(list));
            }
            return Ok(out);
        }
        _ if super::social::PEOPLE_EVENTS.contains(&t) => return Ok(super::social::people(t, b)?.into_iter().collect()),
        _ if super::account::EVENTS.contains(&t) => return Ok(super::account::dispatch(t, b)?.into_iter().collect()),
        _ if super::quests::EVENTS.contains(&t) => return Ok(super::quests::dispatch(t, b)?.into_iter().collect()),
        _ if super::stream::EVENTS.contains(&t) => return Ok(super::stream::dispatch(t, b)?.into_iter().collect()),
        _ if super::calls::EVENTS.contains(&t) => return super::calls::dispatch(t, b),
        _ if super::threads::EVENTS.contains(&t) => return Ok(super::threads::dispatch(t, b, me)?.into_iter().collect()),
        _ => return Ok(Vec::new()),
    };
    Ok(vec![ev])
}

/// The list update, plus the presences of the members it carries.
fn member_list(w: MemberListWire) -> (MemberListUpdate, Vec<(Id, UserPresence)>) {
    let mut presences = Vec::new();
    let mut item = |it: ItemWire| list_item(it, &mut presences);
    let ops = w
        .ops
        .into_iter()
        .filter_map(|op| {
            Some(match op {
                OpWire::Sync { range: [s, e], items } => {
                    MemberListOp::Sync { range: (s, e), items: items.into_iter().map(&mut item).collect() }
                }
                OpWire::Invalidate { range: [s, e] } => MemberListOp::Invalidate { range: (s, e) },
                OpWire::Insert { index, item: it } => MemberListOp::Insert { index, item: item(it) },
                OpWire::Update { index, item: it } => MemberListOp::Update { index, item: item(it) },
                OpWire::Delete { index } => MemberListOp::Delete { index },
                OpWire::Unknown => return None,
            })
        })
        .collect();
    let update = MemberListUpdate {
        guild_id: w.guild_id,
        list_id: w.id,
        member_count: w.member_count,
        online_count: w.online_count,
        ops,
        groups: w.groups.into_iter().map(|g| (g.id, g.count.unwrap_or(0))).collect(),
    };
    (update, presences)
}

fn list_item(w: ItemWire, presences: &mut Vec<(Id, UserPresence)>) -> MemberListItem {
    match w {
        ItemWire::Group(g) => MemberListItem::Group { id: g.id, count: g.count.unwrap_or(0) },
        ItemWire::Member(mut m) => {
            let p = m.presence.take().map(PresenceWire::into_presence).unwrap_or_default();
            let status = p.status;
            let id = m.user_id();
            if !id.is_zero() {
                presences.push((id, p));
            }
            MemberListItem::Member { member: m.into_member(), status }
        }
    }
}

fn presence(status: &str) -> Presence {
    match status {
        "online" => Presence::Online,
        "idle" => Presence::Idle,
        "dnd" => Presence::Dnd,
        _ => Presence::Offline, // offline, invisible, unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready_of(json: &str) -> Ready {
        let head: Head = serde_json::from_str(json).unwrap();
        assert_eq!((head.op, head.t.as_deref()), (0, Some("READY")));
        ready(json.as_bytes()).unwrap()
    }

    #[test]
    fn ready_flat_guilds_deduped_users_versioned_states() {
        let r = ready_of(
            r#"{"op":0,"t":"READY","s":1,"d":{"v":9,"user":{"id":"1","username":"me"},
            "session_id":"sess","resume_gateway_url":"wss://resume.example",
            "guilds":[
              {"id":"10","name":"G","icon":null,"owner_id":"2","channels":[{"id":"100","type":0,"name":"general"}],
               "roles":[{"id":"10","name":"@everyone","permissions":"1024"}],"emojis":[]},
              {"id":"11","unavailable":true}],
            "merged_members":[[{"user_id":"1","roles":["5"],"nick":"Me!"}],[]],
            "private_channels":[{"id":"20","type":1,"last_message_id":"99","recipient_ids":["2"]}],
            "users":[{"id":"2","username":"friend","discriminator":"0"}],
            "read_state":{"version":3,"partial":false,"entries":[
              {"id":"20","last_message_id":"98","mention_count":2},
              {"id":"10","read_state_type":1,"last_acked_id":"5","badge_count":1}]},
            "user_guild_settings":{"version":4,"partial":false,"entries":[
              {"guild_id":null,"muted":false,"message_notifications":0,"channel_overrides":[]}]},
            "user_settings":{"guild_folders":[{"id":null,"guild_ids":["11"]},{"id":7,"guild_ids":["10"]}]},
            "sessions":[]}}"#,
        );
        assert_eq!(r.session_id, "sess");
        assert_eq!(r.resume_url.as_deref(), Some("wss://resume.example"));
        let d = r.data;
        assert_eq!(d.user.id, Id(1));
        // Folder order: 11 before 10.
        assert_eq!(d.guilds.iter().map(|g| g.id).collect::<Vec<_>>(), [Id(11), Id(10)]);
        let g = &d.guilds[1];
        assert_eq!((g.name.as_str(), g.owner_id), ("G", Id(2)));
        assert_eq!(g.channels[0].guild_id, Some(Id(10)));
        assert_eq!(g.roles[0].permissions.0, 1024);
        assert!(d.guilds[0].unavailable);
        assert_eq!(d.my_members.len(), 1);
        assert_eq!(d.my_members[0].0, Id(10));
        assert_eq!(d.my_members[0].1.nick.as_deref(), Some("Me!"));
        assert_eq!(d.my_members[0].1.roles, [Id(5)]);
        assert_eq!(d.private_channels[0].recipient_ids, [Id(2)]);
        assert_eq!(d.users.len(), 1);
        assert_eq!(d.read_states.len(), 1);
        assert_eq!((d.read_states[0].id, d.read_states[0].mention_count), (Id(20), 2));
        assert_eq!(d.guild_settings.len(), 1);
        assert_eq!(d.guild_settings[0].guild_id, None);
    }

    #[test]
    fn ready_nested_properties_full_recipients_plain_arrays() {
        let r = ready_of(
            r#"{"t":"READY","s":1,"op":0,"d":{"user":{"id":"1","username":"me"},"session_id":"s2",
            "guilds":[{"id":"10","data_mode":"full",
              "properties":{"id":"10","name":"Nested","owner_id":"1","icon":"abc","default_message_notifications":1,"premium_tier":2,"mfa_level":1,"rules_channel_id":"100"},
              "channels":[{"id":"100","type":0}],"roles":[],"emojis":[{"id":"9","name":"e"}],
              "members":[{"user":{"id":"3","username":"x"},"roles":[]},{"user":{"id":"1","username":"me"},"roles":["7"]}]}],
            "private_channels":[
              {"id":"20","type":3,"name":"grp","recipients":[{"id":"2","username":"a"},{"id":"3","username":"b"}]},
              {"id":"21","type":1,"recipients":[{"id":"2","username":"a"}]}],
            "read_state":[{"id":"20","last_message_id":"5"}],
            "user_guild_settings":[{"guild_id":"10","muted":true,"message_notifications":null}]}}"#,
        );
        assert_eq!(r.resume_url, None);
        let d = r.data;
        let g = &d.guilds[0];
        assert_eq!((g.id, g.name.as_str(), g.owner_id, g.icon.as_deref()), (Id(10), "Nested", Id(1), Some("abc")));
        assert_eq!((g.default_message_notifications, g.premium_tier, g.mfa_level), (1, 2, 1));
        assert_eq!(g.rules_channel_id, Some(Id(100)));
        assert_eq!(g.emojis.len(), 1);
        assert_eq!(g.channels[0].guild_id, Some(Id(10)));
        assert_eq!(d.my_members.len(), 1);
        assert_eq!(d.my_members[0].1.roles, [Id(7)]);
        assert_eq!(d.private_channels[0].recipient_ids, [Id(2), Id(3)]);
        assert!(d.private_channels[0].recipients.is_empty());
        assert_eq!(d.private_channels[1].recipient_ids, [Id(2)]);
        assert_eq!(d.users.iter().map(|u| u.id).collect::<Vec<_>>(), [Id(2), Id(3)]);
        assert_eq!(d.read_states.len(), 1);
        assert_eq!(d.guild_settings[0].guild_id, Some(Id(10)));
        assert!(d.guild_settings[0].muted);
        assert_eq!(d.guild_settings[0].message_notifications, 3);
    }

    #[test]
    fn member_list_ops() {
        let json = r#"{"op":0,"t":"GUILD_MEMBER_LIST_UPDATE","s":5,"d":{"guild_id":"10","id":"everyone",
            "member_count":3,"online_count":2,"groups":[{"id":"online","count":2}],
            "ops":[
              {"op":"SYNC","range":[0,99],"items":[
                {"group":{"id":"online","count":2}},
                {"member":{"user":{"id":"1","username":"a"},"roles":[],"nick":"A","presence":{"status":"dnd"}}},
                {"group":{"id":"offline"}}]},
              {"op":"INSERT","index":2,"item":{"member":{"user":{"id":"2"},"roles":["5"],"presence":{"status":"idle"}}}},
              {"op":"UPDATE","index":1,"item":{"member":{"user":{"id":"1"},"presence":null}}},
              {"op":"DELETE","index":3},
              {"op":"INVALIDATE","range":[100,199]},
              {"op":"SOMETHING_NEW","foo":1}]}}"#;
        let evs = dispatch("GUILD_MEMBER_LIST_UPDATE", json.as_bytes(), Id(1)).unwrap();
        let [GatewayEvent::MemberListUpdate(u), GatewayEvent::PresenceUpdate(p)] = evs.as_slice() else { panic!("{evs:?}") };
        // Every member item reports its presence; `null` = offline (state forgets the user).
        let statuses: Vec<(Id, Presence)> = p.iter().map(|(id, p)| (*id, p.status)).collect();
        assert_eq!(statuses, [(Id(1), Presence::Dnd), (Id(2), Presence::Idle), (Id(1), Presence::Offline)]);
        assert_eq!((u.guild_id, u.list_id.as_str(), u.member_count, u.online_count), (Id(10), "everyone", 3, 2));
        assert_eq!(u.ops.len(), 5);
        let MemberListOp::Sync { range, items } = &u.ops[0] else { panic!() };
        assert_eq!(*range, (0, 99));
        assert!(matches!(&items[0], MemberListItem::Group { id, count: 2 } if id == "online"));
        let MemberListItem::Member { member, status } = &items[1] else { panic!() };
        assert_eq!((member.nick.as_deref(), *status), (Some("A"), Presence::Dnd));
        assert_eq!(member.user.as_ref().unwrap().id, Id(1));
        assert!(matches!(&items[2], MemberListItem::Group { id, count: 0 } if id == "offline"));
        assert!(matches!(&u.ops[1], MemberListOp::Insert { index: 2, item: MemberListItem::Member { status: Presence::Idle, .. } }));
        assert!(matches!(&u.ops[2], MemberListOp::Update { index: 1, item: MemberListItem::Member { status: Presence::Offline, .. } }));
        assert!(matches!(&u.ops[3], MemberListOp::Delete { index: 3 }));
        assert!(matches!(&u.ops[4], MemberListOp::Invalidate { range: (100, 199) }));
        assert_eq!(u.groups, [("online".to_string(), 2)]);
    }

    #[test]
    fn small_dispatches() {
        let ev = |t: &str, d: &str| dispatch(t, format!(r#"{{"op":0,"t":"{t}","d":{d}}}"#).as_bytes(), Id(1)).unwrap();

        let r = ev("MESSAGE_REACTION_ADD", r#"{"user_id":"2","channel_id":"3","message_id":"4","emoji":{"id":null,"name":"x"},"type":1}"#);
        assert!(matches!(&r[..], [GatewayEvent::ReactionAdd { burst: true, user_id: Id(2), .. }]));
        let r = ev("MESSAGE_REACTION_REMOVE", r#"{"user_id":"2","channel_id":"3","message_id":"4","emoji":{"id":"5","name":null},"burst":false}"#);
        assert!(matches!(&r[..], [GatewayEvent::ReactionRemove { burst: false, .. }]));

        let r = ev("MESSAGE_POLL_VOTE_ADD", r#"{"user_id":"2","channel_id":"3","message_id":"4","answer_id":2}"#);
        assert!(matches!(&r[..], [GatewayEvent::PollVote { channel_id: Id(3), message_id: Id(4), user_id: Id(2), answer_id: 2, add: true }]));
        let r = ev("MESSAGE_POLL_VOTE_REMOVE", r#"{"user_id":"2","channel_id":"3","message_id":"4","answer_id":1,"guild_id":"9"}"#);
        assert!(matches!(&r[..], [GatewayEvent::PollVote { answer_id: 1, add: false, .. }]));

        // Someone else's member update is dropped, mine is kept.
        assert!(ev("GUILD_MEMBER_UPDATE", r#"{"guild_id":"10","user":{"id":"2"},"roles":[]}"#).is_empty());
        let r = ev("GUILD_MEMBER_UPDATE", r#"{"guild_id":"10","user":{"id":"1"},"roles":["3"],"nick":null}"#);
        assert!(matches!(&r[..], [GatewayEvent::MyMemberUpdate { guild_id: Id(10), member }] if member.roles == [Id(3)]));

        let r = ev("GUILD_DELETE", r#"{"id":"10"}"#);
        assert!(matches!(&r[..], [GatewayEvent::GuildDelete { id: Id(10), unavailable: false }]));
        let r = ev("MESSAGE_ACK", r#"{"channel_id":"3","message_id":"4","version":1,"mention_count":null}"#);
        assert!(matches!(&r[..], [GatewayEvent::MessageAck { mention_count: None, .. }]));
        let r = ev("MESSAGE_UPDATE", r#"{"id":"4","channel_id":"3","embeds":[]}"#);
        assert!(matches!(&r[..], [GatewayEvent::MessageUpdate(u)] if u.content.is_none() && u.embeds.is_some()));

        let r = ev("GUILD_CREATE", r#"{"id":"10","name":"G","channels":[{"id":"1","type":0}],"members":[{"user":{"id":"1"},"roles":["9"]}]}"#);
        assert!(matches!(&r[..], [GatewayEvent::GuildCreate(g), GatewayEvent::MyMemberUpdate { .. }] if g.channels[0].guild_id == Some(Id(10))));

        let r = ev("CHANNEL_UNREAD_UPDATE", r#"{"guild_id":"10","channel_unread_updates":[{"id":"3","last_message_id":"9","last_pin_timestamp":null},{"id":"4","last_message_id":null}]}"#);
        assert!(matches!(&r[..], [GatewayEvent::ChannelLastMessage { updates }] if *updates == [(Id(3), Some(Id(9))), (Id(4), None)]));
        let r = ev("RELATIONSHIP_ADD", r#"{"id":"2","type":2,"user":{"id":"2"}}"#);
        assert!(matches!(&r[..], [GatewayEvent::RelationshipUpsert(rel)] if rel.hides_author()));
        let r = ev("RELATIONSHIP_REMOVE", r#"{"id":"2","type":2}"#);
        assert!(matches!(&r[..], [GatewayEvent::RelationshipRemove { id: Id(2) }]));
        let r = ev("GUILD_MEMBER_UPDATE", r#"{"guild_id":"10","user":{"id":"1"},"roles":[],"communication_disabled_until":"2099-01-01T00:00:00+00:00"}"#);
        assert!(matches!(&r[..], [GatewayEvent::MyMemberUpdate { member, .. }] if member.timed_out()));

        assert!(ev("SESSIONS_REPLACE", "[]").is_empty());

        // Voice.
        let r = ev("VOICE_STATE_UPDATE", r#"{"guild_id":"10","channel_id":"5","user_id":"2","session_id":"s","self_mute":true,"deaf":false}"#);
        assert!(matches!(&r[..], [GatewayEvent::VoiceStateUpdate(v)] if v.channel_id == Some(Id(5)) && v.self_mute));
        let r = ev("VOICE_SERVER_UPDATE", r#"{"token":"VOICE_TOKEN","guild_id":"10","endpoint":"smart.loyal.discord.media:443"}"#);
        let [GatewayEvent::VoiceServerUpdate { guild_id, channel_id, endpoint, token }] = &r[..] else { panic!() };
        assert_eq!((*guild_id, *channel_id, endpoint.as_deref()), (Some(Id(10)), None, Some("smart.loyal.discord.media:443")));
        assert_eq!(token.expose(), "VOICE_TOKEN");
        assert!(!format!("{r:?}").contains("VOICE_TOKEN"), "token redacted in Debug");
        let r = ev("VOICE_SERVER_UPDATE", r#"{"token":"x","guild_id":null,"channel_id":"7","endpoint":null}"#);
        assert!(matches!(&r[..], [GatewayEvent::VoiceServerUpdate { endpoint: None, channel_id: Some(Id(7)), .. }]));
        let r = ev(
            "READY_SUPPLEMENTAL",
            r#"{"merged_members":[[]],"merged_presences":{"friends":[],"guilds":[]},"guilds":[
              {"id":"10","voice_states":[{"channel_id":"5","user_id":"2","session_id":"a"},{"channel_id":"6","user_id":"3"}]},
              {"id":"11"}]}"#,
        );
        assert_eq!(r.len(), 2);
        assert!(r.iter().all(|e| matches!(e, GatewayEvent::VoiceStateUpdate(v) if v.guild_id == Some(Id(10)))));
        let r = ev("GUILD_CREATE", r#"{"id":"10","voice_states":[{"channel_id":"5","user_id":"2"}]}"#);
        assert!(matches!(&r[..], [GatewayEvent::GuildCreate(g)] if g.voice_states[0].guild_id == Some(Id(10))));
        assert!(dispatch("MESSAGE_CREATE", br#"{"op":0,"d":{"id":[1]}}"#, Id(1)).is_err());
    }

    fn ev(t: &str, d: &str) -> Vec<GatewayEvent> {
        dispatch(t, format!(r#"{{"op":0,"t":"{t}","d":{d}}}"#).as_bytes(), Id(1)).unwrap()
    }

    #[test]
    fn guild_marks_from_properties_and_flat_guilds() {
        // CLIENT_STATE_V2: features and tier in `properties`, the boost count at the top level.
        let r = ready_of(
            r#"{"t":"READY","s":1,"op":0,"d":{"user":{"id":"1","username":"me"},"session_id":"s",
            "guilds":[{"id":"10","premium_subscription_count":9,
              "properties":{"id":"10","name":"N","features":["VERIFIED","NEWS","COMMUNITY"],"premium_tier":2}}]}}"#,
        );
        let g = &r.data.guilds[0];
        assert_eq!((g.features.as_slice(), g.premium_tier, g.premium_subscription_count), (&["VERIFIED".to_owned()][..], 2, 9));
        let out = ev("GUILD_UPDATE", r#"{"id":"10","name":"N","features":["PARTNERED","BANNER"],"premium_tier":1,"premium_subscription_count":3}"#);
        let [GatewayEvent::GuildUpdate(g)] = &out[..] else { panic!("{out:?}") };
        assert_eq!((g.features.as_slice(), g.premium_tier, g.premium_subscription_count), (&["PARTNERED".to_owned()][..], 1, 3));
        let out = ev("GUILD_UPDATE", r#"{"id":"10","name":"N","features":null,"premium_subscription_count":null}"#);
        let [GatewayEvent::GuildUpdate(g)] = &out[..] else { panic!("{out:?}") };
        assert!(g.features.is_empty() && g.premium_subscription_count == 0);
    }

    fn wrap(d: &str) -> Vec<u8> {
        format!(r#"{{"op":0,"d":{d}}}"#).into_bytes()
    }

    #[test]
    fn typing_and_pins() {
        let r = ev(
            "TYPING_START",
            r#"{"channel_id":"3","guild_id":"10","user_id":"2","timestamp":1700000000,
            "member":{"user":{"id":"2","username":"bob"},"nick":"Bobby","roles":[]}}"#,
        );
        let [GatewayEvent::TypingStart { channel_id: Id(3), guild_id: Some(Id(10)), user_id: Id(2), timestamp: 1_700_000_000, member: Some(m) }] =
            &r[..]
        else {
            panic!("{r:?}")
        };
        assert_eq!((m.nick.as_deref(), m.user.as_ref().map(|u| u.username.as_str())), (Some("Bobby"), Some("bob")));
        let r = ev("TYPING_START", r#"{"channel_id":"20","user_id":"2","timestamp":"1700000000"}"#);
        assert!(matches!(&r[..], [GatewayEvent::TypingStart { guild_id: None, member: None, .. }]), "DM: no guild, no member");

        let r = ev("CHANNEL_PINS_UPDATE", r#"{"guild_id":"10","channel_id":"3","last_pin_timestamp":"2026-11-12T10:00:00+00:00"}"#);
        assert!(matches!(&r[..], [GatewayEvent::ChannelPinsUpdate { channel_id: Id(3), guild_id: Some(Id(10)), last_pin_timestamp: Some(t) }] if t.starts_with("2026")));
        let r = ev("CHANNEL_PINS_UPDATE", r#"{"channel_id":"20","last_pin_timestamp":null}"#);
        assert!(matches!(&r[..], [GatewayEvent::ChannelPinsUpdate { guild_id: None, last_pin_timestamp: None, .. }]));
    }

    #[test]
    fn presences() {
        let r = ev(
            "PRESENCE_UPDATE",
            r#"{"user":{"id":"2"},"guild_id":"10","status":"dnd","client_status":{"mobile":"dnd"},"activities":[
              {"type":"weird","name":"skipped"},
              {"type":4,"name":"Custom Status","state":"brb","emoji":{"name":"🍕"}},
              {"type":6,"name":"Hang Status"},
              {"type":2,"name":"Spotify","details":"x","timestamps":{"start":"1700000000000","end":1700000180000},"application_id":7,
               "assets":{"large_image":"spotify:ab","large_text":"","small_image":5,"small_text":"s"},"sync_id":"4uLU6hMCjMI75M1A2tKUQC",
               "party":{"id":"spotify:2","size":[1,"x"]},"url":3},
              {"type":0,"name":"Chess"}]}"#,
        );
        let [GatewayEvent::PresenceUpdate(list)] = &r[..] else { panic!("{r:?}") };
        let (id, p) = &list[0];
        assert_eq!((*id, p.status, p.mobile), (Id(2), Presence::Dnd, true));
        let custom = p.custom_status.as_ref().unwrap();
        assert_eq!((custom.text.as_deref(), custom.emoji.as_ref().and_then(|e| e.name.as_deref())), (Some("brb"), Some("🍕")));
        let rich = RichPresence {
            details: Some("x".into()),
            start: Some(1_700_000_000_000),
            end: Some(1_700_000_180_000),
            application_id: Some(Id(7)),
            large_image: Some("spotify:ab".into()),
            small_text: Some("s".into()),
            sync_id: Some("4uLU6hMCjMI75M1A2tKUQC".into()),
            ..Default::default()
        };
        let spotify = Activity { kind: 2, name: "Spotify".into(), rich: Some(Box::new(rich)) };
        assert_eq!(p.activity, Some(spotify), "first non-custom activity; type 6 skipped; odd asset types dropped");

        // Lenient: odd field types, desktop + mobile is not "mobile", invisible is offline.
        let r = ev("PRESENCE_UPDATE", r#"{"user":{"id":"3"},"status":5,"activities":null,"client_status":{"desktop":"online","mobile":"online"}}"#);
        assert!(matches!(&r[..], [GatewayEvent::PresenceUpdate(l)] if l[0].1 == UserPresence::default()));
        let r = ev("PRESENCE_UPDATE", r#"{"user":{"id":"3"},"status":"online","client_status":{"desktop":"online","mobile":"online"},"activities":[{"type":4,"name":"Custom Status"}]}"#);
        assert!(matches!(&r[..], [GatewayEvent::PresenceUpdate(l)] if !l[0].1.mobile && l[0].1.custom_status.is_none()));
        assert!(ev("PRESENCE_UPDATE", r#"{"user":{},"status":"online"}"#).is_empty(), "no user id");

        // READY_SUPPLEMENTAL: friends + every guild, next to the voice states.
        let r = ev(
            "READY_SUPPLEMENTAL",
            r#"{"guilds":[{"id":"10","voice_states":[{"channel_id":"5","user_id":"2"}]}],
            "merged_presences":{"friends":[{"user_id":"2","status":"online","last_modified":1,"activities":[],"client_status":{"desktop":"online"}}],
              "guilds":[[{"user_id":"3","status":"idle","activities":[{"type":3,"name":"TV"}]},{"user_id":"4","status":"offline"}],[]]}}"#,
        );
        let [GatewayEvent::VoiceStateUpdate(_), GatewayEvent::PresenceUpdate(list)] = &r[..] else { panic!("{r:?}") };
        let got: Vec<(Id, Presence)> = list.iter().map(|(id, p)| (*id, p.status)).collect();
        assert_eq!(got, [(Id(2), Presence::Online), (Id(3), Presence::Idle), (Id(4), Presence::Offline)]);
        assert_eq!(list[1].1.activity.as_ref().unwrap().label(), "Watching TV");
        assert_eq!(list[1].1.activity.as_ref().unwrap().rich, None, "no rich presence: nothing boxed");
        assert_eq!(ev("READY_SUPPLEMENTAL", r#"{"guilds":[],"merged_presences":null}"#).len(), 0);

        // Stream URL and party size.
        let r = ev(
            "PRESENCE_UPDATE",
            r#"{"user":{"id":"5"},"status":"online","activities":[{"type":1,"name":"Twitch","url":"https://www.twitch.tv/x","party":{"size":[3,4]}}]}"#,
        );
        let [GatewayEvent::PresenceUpdate(list)] = &r[..] else { panic!("{r:?}") };
        let rich = list[0].1.activity.as_ref().unwrap().rich.as_deref().unwrap();
        assert_eq!((rich.url.as_deref(), rich.party), (Some("https://www.twitch.tv/x"), Some([3, 4])));
    }

    #[test]
    fn my_status_sources() {
        // READY user_settings wins; an expired custom status is none.
        let r = ready_of(
            r#"{"op":0,"t":"READY","d":{"user":{"id":"1"},"session_id":"s",
            "user_settings":{"status":"dnd","custom_status":{"text":"away","emoji_id":null,"emoji_name":"🍕","expires_at":"2999-01-01T00:00:00.000Z"}},
            "sessions":[{"session_id":"all","status":"online","activities":[]}]}}"#,
        );
        let c = r.status.custom.as_ref().unwrap().as_ref().unwrap();
        assert_eq!(r.status.status, Some(UserStatus::Dnd));
        assert!(r.status.settings);
        assert_eq!(r.status.expires, Some(chrono::DateTime::parse_from_rfc3339("2999-01-01T00:00:00Z").unwrap().to_utc()));
        assert_eq!((c.text.as_deref(), c.emoji.as_ref().unwrap().name.as_deref()), (Some("away"), Some("🍕")));
        let r = ready_of(
            r#"{"op":0,"t":"READY","d":{"user":{"id":"1"},"session_id":"s",
            "user_settings":{"status":"idle","custom_status":{"text":"old","expires_at":"2020-01-01T00:00:00Z"}}}}"#,
        );
        assert_eq!(r.status, StatusPatch { status: Some(UserStatus::Idle), custom: Some(None), expires: None, settings: true });
        let r = ready_of(r#"{"op":0,"t":"READY","d":{"user":{"id":"1"},"session_id":"s","user_settings":{"status":"online"}}}"#);
        assert_eq!(r.status.custom, Some(None), "READY is complete");
        // No JSON settings: this session from `sessions` (no `all` entry).
        let r = ready_of(
            r#"{"op":0,"t":"READY","d":{"user":{"id":"1"},"session_id":"s",
            "sessions":[{"session_id":"x","status":"online"},{"session_id":"s","status":"invisible","activities":[{"type":4,"state":"hi","name":"Custom Status"}]}]}}"#,
        );
        assert_eq!(r.status.status, Some(UserStatus::Invisible));
        assert!(!r.status.settings);
        assert_eq!(r.status.custom.unwrap().unwrap().text.as_deref(), Some("hi"));

        // USER_SETTINGS_UPDATE is partial.
        let p = settings_update(&wrap(r#"{"status":"idle"}"#)).unwrap();
        assert_eq!(p, StatusPatch { status: Some(UserStatus::Idle), settings: true, ..Default::default() });
        let p = settings_update(&wrap(r#"{"custom_status":null}"#)).unwrap();
        assert_eq!(p, StatusPatch { custom: Some(None), settings: true, ..Default::default() });
        let p = settings_update(&wrap(r#"{"custom_status":{"text":"","emoji_id":"9","emoji_name":"pepe"}}"#)).unwrap();
        assert_eq!(p.custom.unwrap().unwrap().emoji.unwrap().id, Some(Id(9)));
        let p = settings_update(&wrap(r#"{"guild_folders":[]}"#)).unwrap();
        assert!(p.status.is_none() && p.custom.is_none());

        // SESSIONS_REPLACE: `all` first, then mine; offline/unknown keep the status.
        let sessions = r#"[{"session_id":"mine","status":"dnd","activities":[]},{"session_id":"all","status":"idle","activities":[{"type":4,"name":"Custom Status","state":"zz"}]}]"#;
        let p = sessions_replace(&wrap(sessions), "mine").unwrap();
        assert_eq!((p.status, p.custom.unwrap().unwrap().text.as_deref()), (Some(UserStatus::Idle), Some("zz")));
        let p = sessions_replace(&wrap(r#"[{"session_id":"mine","status":"offline"}]"#), "mine").unwrap();
        assert_eq!(p, StatusPatch { custom: Some(None), ..Default::default() });
        assert_eq!(sessions_replace(&wrap(r#"[{"session_id":"other","status":"dnd"}]"#), "mine").unwrap(), StatusPatch::default());
        // My presence: the `all` session's game and custom status; none without `all`.
        let game = r#"[{"session_id":"all","status":"online","activities":[{"type":0,"name":"Factorio"},{"type":4,"name":"Custom Status","state":"zz"}]}]"#;
        let me = my_presence(&wrap(game)).unwrap().unwrap();
        assert_eq!((me.status, me.activity.unwrap().name.as_str(), me.custom_status.unwrap().text.as_deref()), (Presence::Online, "Factorio", Some("zz")));
        assert!(my_presence(&wrap(r#"[{"session_id":"mine","status":"dnd"}]"#)).unwrap().is_none());
    }

    #[test]
    fn members_chunk_and_rate_limit() {
        let c = members_chunk(&wrap(
            r#"{"guild_id":"10","members":[{"user":{"id":"2","username":"ab"},"nick":null,"roles":["5"]}],
            "chunk_index":0,"chunk_count":1,"not_found":[123],"nonce":"rc1"}"#,
        ))
        .unwrap();
        assert_eq!((c.members.len(), c.chunk_index, c.chunk_count, c.nonce.as_deref()), (1, 0, 1, Some("rc1")));
        assert_eq!(c.members[0].user.as_ref().unwrap().id, Id(2));
        let r = rate_limited(&wrap(r#"{"opcode":8,"retry_after":2.5,"meta":{"guild_id":"10","nonce":"rc1"}}"#)).unwrap();
        assert_eq!((r.opcode, r.retry_after, r.meta.nonce.as_deref()), (8, 2.5, Some("rc1")));
    }
}
