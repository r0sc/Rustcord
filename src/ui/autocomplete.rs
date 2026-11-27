//! Composer autocomplete (FEATURES_NOTES §1.6, §1.7): `@` members (local first, then op 8
//! answers), roles and @everyone/@here when I may ping them; `#` channels of the guild in sidebar
//! order; `:` emoji (≥ 2 chars, the guild's own custom emoji first). Rows insert readable text
//! ("@Alice ", "#general ", ":blob: "); the composer keeps a per-draft side map from that text to
//! the wire syntax and `expand`s it on send, outside code. The popup itself is the composer's.
//! OWNERSHIP: composer engineer.

use super::icons::{self, Badge, Icon as Vector};
use super::pickers;
use super::rgb;
use crate::emoji;
use crate::model::{channel_type, perm, CustomEmoji, Id, Member, MemberListItem, User};
use crate::state::{AppState, SidebarItem};
use egui::Color32;
use std::collections::{HashMap, HashSet};
use std::ops::Range;

/// Rows shown at most.
pub const MAX_ROWS: usize = 10;
/// Op 8 `limit` for `@` searches.
pub const SEARCH_LIMIT: u8 = 25;

/// What the word before the cursor asks for (the text after the trigger character).
#[derive(Debug, PartialEq, Eq)]
pub enum Trigger<'a> {
    Member(&'a str),
    Channel(&'a str),
    Emoji(&'a str),
}

pub fn trigger(word: &str) -> Option<Trigger<'_>> {
    let rest = word.get(1..)?;
    match word.as_bytes().first()? {
        b'@' => Some(Trigger::Member(rest)),
        b'#' => Some(Trigger::Channel(rest)),
        b':' if rest.chars().count() >= 2 && rest.chars().all(|c| c.is_ascii_alphanumeric() || "_+-".contains(c)) => {
            Some(Trigger::Emoji(rest))
        }
        _ => None,
    }
}

/// The popup's header line ("Members", "Channels", "Emoji matching :sm").
pub fn title(word: &str) -> String {
    match trigger(word) {
        Some(Trigger::Member(_)) => "Members".into(),
        Some(Trigger::Channel(_)) => "Channels".into(),
        Some(Trigger::Emoji(q)) => format!("Emoji matching :{q}"),
        None => String::new(),
    }
}

/// What leads a row.
#[derive(Clone)]
pub enum Icon {
    /// 24 px avatar with the status dot.
    User(Box<(User, Option<Member>)>),
    /// Channel kind (`icons::of`) or `At` for roles and @everyone.
    Vector((Vector, Option<Badge>)),
    /// A unicode emoji (20 px).
    Emoji(String),
    /// A custom emoji (the composer loads its texture).
    Custom(CustomEmoji),
}

/// One popup row.
#[derive(Clone)]
pub struct Row {
    /// Readable text that replaces the word, with a trailing space.
    pub insert: String,
    /// What `insert` (trimmed) stands for on the wire, when that differs (`<@id>`, `<#id>`, …).
    pub wire: Option<String>,
    pub icon: Icon,
    pub label: String,
    /// Role color of the label.
    pub color: Option<Color32>,
    /// Muted text on the right (username, category, server, "Role").
    pub detail: String,
}

impl Row {
    fn new(insert: String, wire: Option<String>, icon: Icon, label: String) -> Self {
        Self { insert, wire, icon, label, color: None, detail: String::new() }
    }

    fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = detail.into();
        self
    }
}

/// Rows for `word`. `glyph(e)` says whether the emoji font can draw a unicode emoji.
pub fn suggest(state: &AppState, ch: Id, word: &str, glyph: &dyn Fn(&str) -> bool) -> Vec<Row> {
    let guild_id = state.channels.get(&ch).and_then(|c| c.guild_id);
    match trigger(word) {
        Some(Trigger::Member(q)) => members(state, ch, guild_id, &q.to_lowercase()),
        Some(Trigger::Channel(q)) => guild_id.map(|g| channels(state, g, &q.to_lowercase())).unwrap_or_default(),
        Some(Trigger::Emoji(q)) => emojis(state, ch, &q.to_lowercase(), glyph),
        None => Vec::new(),
    }
}

