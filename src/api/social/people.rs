//! Relationships, DMs, inbox, profiles and notes (FEATURES_NOTES §2.3-§2.6). Same rules as the
//! rest of `api/social.rs`: one request per user action, nothing retried on 4xx, captcha ->
//! "do it in the official app" (`error_from`), `X-Context-Properties` per §0.3.
//!
//! OWNERSHIP: friends/DMs/inbox engineer (docs/SOCIAL_DESIGN.md). The signatures are the contract.

use super::super::{ApiClient, ApiError, net_err};
use crate::backend::{ContextLocation, FriendReply, MAX_REJECT_BATCH, MentionFilter};
use crate::model::{Channel, Id, Message, UserProfile};
use base64::Engine as _;
use reqwest::{Method, Url};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

/// 80013: the request comes from someone without shared context; the user must confirm it.
const CONFIRM_STRANGER: u64 = 80013;
/// Group DMs hold 10 people including me.
const MAX_GROUP_OTHERS: usize = 9;
const MAX_NOTE_CHARS: usize = 256;
/// `POST /read-states/ack-bulk` chunk size (cap UNCONFIRMED; 100 is what the notes use).
const ACK_CHUNK: usize = 100;
/// `consent_status` ACCEPTED; the other statuses are employee-only.
const CONSENT_ACCEPTED: u8 = 2;
/// `supplemental-data` takes 1-25 channel ids.
const MAX_PREVIEW_IDS: usize = 25;

impl ApiClient {
    /// `POST /users/@me/relationships {"username":"name","discriminator":null}` (a typed
    /// `name#1234` sends `"discriminator":"1234"`) with `X-Context-Properties` `{"location":"Add Friend"}`
    /// -> 204. Codes 80000-80008, 30002 -> readable text (FEATURES_NOTES §0.4).
    pub async fn send_friend_request(&self, username: &str) -> Result<FriendReply, ApiError> {
        let body = friend_request_body(username).ok_or_else(|| local("Enter a username."))?;
        let url = Self::url(&["users", "@me", "relationships"]);
        self.send_coded(Method::POST, url, Some(body), &context(Some("Add Friend"))).await.map_err(readable)?;
        Ok(FriendReply::Done)
    }

    /// `PUT /users/@me/relationships/{u} {}` (+ `"confirm_stranger_request":true` when
    /// `confirm_stranger`) with `X-Context-Properties` for `location` -> 204 = `Done`;
    /// 80013 -> `Ok(ConfirmStranger)`; other 800xx -> readable text.
    pub async fn accept_friend(&self, user_id: Id, confirm_stranger: bool, location: ContextLocation) -> Result<FriendReply, ApiError> {
        let body = if confirm_stranger { json!({ "confirm_stranger_request": true }) } else { json!({}) };
        match self.send_coded(Method::PUT, relationship_url(user_id), Some(body), &context(Some(location_name(location)))).await {
            Ok(_) => Ok(FriendReply::Done),
            // Asked once; a second 80013 after confirming is an error, not another question.
            Err((_, Some(CONFIRM_STRANGER))) if !confirm_stranger => Ok(FriendReply::ConfirmStranger),
            Err(e) => Err(readable(e)),
        }
    }

    /// `DELETE /users/@me/relationships/{u}` with `X-Context-Properties` -> 204.
    pub async fn remove_relationship(&self, user_id: Id, location: ContextLocation) -> Result<(), ApiError> {
        let ctx = context(Some(location_name(location)));
        self.send_coded(Method::DELETE, relationship_url(user_id), None, &ctx).await.map(drop).map_err(readable)
    }

    /// `PUT /users/@me/relationships/{u} {"type":2}` with `X-Context-Properties` -> 204.
    pub async fn block_user(&self, user_id: Id, location: ContextLocation) -> Result<(), ApiError> {
        let ctx = context(Some(location_name(location)));
        let body = json!({ "type": crate::model::relationship_type::BLOCKED });
        self.send_coded(Method::PUT, relationship_url(user_id), Some(body), &ctx).await.map(drop).map_err(readable)
    }

