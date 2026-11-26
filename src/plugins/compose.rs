//! Composer plugins: TextReplace (PLUGINS_NOTES S30, plain-text rules only) and CharacterCounter
//! (S31) only change what this app shows or sends for the user's own Enter. QuickReply (class A)
//! picks the message to reply to or edit with keys; it sends nothing itself.

use super::{Plugin, enabled, opt_raw};
use crate::model::{Id, Message};
use crate::ui::UiCx;

/// One TextReplace rule; plugins.toml: `[TextReplace] rules = [{ find, replace, case }]`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Rule {
    pub find: String,
    pub replace: String,
    /// Case-sensitive match.
    pub case: bool,
}

/// The saved rules (empty or broken entries are kept as they are; empty `find`s do nothing).
pub fn rules() -> Vec<Rule> {
    let Some(toml::Value::Array(list)) = opt_raw(Plugin::TextReplace, "rules") else { return Vec::new() };
    let s = |t: &toml::Table, k: &str| t.get(k).and_then(toml::Value::as_str).unwrap_or_default().to_owned();
    list.iter()
        .filter_map(toml::Value::as_table)
        .map(|t| Rule { find: s(t, "find"), replace: s(t, "replace"), case: t.get("case").and_then(toml::Value::as_bool).unwrap_or(false) })
        .collect()
}

/// `rules` as stored in plugins.toml.
pub fn rules_value(rules: &[Rule]) -> toml::Value {
    let row = |r: &Rule| {
        let mut t = toml::Table::new();
        t.insert("find".into(), r.find.clone().into());
        t.insert("replace".into(), r.replace.clone().into());
        t.insert("case".into(), r.case.into());
        toml::Value::Table(t)
    };
    toml::Value::Array(rules.iter().map(row).collect())
}

/// The text of a message the user sends with Enter, after the TextReplace rules (once, in order).
pub fn text_replace(text: String) -> String {
    if !enabled(Plugin::TextReplace) {
        return text;
    }
    apply(&rules(), text)
}

fn apply(rules: &[Rule], mut text: String) -> String {
    for r in rules.iter().filter(|r| !r.find.is_empty()) {
        text = replace(&text, &r.find, &r.replace.replace("\\n", "\n"), r.case);
    }
    text
}

/// Every non-overlapping `find` in `text` replaced by `with`; the result is not searched again.
fn replace(text: &str, find: &str, with: &str, case: bool) -> String {
    if case {
        return text.replace(find, with);
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        match prefix_len_ci(rest, find) {
            Some(n) => {
                out.push_str(with);
                rest = &rest[n..];
            }
            None => {
                out.push(c);
                rest = &rest[c.len_utf8()..];
            }
        }
    }
    out
}

/// Byte length of the start of `s` that equals `p` ignoring case (char by char).
fn prefix_len_ci(s: &str, p: &str) -> Option<usize> {
    let mut it = s.char_indices();
    for pc in p.chars() {
        let (_, sc) = it.next()?;
        if !sc.to_lowercase().eq(pc.to_lowercase()) {
            return None;
        }
    }
    Some(it.next().map_or(s.len(), |(i, _)| i))
}

/// Discord's message length limit: 4000 with full Nitro (premium_type 2), else 2000.
pub fn max_len(premium_type: u8) -> usize {
    if premium_type == 2 { 4000 } else { 2000 }
}

/// CharacterCounter is on: the composer counts every non-empty draft.
pub fn counting() -> bool {
    enabled(Plugin::CharacterCounter)
}

/// The composer's counter chip ("n/limit", over the limit) for a draft of `len` characters.
pub fn counter(len: usize, max: usize) -> Option<(String, bool)> {
    (len > 0).then(|| (format!("{len}/{max}"), len > max))
}

/// QuickReply: Ctrl+Up / Ctrl+Down (Ctrl+Shift: my own messages) pressed in the composer or an
/// inline edit. Takes the key out of the input before the text box sees it. Returns
/// `(own, up)`. Ctrl+Alt+Up/Down is "previous/next server" (egui matches shortcuts with extra
/// Alt), so it is left alone.
pub fn quick_reply_key(ctx: &egui::Context) -> Option<(bool, bool)> {
    if !enabled(Plugin::QuickReply) {
        return None;
    }
    ctx.input_mut(|i| {
        let mut hit = None;
        i.events.retain(|e| match e {
            egui::Event::Key { key: k @ (egui::Key::ArrowUp | egui::Key::ArrowDown), pressed: true, modifiers, .. } if modifiers.command && !modifiers.alt => {
                hit = Some((modifiers.shift, *k == egui::Key::ArrowUp));
                false
            }
            _ => true,
        });
        hit
    })
}

/// The messages the keys walk over, oldest first: no system messages (only default and reply
/// types), none deleted (MessageLogger keeps them greyed) or hidden (NoBlockedMessages), and with
/// `own` only mine that can be edited.
fn targets(msgs: &[Message], me: Option<Id>, own: bool, hidden: impl Fn(&Message) -> bool) -> Vec<Id> {
    msgs.iter()
        .filter(|m| matches!(m.kind, 0 | 19) && !hidden(m) && (!own || (Some(m.author.id) == me && m.can_edit())))
        .map(|m| m.id)
        .collect()
}