/// Prefix match on any of the names (`q` lowercased; empty matches all).
fn name_match(q: &str, names: &[&str]) -> bool {
    names.iter().any(|n| n.to_lowercase().starts_with(q))
}

fn members(state: &AppState, ch: Id, guild_id: Option<Id>, q: &str) -> Vec<Row> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut add = |out: &mut Vec<Row>, user: &User, member: Option<&Member>| {
        if out.len() >= MAX_ROWS || !seen.insert(user.id) {
            return;
        }
        // The row shows my local name; what goes into the text is the real one (the wire form is the
        // id), so a local name never reaches a sent message.
        let shown = state.member_display_name(guild_id, user, member);
        let real = state.real_display_name(guild_id, user, member);
        let nick = member.and_then(|m| m.nick.as_deref()).unwrap_or_default();
        if !name_match(q, &[&shown, &real, nick, user.global_name.as_deref().unwrap_or_default(), &user.username]) {
            return;
        }
        let icon = Icon::User(Box::new((user.clone(), member.cloned())));
        let mut row = Row::new(format!("@{real} "), Some(format!("<@{}>", user.id.0)), icon, shown).detail(user.username.clone());
        row.color = guild_id.zip(member).and_then(|(g, m)| state.role_color(g, &m.roles)).map(rgb);
        out.push(row);
    };
    // Recent authors here first (newest first), like the web client.
    if let Some(cm) = state.messages.get(&ch) {
        for m in cm.msgs.iter().rev().filter(|m| !m.author.bot || m.member.is_some()) {
            add(&mut out, &m.author, m.member.as_ref());
        }
    }
    match guild_id {
        None => {
            let recipients = state.channels.get(&ch).map(|c| c.recipient_ids.clone()).unwrap_or_default();
            for u in recipients.iter().filter_map(|id| state.users.get(id)).chain(state.me.as_ref()) {
                add(&mut out, u, None);
            }
        }
        Some(g) => {
            if let Some(me) = &state.me {
                add(&mut out, me, state.my_members.get(&g));
            }
            let listed = state.member_lists.get(&g).into_iter().flat_map(|l| &l.items).filter_map(|it| match it {
                MemberListItem::Member { member, .. } => Some(member),
                _ => None,
            });
            // Op 8 answers, newest first.
            let found = state.chat.member_search.iter().rev().filter(|s| s.guild_id == g).flat_map(|s| &s.members);
            for m in listed.chain(found) {
                if let Some(u) = &m.user {
                    add(&mut out, u, Some(m));
                }
            }
            let ping_all = state.channel_permissions(ch) & perm::MENTION_EVERYONE != 0;
            if ping_all {
                for (name, what) in [("everyone", "All members"), ("here", "Online members")] {
                    if out.len() < MAX_ROWS && name.starts_with(q) {
                        out.push(Row::new(format!("@{name} "), None, Icon::Vector((Vector::At, None)), format!("@{name}")).detail(what));
                    }
                }
            }
            let roles = state.guild(g).map(|g| g.roles.as_slice()).unwrap_or_default();
            for r in roles.iter().filter(|r| r.id != g && (ping_all || r.mentionable) && r.name.to_lowercase().starts_with(q)) {
                if out.len() >= MAX_ROWS {
                    break;
                }
                let mut row = Row::new(format!("@{} ", r.name), Some(format!("<@&{}>", r.id.0)), Icon::Vector((Vector::At, None)), format!("@{}", r.name)).detail("Role");
                row.color = (r.color != 0).then(|| rgb(r.color));
                out.push(row);
            }
        }
    }
    out
}