    /// `POST /users/@me/channels {"recipients":["<u>"]}` with `X-Context-Properties: e30=` -> the DM
    /// (the existing one if any). 50007 -> readable text.
    pub async fn open_dm(&self, user_id: Id) -> Result<Channel, ApiError> {
        let body = json!({ "recipients": [user_id] });
        let resp = self.send_coded(Method::POST, Self::url(&["users", "@me", "channels"]), Some(body), &context(None)).await;
        json_of(resp.map_err(readable)?).await
    }

    /// `POST /users/@me/channels {"recipients":[…]}` with `X-Context-Properties` `{"location":"New Group DM"}`
    /// -> the group. 30004 / 30011 / 50033 -> readable text.
    pub async fn create_group_dm(&self, recipients: &[Id]) -> Result<Channel, ApiError> {
        if recipients.is_empty() {
            return Err(local("Pick at least one friend."));
        }
        if recipients.len() > MAX_GROUP_OTHERS {
            return Err(local("A group can have at most 10 members."));
        }
        let body = json!({ "recipients": recipients });
        let ctx = context(Some("New Group DM"));
        let resp = self.send_coded(Method::POST, Self::url(&["users", "@me", "channels"]), Some(body), &ctx).await;
        json_of(resp.map_err(readable)?).await
    }

    /// `PUT /channels/{c}/recipients/{u}` with `X-Context-Properties` `{"location":"Add Friends to DM"}`:
    /// 204 -> `None` (a group); a channel body -> `Some` (a 1:1 DM became a new group).
    pub async fn add_recipient(&self, channel_id: Id, user_id: Id) -> Result<Option<Channel>, ApiError> {
        let ctx = context(Some("Add Friends to DM"));
        let resp = self.send_coded(Method::PUT, recipient_url(channel_id, user_id), None, &ctx).await.map_err(readable)?;
        let bytes = resp.bytes().await.map_err(net_err)?;
        if bytes.iter().all(u8::is_ascii_whitespace) {
            return Ok(None);
        }
        serde_json::from_slice(&bytes).map(Some).map_err(|e| ApiError::Decode(e.to_string()))
    }

    /// `DELETE /channels/{c}/recipients/{u}` -> 204.
    pub async fn remove_recipient(&self, channel_id: Id, user_id: Id) -> Result<(), ApiError> {
        self.send_coded(Method::DELETE, recipient_url(channel_id, user_id), None, &[]).await.map(drop).map_err(readable)
    }

    /// `PATCH /channels/{c} {"name": name | null (empty)}` -> the channel (dropped; CHANNEL_UPDATE follows).
    pub async fn rename_group(&self, channel_id: Id, name: &str) -> Result<(), ApiError> {
        let name = Some(name.trim()).filter(|n| !n.is_empty());
        let url = Self::url(&["channels", &channel_id.to_string()]);
        self.send(Method::PATCH, url, Some(json!({ "name": name }))).await.map(drop)
    }

    /// `DELETE /channels/{c}[?silent=true]` -> the channel (dropped; CHANNEL_DELETE follows).
    pub async fn close_dm(&self, channel_id: Id, silent: bool) -> Result<(), ApiError> {
        let mut url = Self::url(&["channels", &channel_id.to_string()]);
        if silent {
            url.query_pairs_mut().append_pair("silent", "true");
        }
        self.send(Method::DELETE, url, None).await.map(drop)
    }

    /// Accept a message request: `PUT /channels/{c}/recipients/@me {"consent_status":2}` -> the DM
    /// (dropped; CHANNEL_UPDATE follows with `is_message_request:false`).
    pub async fn accept_message_request(&self, c: Id) -> Result<(), ApiError> {
        self.send(Method::PUT, request_url(c), Some(json!({ "consent_status": CONSENT_ACCEPTED }))).await.map(drop)
    }

