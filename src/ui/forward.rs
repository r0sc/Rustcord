//! Forwarded messages (docs/PROTOCOL_NOTES.md, Forward): a received forward is drawn by
//! `messages::forwarded`; this file holds the test hook for it.
//! OWNERSHIP: ui-messages.

use super::UiCx;
use crate::model::{Id, Message, MessageReference, MessageSnapshot, User};
use crate::state::AppState;

/// A cached user other than me (the lowest id, so screenshots stay the same between runs).
pub(super) fn demo_author(state: &AppState) -> User {
    let me = state.me.as_ref().map(|u| u.id);
    state.users.values().filter(|u| Some(u.id) != me).min_by_key(|u| u.id).cloned().unwrap_or_default()
}

/// `RUSTCORD_MSG_DEMO=forward`: a forward of the message in the middle of the loaded history.
pub(super) fn demo(cx: &mut UiCx<'_>, ch: Id, demo: impl Fn(&str) -> bool) {
    if !demo("forward") {
        return;
    }
    let author = demo_author(cx.state);
    let guild_id = cx.state.channels.get(&ch).and_then(|c| c.guild_id);
    let Some(list) = cx.state.messages.get_mut(&ch) else { return };
    let (Some(src), Some(newest)) = (list.msgs.get(list.msgs.len() / 2).cloned(), list.msgs.last()) else { return };
    let snap = Message {
        content: src.content.clone(),
        attachments: src.attachments.clone(),
        embeds: src.embeds.clone(),
        mentions: src.mentions.clone(),
        timestamp: src.timestamp.clone(),
        ..Message::default()
    };
    let forward = Message {
        id: Id(newest.id.0 + (1 << 22)),
        channel_id: ch,
        guild_id,
        author,
        timestamp: newest.timestamp.clone(),
        flags: 1 << 14,
        message_reference: Some(MessageReference { message_id: Some(src.id), channel_id: Some(ch), guild_id, kind: 1 }),
        message_snapshots: vec![MessageSnapshot { message: snap }],
        ..Message::default()
    };
    list.msgs.push(forward);
}