/// Viewable channels of the guild: prefix matches first, then names containing `q`, each in
/// sidebar order (threads after the channels).
fn channels(state: &AppState, g: Id, q: &str) -> Vec<Row> {
    use channel_type::*;
    let order: HashMap<Id, usize> = state
        .sidebar_channels(g)
        .into_iter()
        .filter_map(|it| match it {
            SidebarItem::Channel { id } => Some(id),
            _ => None,
        })
        .enumerate()
        .map(|(i, id)| (id, i))
        .collect();
    let mut list: Vec<_> = state
        .channels
        .values()
        .filter(|c| c.guild_id == Some(g) && c.kind != GUILD_CATEGORY && c.kind != GUILD_DIRECTORY && state.can_view(c.id))
        .filter_map(|c| Some((c, c.name.as_deref()?.to_lowercase())))
        .filter(|(_, n)| n.contains(q))
        .collect();
    list.sort_by_key(|(c, n)| (!n.starts_with(q), order.get(&c.id).copied().unwrap_or(usize::MAX), c.position, c.id));
    list.into_iter()
        .take(MAX_ROWS)
        .map(|(c, _)| {
            let name = c.name.clone().unwrap_or_default();
            let category = c.parent_id.and_then(|p| state.channels.get(&p)).and_then(|p| p.name.clone()).unwrap_or_default();
            Row::new(format!("#{name} "), Some(format!("<#{}>", c.id.0)), Icon::Vector(icons::of(c, state)), name).detail(category)
        })
        .collect()
}

/// Usable custom emoji (this guild's first) by name prefix, then unicode emoji.
fn emojis(state: &AppState, ch: Id, q: &str, glyph: &dyn Fn(&str) -> bool) -> Vec<Row> {
    let mut out = Vec::new();
    for (_, guild, list) in pickers::custom_emoji(state, ch) {
        for (e, usable) in list {
            if out.len() >= MAX_ROWS / 2 {
                break;
            }
            // Lowercase prefix match without a lowercased copy of every name each frame.
            let mut name = e.name.chars().flat_map(char::to_lowercase);
            if usable && q.chars().all(|c| name.next() == Some(c)) {
                let code = format!(":{}:", e.name);
                out.push(Row::new(format!("{code} "), Some(pickers::custom_text(e)), Icon::Custom(e.clone()), code).detail(guild.to_owned()));
            }
        }
    }
    let room = MAX_ROWS - out.len();
    for (code, e) in emoji::search(q, MAX_ROWS * 2).into_iter().filter(|(_, e)| glyph(e.as_str())).take(room) {
        out.push(Row::new(format!("{} ", e.as_str()), None, Icon::Emoji(e.as_str().to_owned()), format!(":{code}:")));
    }
    out
}

/// `:name:` for a custom emoji's wire text (`<:name:id>` / `<a:name:id>`), else None.
pub fn readable_emoji(wire: &str) -> Option<String> {
    let inner = wire.strip_prefix('<')?.strip_suffix('>')?;
    let mut parts = inner.split(':');
    let (_, name, id) = (parts.next()?, parts.next()?, parts.next()?);
    (!name.is_empty() && id.parse::<u64>().is_ok() && parts.next().is_none()).then(|| format!(":{name}:"))
}

/// `text` with every readable mention of `map` (readable, wire) replaced by its wire form,
/// except in code blocks and inline code. A mention must stand alone (no letter, digit or `_`
/// right before or after it), so edited or partly deleted mentions stay plain text.
pub fn expand(text: &str, map: &[(String, String)]) -> String {
    let mut keys: Vec<&(String, String)> = map.iter().filter(|(r, _)| !r.is_empty()).collect();
    if keys.is_empty() {
        return text.to_owned();
    }
    // Longest first: "@Alice Smith" before "@Alice".
    keys.sort_by_key(|(r, _)| std::cmp::Reverse(r.len()));
    let mut out = String::with_capacity(text.len() + 32);
    let mut last = 0;
    for code in code_spans(text).into_iter().chain(std::iter::once(text.len()..text.len())) {
        expand_plain(&text[last..code.start], &keys, &mut out);
        out.push_str(&text[code.clone()]);
        last = code.end;
    }
    out
}