    /// Ignore message requests: one id `DELETE /channels/{c}/recipients/@me`, 2..=50 `PUT
    /// /channels/recipients/@me/batch-reject {"channel_ids"}`. The DMs come back (dropped);
    /// CHANNEL_UPDATE / CHANNEL_DELETE follow.
    pub async fn reject_message_requests(&self, ids: &[Id]) -> Result<(), ApiError> {
        let (method, url, body) = reject_route(ids)?;
        self.send(method, url, body).await.map(drop)
    }

    /// `GET /users/@me/message-requests/supplemental-data?channel_ids=…` (1-25) -> the trigger
    /// message of each request that has one.
    pub async fn message_request_previews(&self, ids: &[Id]) -> Result<Vec<Message>, ApiError> {
        if ids.is_empty() || ids.len() > MAX_PREVIEW_IDS {
            return Err(local("Ask for 1 to 25 message request previews at a time."));
        }
        let wire: Vec<PreviewWire> = self.send_json(Method::GET, previews_url(ids), None).await?;
        Ok(wire.into_iter().filter_map(PreviewWire::into_message).collect())
    }

    /// `GET /users/@me/mentions?limit=1..100&roles=&everyone=[&guild_id=][&before=]` -> newest first.
    pub async fn mentions(&self, filter: MentionFilter, before: Option<Id>, limit: u8) -> Result<Vec<Message>, ApiError> {
        self.send_json(Method::GET, mentions_url(filter, before, limit), None).await
    }

    /// `DELETE /users/@me/mentions/{m}` -> 204.
    pub async fn delete_mention(&self, message_id: Id) -> Result<(), ApiError> {
        self.send(Method::DELETE, Self::url(&["users", "@me", "mentions", &message_id.to_string()]), None).await.map(drop)
    }

    /// `POST /read-states/ack-bulk {"read_states":[{"channel_id","message_id","read_state_type":0}…]}`
    /// in chunks of ≤ 100, one after another -> 204 each.
    pub async fn ack_bulk(&self, acks: &[(Id, Id)]) -> Result<(), ApiError> {
        for chunk in acks.chunks(ACK_CHUNK) {
            self.send(Method::POST, Self::url(&["read-states", "ack-bulk"]), Some(ack_bulk_body(chunk))).await?;
        }
        Ok(())
    }

    /// `GET /users/{u}/profile?with_mutual_guilds=true&with_mutual_friends=true&with_mutual_friends_count=false[&guild_id=]&type=popout`.
    /// 403 (no shared context) -> readable text.
    pub async fn profile(&self, user_id: Id, guild_id: Option<Id>) -> Result<UserProfile, ApiError> {
        self.send_json(Method::GET, profile_url(user_id, guild_id), None).await.map_err(|e| match e {
            ApiError::Forbidden(_) => local("You can't see this profile: you share no server or friendship with this user."),
            e => e,
        })
    }