/// One key press: from `current` (none = nothing picked) one step up (older) or down (newer).
/// Up from nothing picks the newest; up at the oldest stays; down past the newest picks nothing.
fn step(list: &[Id], current: Option<Id>, up: bool) -> Option<Id> {
    let at = current.and_then(|c| list.iter().position(|m| *m == c));
    match (at, up) {
        (None, true) => list.last().copied(),
        (None, false) => None,
        (Some(i), true) => list.get(i.saturating_sub(1)).copied(),
        (Some(i), false) => list.get(i + 1).copied(),
    }
}

/// Applies a QuickReply key in channel `ch`: the reply bar (ping as clicking Reply, NoReplyMention
/// included) or the inline edit moves to the next message and the list scrolls to it.
pub fn quick_reply(cx: &mut UiCx<'_>, ch: Id, own: bool, up: bool) {
    let me = cx.state.me.as_ref().map(|u| u.id);
    let Some(list) = cx.state.messages.get(&ch) else { return };
    let ids = targets(&list.msgs, me, own, |m| super::chat::hide_message(cx.state, m) || super::message_logger::is_deleted(m.id));
    let current = if own { cx.view.editing.filter(|e| e.0 == ch).map(|e| e.1) } else { cx.view.reply_to.filter(|r| r.0 == ch).map(|r| r.1) };
    let next = step(&ids, current, up);
    if own {
        cx.view.reply_to = None;
        cx.view.editing = next.map(|id| (ch, id));
        if let Some(m) = next.and_then(|id| list.msgs.iter().find(|m| m.id == id)) {
            cx.view.edit_text = m.content.clone();
        }
    } else {
        cx.view.editing = None;
        cx.view.reply_to = next.map(|id| (ch, id, super::chat::reply_ping(cx.state, ch, id)));
    }
    cx.view.scroll_to_message = next;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(find: &str, replace: &str, case: bool) -> Rule {
        Rule { find: find.into(), replace: replace.into(), case }
    }

    #[test]
    fn text_replace_rules() {
        let rules = [rule("teh", "the", false), rule("RUST", "Rust", true), rule("", "x", false), rule("br", "line\\nbreak", false)];
        assert_eq!(apply(&rules, "Teh RUST and teh rust br".into()), "the Rust and the rust line\nbreak");
        // Applied once: a replacement containing its own `find` does not grow.
        assert_eq!(apply(&[rule("lol", "lol 😂", false)], "LOL".into()), "lol 😂");
        // Case-insensitive across non-ASCII, the text around kept byte for byte.
        assert_eq!(replace("Ärger ÄRGER ärger", "ärger", "Ä", false), "Ä Ä Ä");
        assert_eq!(replace("ab", "abc", "x", false), "ab");
        // Off by default: nothing changes.
        assert_eq!(text_replace("teh".into()), "teh");
        // Round trip through plugins.toml.
        let back: Vec<Rule> = match rules_value(&rules) {
            toml::Value::Array(a) => a
                .iter()
                .filter_map(toml::Value::as_table)
                .map(|t| rule(t["find"].as_str().unwrap(), t["replace"].as_str().unwrap(), t["case"].as_bool().unwrap()))
                .collect(),
            _ => panic!(),
        };
        assert_eq!(back, rules);
    }

    fn ids(v: &[u64]) -> Vec<Id> {
        v.iter().map(|n| Id(*n)).collect()
    }

    #[test]
    fn quick_reply_steps() {
        let list = ids(&[1, 2, 3]);
        let go = |cur: Option<u64>, up| step(&list, cur.map(Id), up).map(|i| i.0);
        // Nothing picked: up takes the newest, down stays nothing.
        assert_eq!((go(None, true), go(None, false)), (Some(3), None));
        // Walk up to the top and stay there.
        assert_eq!((go(Some(3), true), go(Some(2), true), go(Some(1), true)), (Some(2), Some(1), Some(1)));
        // Walk down; past the newest it cancels.
        assert_eq!((go(Some(1), false), go(Some(2), false), go(Some(3), false)), (Some(2), Some(3), None));
        // A picked message that is gone from the list starts over, as if nothing were picked.
        assert_eq!(go(Some(9), true), Some(3));
        assert_eq!(step(&[], None, true), None);
    }

    #[test]
    fn quick_reply_targets_skip_system_hidden_and_foreign_messages() {
        let me = Id(7);
        let m = |id: u64, kind: u8, author: u64| Message { id: Id(id), kind, author: crate::model::User { id: Id(author), ..Default::default() }, ..Default::default() };
        let msgs = [m(1, 0, 7), m(2, 7, 7), m(3, 19, 8), m(4, 0, 7), m(5, 0, 8)];
        assert_eq!(targets(&msgs, Some(me), false, |_| false), ids(&[1, 3, 4, 5]));
        assert_eq!(targets(&msgs, Some(me), true, |_| false), ids(&[1, 4]));
        assert_eq!(targets(&msgs, Some(me), false, |m| m.id == Id(5)), ids(&[1, 3, 4]));
        assert_eq!(targets(&msgs, None, true, |_| false), ids(&[]));
    }

    #[test]
    fn counter_limits() {
        assert_eq!((max_len(0), max_len(1), max_len(2), max_len(3)), (2000, 2000, 4000, 2000));
        assert_eq!(counter(0, 2000), None);
        assert_eq!(counter(12, 2000), Some(("12/2000".into(), false)));
        assert_eq!(counter(2000, 2000), Some(("2000/2000".into(), false)));
        assert_eq!(counter(2001, 2000), Some(("2001/2000".into(), true)));
        assert_eq!(counter(2001, 4000), Some(("2001/4000".into(), false)));
        assert!(!counting());
    }
}