fn expand_plain(text: &str, keys: &[&(String, String)], out: &mut String) {
    let word = |c: char| c.is_alphanumeric() || c == '_';
    let mut i = 0;
    while let Some(c) = text[i..].chars().next() {
        let rest = &text[i..];
        let alone_before = !text[..i].chars().next_back().is_some_and(word);
        let hit = keys.iter().find(|(r, _)| alone_before && rest.starts_with(r.as_str()) && !rest[r.len()..].chars().next().is_some_and(word));
        match hit {
            Some((r, w)) => {
                out.push_str(w);
                i += r.len();
            }
            None => {
                out.push(c);
                i += c.len_utf8();
            }
        }
    }
}

/// Byte ranges of code in `text`, delimiters included: ``` blocks and `inline` / ``double``
/// spans. Unclosed backticks are plain text, as Discord renders them.
fn code_spans(text: &str) -> Vec<Range<usize>> {
    let b = text.as_bytes();
    let run_at = |i: usize| b[i..].iter().take_while(|&&c| c == b'`').count();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'`' {
            i += 1;
            continue;
        }
        let run = run_at(i);
        let close = if run >= 3 {
            text[i + 3..].find("```").map(|p| i + 3 + p + 3)
        } else {
            // The next run of exactly as many backticks.
            let mut j = i + run;
            loop {
                if j >= b.len() {
                    break None;
                }
                if b[j] != b'`' {
                    j += 1;
                    continue;
                }
                let k = run_at(j);
                if k == run {
                    break Some(j + k);
                }
                j += k;
            }
        };
        match close {
            Some(end) => {
                out.push(i..end);
                i = end;
            }
            None => i += run,
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Channel, Guild, Message, Role};

    #[test]
    fn triggers() {
        assert_eq!(trigger("@al"), Some(Trigger::Member("al")));
        assert_eq!(trigger("@"), Some(Trigger::Member("")));
        assert_eq!(trigger("#gen"), Some(Trigger::Channel("gen")));
        assert_eq!(trigger(":sm"), Some(Trigger::Emoji("sm")));
        assert_eq!(trigger(":s"), None, "emoji need 2 chars");
        assert_eq!(trigger(":)"), None);
        assert_eq!(trigger("http://x"), None);
        assert_eq!(trigger(""), None);
        assert_eq!(trigger("é"), None);
        assert_eq!(title(":sm"), "Emoji matching :sm");
    }

    fn state() -> (AppState, Id) {
        let me = User { id: Id(1), username: "ferris".into(), ..Default::default() };
        let alice = User { id: Id(2), username: "alice".into(), global_name: Some("Alice".into()), ..Default::default() };
        let mut s = AppState::default();
        s.me = Some(me.clone());
        let (g, ch) = (Id(10), Id(100));
        let role = Role { id: Id(11), name: "Mods".into(), permissions: crate::model::Perms(perm::ALL), ..Default::default() };
        let everyone = Role { id: g, name: "@everyone".into(), permissions: crate::model::Perms(perm::ALL), ..Default::default() };
        s.guilds.push(Guild { id: g, owner_id: Id(1), roles: vec![everyone, role], ..Default::default() });
        s.channels.insert(ch, Channel { id: ch, guild_id: Some(g), name: Some("general".into()), ..Default::default() });
        s.channels.insert(Id(101), Channel { id: Id(101), guild_id: Some(g), name: Some("help".into()), position: 1, ..Default::default() });
        // A voice channel at position 0 of its own category still sorts after the text channels.
        let voice = Channel { id: Id(102), guild_id: Some(g), name: Some("hangout".into()), kind: channel_type::GUILD_VOICE, ..Default::default() };
        s.channels.insert(Id(102), voice);
        let msgs = vec![Message { id: Id(5), channel_id: ch, author: alice, ..Default::default() }];
        s.messages.insert(ch, crate::state::ChannelMessages { msgs, ..Default::default() });
        (s, ch)
    }

    /// (readable insert, wire) of each row.
    fn rows(s: &AppState, ch: Id, word: &str) -> Vec<(String, Option<String>)> {
        suggest(s, ch, word, &|_| true).into_iter().map(|r| (r.insert, r.wire)).collect()
    }

    fn pair(insert: &str, wire: Option<&str>) -> (String, Option<String>) {
        (insert.to_owned(), wire.map(str::to_owned))
    }

    #[test]
    fn a_local_name_is_found_but_never_inserted() {
        let (s, ch) = state();
        let list = [crate::names::CustomName { id: Id(2), name: Some("Ali".into()), ..Default::default() }];
        crate::names::with_names(&list, || {
            let found = suggest(&s, ch, "@ali", &|_| true);
            assert_eq!(found.len(), 1);
            assert_eq!((found[0].insert.as_str(), found[0].wire.as_deref(), found[0].label.as_str()), ("@Alice ", Some("<@2>"), "Ali"));
            // The real name still finds them.
            assert_eq!(suggest(&s, ch, "@al", &|_| true).len(), 1);
        });
    }

    #[test]
    fn rows_insert_readable_text() {
        let (mut s, ch) = state();
        assert_eq!(rows(&s, ch, "@al"), [pair("@Alice ", Some("<@2>"))]);
        assert_eq!(rows(&s, ch, "@")[..2], [pair("@Alice ", Some("<@2>")), pair("@ferris ", Some("<@1>"))], "recent authors first");
        assert_eq!(rows(&s, ch, "@ev"), [pair("@everyone ", None)], "owner may ping everyone");
        assert_eq!(rows(&s, ch, "@mo"), [pair("@Mods ", Some("<@&11>"))]);
        assert_eq!(rows(&s, ch, "#he"), [pair("#help ", Some("<#101>"))]);
        let order: Vec<_> = rows(&s, ch, "#").into_iter().map(|(i, _)| i).collect();
        assert_eq!(order, ["#general ", "#help ", "#hangout "], "sidebar order: text before voice");
        assert_eq!(rows(&s, ch, ":thumbsu"), [pair("👍 ", None)]);
        assert!(rows(&s, ch, ":zzzzzz").is_empty());
        // Not the owner, no MENTION_EVERYONE: only mentionable roles, no @everyone.
        s.guilds[0].owner_id = Id(99);
        s.guilds[0].roles[0].permissions = crate::model::Perms(perm::VIEW_CHANNEL | perm::SEND_MESSAGES);
        s.guilds[0].roles.push(Role { id: Id(12), name: "Movie night".into(), mentionable: true, ..Default::default() });
        assert_eq!(rows(&s, ch, "@mo"), [pair("@Movie night ", Some("<@&12>"))]);
        assert!(rows(&s, ch, "@ev").is_empty());
    }

    #[test]
    fn mentions_expand_outside_code() {
        let map: Vec<(String, String)> = [("@Alice", "<@2>"), ("@Alice Smith", "<@3>"), ("#general", "<#100>"), (":blob:", "<:blob:7>")]
            .into_iter()
            .map(|(a, b)| (a.to_owned(), b.to_owned()))
            .collect();
        assert_eq!(expand("hi @Alice, @Alice Smith in #general :blob:", &map), "hi <@2>, <@3> in <#100> <:blob:7>");
        assert_eq!(expand("`@Alice` and ``#general`` @Alice", &map), "`@Alice` and ``#general`` <@2>", "inline code stays");
        assert_eq!(expand("```\n@Alice\n```\n@Alice", &map), "```\n@Alice\n```\n<@2>", "code blocks stay");
        assert_eq!(expand("unclosed `@Alice", &map), "unclosed `<@2>", "a lone backtick is text");
        // Deleted or edited mentions are plain text.
        assert_eq!(expand("@Alic, @Alicex, mail@Alice", &map), "@Alic, @Alicex, mail@Alice");
        assert_eq!(expand("no map", &[]), "no map");
        assert_eq!(readable_emoji("<a:dance:7>").as_deref(), Some(":dance:"));
        assert_eq!(readable_emoji("<@7>"), None);
        assert_eq!(readable_emoji("👍"), None);
    }
}