    /// `GET /users/@me/notes/{u}` -> `{note}`; 404 -> `Ok(None)`; empty note -> `None`.
    pub async fn note(&self, user_id: Id) -> Result<Option<String>, ApiError> {
        #[derive(serde::Deserialize)]
        struct NoteWire {
            #[serde(default, deserialize_with = "crate::model::null_default")]
            note: String,
        }
        match self.send_json::<NoteWire>(Method::GET, note_url(user_id), None).await {
            Ok(w) => Ok(Some(w.note).filter(|n| !n.is_empty())),
            Err(ApiError::NotFound) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// `PUT /users/@me/notes/{u} {"note"}` (≤ 256 chars, `""` clears) -> 204.
    pub async fn set_note(&self, user_id: Id, note: &str) -> Result<(), ApiError> {
        if note.chars().count() > MAX_NOTE_CHARS {
            return Err(local("A note can be at most 256 characters long."));
        }
        self.send(Method::PUT, note_url(user_id), Some(json!({ "note": note }))).await.map(drop)
    }
}

fn local(message: &str) -> ApiError {
    ApiError::Http { status: 0, message: message.into() }
}

async fn json_of<T: DeserializeOwned>(resp: reqwest::Response) -> Result<T, ApiError> {
    let bytes = resp.bytes().await.map_err(net_err)?;
    serde_json::from_slice(&bytes).map_err(|e| ApiError::Decode(e.to_string()))
}

/// `X-Context-Properties`: standard base64 of the compact location JSON (`{}` without one).
fn context(location: Option<&str>) -> [(&'static str, String); 1] {
    let json = location.map_or_else(|| json!({}), |l| json!({ "location": l }));
    [("x-context-properties", base64::engine::general_purpose::STANDARD.encode(json.to_string()))]
}

fn location_name(l: ContextLocation) -> &'static str {
    match l {
        ContextLocation::Friends => "Friends",
        ContextLocation::UserProfile => "User Profile",
        ContextLocation::ContextMenu => "ContextMenu",
        ContextLocation::DmChannel => "DM Channel",
    }
}

/// Plain text for the relationship / DM error codes (FEATURES_NOTES §0.4); anything else, a
/// captcha answer included (it has no `code`), stays as it is.
fn readable((err, code): (ApiError, Option<u64>)) -> ApiError {
    let text = match code {
        Some(80000) => "This user isn't accepting friend requests.",
        Some(80001) => "This user can't receive your friend request.",
        Some(80002) => "Bots can't be added as friends.",
        Some(80003) => "You can't add yourself as a friend.",
        Some(80004) => "No user with that username. Check the spelling.",
        Some(80005) => "There is no pending friend request from this user.",
        Some(80006) => "You need to be friends with this user first.",
        Some(80007) => "You're already friends with this user.",
        Some(80008) => "This name needs its #number (name#1234).",
        Some(CONFIRM_STRANGER) => "This friend request needs a confirmation. Accept it in the official app.",
        Some(30002) => "You've reached the limit of 1,000 friends.",
        Some(30004) => "A group can have at most 10 members.",
        Some(30011) => "You've reached the maximum number of group DMs.",
        Some(50007) => "You can't message this user.",
        Some(50033) => "Some of these users can't be added to a group.",
        _ => return err,
    };
    local(text)
}

/// Add Friend input -> request body; `name#1234` (4 digits) is a legacy tag. None when empty.
fn friend_request_body(input: &str) -> Option<Value> {
    let input = input.trim();
    let (name, discriminator) = match input.rsplit_once('#') {
        Some((n, d)) if d.len() == 4 && d.bytes().all(|b| b.is_ascii_digit()) => (n.trim(), Some(d)),
        _ => (input, None),
    };
    (!name.is_empty()).then(|| json!({ "username": name, "discriminator": discriminator }))
}

fn relationship_url(user_id: Id) -> Url {
    ApiClient::url(&["users", "@me", "relationships", &user_id.to_string()])
}

fn recipient_url(channel_id: Id, user_id: Id) -> Url {
    ApiClient::url(&["channels", &channel_id.to_string(), "recipients", &user_id.to_string()])
}

fn request_url(c: Id) -> Url {
    ApiClient::url(&["channels", &c.to_string(), "recipients", "@me"])
}

fn previews_url(ids: &[Id]) -> Url {
    let mut url = ApiClient::url(&["users", "@me", "message-requests", "supplemental-data"]);
    url.query_pairs_mut().extend_pairs(ids.iter().map(|id| ("channel_ids", id.to_string())));
    url
}

/// The request for ignoring `ids`: one DELETE, or one batch PUT.
fn reject_route(ids: &[Id]) -> Result<(Method, Url, Option<Value>), ApiError> {
    match ids {
        [] => Err(local("Pick at least one request.")),
        [one] => Ok((Method::DELETE, request_url(*one), None)),
        _ if ids.len() > MAX_REJECT_BATCH => Err(local("You can ignore up to 50 requests at once.")),
        _ => Ok((Method::PUT, ApiClient::url(&["channels", "recipients", "@me", "batch-reject"]), Some(json!({ "channel_ids": ids })))),
    }
}

#[derive(serde::Deserialize)]
struct PreviewWire {
    channel_id: Id,
    message_preview: Option<Message>,
}

impl PreviewWire {
    fn into_message(self) -> Option<Message> {
        self.message_preview.map(|m| Message { channel_id: self.channel_id, ..m })
    }
}

fn note_url(user_id: Id) -> Url {
    ApiClient::url(&["users", "@me", "notes", &user_id.to_string()])
}

fn mentions_url(filter: MentionFilter, before: Option<Id>, limit: u8) -> Url {
    let mut url = ApiClient::url(&["users", "@me", "mentions"]);
    {
        let mut q = url.query_pairs_mut();
        q.append_pair("limit", &limit.clamp(1, 100).to_string());
        q.append_pair("roles", if filter.roles { "true" } else { "false" });
        q.append_pair("everyone", if filter.everyone { "true" } else { "false" });
        if let Some(g) = filter.guild_id {
            q.append_pair("guild_id", &g.to_string());
        }
        if let Some(b) = before {
            q.append_pair("before", &b.to_string());
        }
    }
    url
}

fn profile_url(user_id: Id, guild_id: Option<Id>) -> Url {
    let mut url = ApiClient::url(&["users", &user_id.to_string(), "profile"]);
    {
        let mut q = url.query_pairs_mut();
        q.append_pair("with_mutual_guilds", "true");
        q.append_pair("with_mutual_friends", "true");
        q.append_pair("with_mutual_friends_count", "false");
        if let Some(g) = guild_id {
            q.append_pair("guild_id", &g.to_string());
        }
        q.append_pair("type", "popout");
    }
    url
}

fn ack_bulk_body(acks: &[(Id, Id)]) -> Value {
    let states: Vec<Value> =
        acks.iter().map(|(c, m)| json!({ "channel_id": c, "message_id": m, "read_state_type": 0 })).collect();
    json!({ "read_states": states })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_headers_match_the_notes() {
        // FEATURES_NOTES §0.3 table.
        assert_eq!(context(None)[0], ("x-context-properties", "e30=".to_owned()));
        assert_eq!(context(Some("Add Friend"))[0].1, "eyJsb2NhdGlvbiI6IkFkZCBGcmllbmQifQ==");
        assert_eq!(context(Some("New Group DM"))[0].1, "eyJsb2NhdGlvbiI6Ik5ldyBHcm91cCBETSJ9");
        assert_eq!(context(Some("Add Friends to DM"))[0].1, "eyJsb2NhdGlvbiI6IkFkZCBGcmllbmRzIHRvIERNIn0=");
        let loc = |l| context(Some(location_name(l)))[0].1.clone();
        assert_eq!(loc(ContextLocation::Friends), "eyJsb2NhdGlvbiI6IkZyaWVuZHMifQ==");
        assert_eq!(loc(ContextLocation::UserProfile), "eyJsb2NhdGlvbiI6IlVzZXIgUHJvZmlsZSJ9");
        assert_eq!(loc(ContextLocation::ContextMenu), "eyJsb2NhdGlvbiI6IkNvbnRleHRNZW51In0=");
        assert_eq!(loc(ContextLocation::DmChannel), "eyJsb2NhdGlvbiI6IkRNIENoYW5uZWwifQ==");
    }

    #[test]
    fn friend_request_bodies() {
        assert_eq!(friend_request_body("  zed "), Some(json!({"username":"zed","discriminator":null})));
        assert_eq!(friend_request_body("Old#0420"), Some(json!({"username":"Old","discriminator":"0420"})));
        assert_eq!(friend_request_body("odd#12"), Some(json!({"username":"odd#12","discriminator":null})), "not a tag");
        assert_eq!(friend_request_body("   "), None);
        assert_eq!(friend_request_body("#1234"), None);
    }

    #[test]
    fn error_codes_become_readable_text() {
        let http = |message: &str| ApiError::Http { status: 400, message: message.into() };
        assert!(readable((http("Unknown User"), Some(80004))).to_string().contains("No user with that username"));
        assert!(readable((ApiError::Forbidden("x".into()), Some(50007))).to_string().contains("can't message"));
        let captcha = http("Discord wants a captcha for this action. Complete it in the official app, then try again.");
        assert!(readable((captcha, None)).to_string().contains("captcha"), "kept, never handled here");
        assert!(matches!(readable((ApiError::NotFound, Some(10013))), ApiError::NotFound));
    }

    #[test]
    fn routes_and_bodies() {
        let base = "https://discord.com/api/v9";
        assert_eq!(relationship_url(Id(2)).as_str(), format!("{base}/users/@me/relationships/2"));
        assert_eq!(recipient_url(Id(5), Id(2)).as_str(), format!("{base}/channels/5/recipients/2"));
        assert_eq!(note_url(Id(2)).as_str(), format!("{base}/users/@me/notes/2"));
        let f = MentionFilter { guild_id: Some(Id(10)), roles: false, everyone: true };
        assert_eq!(
            mentions_url(f, Some(Id(99)), 0).as_str(),
            format!("{base}/users/@me/mentions?limit=1&roles=false&everyone=true&guild_id=10&before=99")
        );
        assert_eq!(mentions_url(MentionFilter::default(), None, 200).query(), Some("limit=100&roles=true&everyone=true"));
        assert_eq!(
            profile_url(Id(2), Some(Id(10))).as_str(),
            format!("{base}/users/2/profile?with_mutual_guilds=true&with_mutual_friends=true&with_mutual_friends_count=false&guild_id=10&type=popout")
        );
        assert!(!profile_url(Id(2), None).as_str().contains("guild_id"));
        assert_eq!(
            ack_bulk_body(&[(Id(1), Id(2))]),
            json!({"read_states":[{"channel_id":"1","message_id":"2","read_state_type":0}]})
        );
        let acks: Vec<(Id, Id)> = (0..250).map(|i| (Id(i), Id(i))).collect();
        assert_eq!(acks.chunks(ACK_CHUNK).map(<[_]>::len).collect::<Vec<_>>(), [100, 100, 50]);
    }

    #[test]
    fn message_request_routes() {
        let base = "https://discord.com/api/v9";
        assert_eq!(request_url(Id(5)).as_str(), format!("{base}/channels/5/recipients/@me"));
        assert!(previews_url(&[Id(1), Id(2)]).as_str().ends_with("supplemental-data?channel_ids=1&channel_ids=2"));
        assert!(reject_route(&[]).is_err());
        let (m, u, b) = reject_route(&[Id(5)]).unwrap();
        assert_eq!((m, u.as_str().to_owned(), b), (Method::DELETE, format!("{base}/channels/5/recipients/@me"), None));
        let (m, u, b) = reject_route(&[Id(1), Id(2)]).unwrap();
        assert_eq!((m, u.as_str().to_owned()), (Method::PUT, format!("{base}/channels/recipients/@me/batch-reject")));
        assert_eq!(b, Some(json!({"channel_ids":["1","2"]})));
        let many = |n: u64| (1..=n).map(Id).collect::<Vec<_>>();
        assert!(reject_route(&many(50)).is_ok() && reject_route(&many(51)).is_err());
        let wire: Vec<PreviewWire> = serde_json::from_str(
            r#"[{"channel_id":"7","message_preview":{"id":"9","channel_id":"1","author":{"id":"3","username":"w"},"content":"hi","timestamp":"2026-11-01T00:00:00+00:00"}},{"channel_id":"8","message_preview":null}]"#,
        )
        .unwrap();
        let msgs: Vec<Message> = wire.into_iter().filter_map(PreviewWire::into_message).collect();
        assert_eq!((msgs.len(), msgs[0].channel_id), (1, Id(7)));
    }
}
