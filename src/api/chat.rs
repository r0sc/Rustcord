//! Chat-essentials REST calls (FEATURES_NOTES Part 1): typing, pins, search, GIFs and message
//! extras. Called from `backend::handle_chat`, which turns results into `ChatEvent`s /
//! `ActionFailed`. Every call goes through `ApiClient::send` (token only to the API host, rate
//! limiter, 401 handling); none is retried on 4xx.
//!
//! OWNERSHIP: REST engineer (docs/CHAT_ESSENTIALS_DESIGN.md). The signatures are the contract.

use super::{ApiClient, ApiError, net_err, reaction_path_emoji, send_body};
use crate::backend::{PollDraft, SearchOutcome, SearchQuery, SearchScope, SearchSort};
use crate::model::{Channel, Emoji, GifItem, Id, Message, PinsPage, SearchPage, User, null_default};
use reqwest::{Method, Url};
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::Duration;

/// Hits per search page (the API maximum; paging is `offset += 25`).
const SEARCH_PAGE: u32 = 25;
/// `/gifs/search` rejects `limit < 20` with 400.
const GIF_LIMIT: u32 = 50;
const _: () = assert!(GIF_LIMIT >= 20);
/// SUPPRESS_EMBEDS message flag.
const SUPPRESS_EMBEDS: u64 = 1 << 2;

impl ApiClient {
    /// `POST /channels/{c}/typing`, no body. 204 -> `Ok(None)`; 200
    /// `{"message_send_cooldown_ms"?}` (slowmode) -> `Ok(Some(ms))`. Throttled by the backend.
    /// A ping still waiting on 429s after one resend window is late and dropped (`RateLimited`).
    pub async fn typing(&self, channel_id: Id) -> Result<Option<u64>, ApiError> {
        let url = Self::url(&["channels", &channel_id.to_string(), "typing"]);
        let resp = tokio::time::timeout(crate::backend::TYPING_RESEND, self.send(Method::POST, url, None))
            .await
            .map_err(|_| ApiError::RateLimited)??;
        if resp.status().as_u16() == 204 {
            return Ok(None);
        }
        Ok(typing_cooldown(&resp.bytes().await.map_err(net_err)?))
    }

    /// `GET /channels/{c}/messages/pins?limit=50[&before=<pinned_at>]`.
    pub async fn pins(&self, channel_id: Id, before: Option<&str>) -> Result<PinsPage, ApiError> {
        self.send_json(Method::GET, pins_url(channel_id, before), None).await
    }

    /// `PUT /channels/{c}/messages/pins/{m}` -> 204.
    pub async fn pin(&self, channel_id: Id, message_id: Id) -> Result<(), ApiError> {
        self.send(Method::PUT, pin_url(channel_id, message_id), None).await.map(drop)
    }

    /// `DELETE /channels/{c}/messages/pins/{m}` -> 204.
    pub async fn unpin(&self, channel_id: Id, message_id: Id) -> Result<(), ApiError> {
        self.send(Method::DELETE, pin_url(channel_id, message_id), None).await.map(drop)
    }

    /// `GET /guilds/{g}/messages/search` or `GET /channels/{c}/messages/search` with the
    /// query as repeated keys, `limit=25`. 202 (code 110000, index not ready): sleep
    /// `retry_after` (0 -> 5 s, at most 10 s) and retry once as part of the user's search, then
    /// `Ok(SearchOutcome::Indexing)`. Other errors -> `Err` (the backend maps them to `Failed`).
    pub async fn search(&self, scope: SearchScope, query: &SearchQuery) -> Result<SearchOutcome, ApiError> {
        let url = search_url(scope, query);
        let mut retried = false;
        loop {
            let resp = self.send(Method::GET, url.clone(), None).await?;
            let indexing = resp.status().as_u16() == 202;
            let body = resp.bytes().await.map_err(net_err)?;
            if !indexing {
                return search_page(&body).map(SearchOutcome::Page);
            }
            let wait = index_wait(&body);
            if retried {
                return Ok(SearchOutcome::Indexing { retry_after_secs: wait });
            }
            retried = true;
            tokio::time::sleep(Duration::from_secs_f32(wait)).await;
        }
    }

    /// GIF picker, `media_format=gif&locale={locale}`: `None` -> `GET /gifs/trending` (its
    /// `gifs`), `Some(q)` -> `GET /gifs/search?q=…&limit=50` (limit must be ≥ 20). Only Discord's
    /// `/gifs/*` endpoints; the GIF hosts are reached by the image loader (no token).
    pub async fn gifs(&self, query: Option<&str>, locale: &str) -> Result<Vec<GifItem>, ApiError> {
        let resp = self.send(Method::GET, gifs_url(query, locale), None).await?;
        gif_list(&resp.bytes().await.map_err(net_err)?)
    }

    /// `POST /channels/{c}/messages/{m}/ack {"manual":true,"mention_count":n}` (Mark unread).
    pub async fn mark_unread(&self, channel_id: Id, message_id: Id, mention_count: u32) -> Result<(), ApiError> {
        let url = Self::url(&["channels", &channel_id.to_string(), "messages", &message_id.to_string(), "ack"]);
        self.send(Method::POST, url, Some(mark_unread_body(mention_count))).await.map(drop)
    }

    /// `GET /channels/{c}/messages?around={m}&limit={limit}` (1..=100), as received.
    pub async fn messages_around(&self, channel_id: Id, message_id: Id, limit: u8) -> Result<Vec<Message>, ApiError> {
        self.send_json(Method::GET, around_url(channel_id, message_id, limit), None).await
    }

    /// `PATCH /channels/{c}/messages/{m} {"flags": flags | 4}` (SUPPRESS_EMBEDS).
    pub async fn suppress_embeds(&self, channel_id: Id, message_id: Id, flags: u64) -> Result<(), ApiError> {
        let url = Self::url(&["channels", &channel_id.to_string(), "messages", &message_id.to_string()]);
        // The MESSAGE_UPDATE from the gateway carries the change.
        self.send(Method::PATCH, url, Some(json!({ "flags": flags | SUPPRESS_EMBEDS }))).await.map(drop)
    }

    /// `GET /channels/{c}/messages/{m}/reactions/{emoji}?type=0|1&limit=100[&after=<user id>]`
    /// (emoji path segment as in `add_reaction`).
    pub async fn reaction_users(
        &self,
        channel_id: Id,
        message_id: Id,
        emoji: &Emoji,
        burst: bool,
        after: Option<Id>,
    ) -> Result<Vec<User>, ApiError> {
        let url = reaction_users_url(channel_id, message_id, &reaction_path_emoji(emoji)?, burst, after);
        self.send_json(Method::GET, url, None).await
    }

    /// Forward: `POST /channels/{dest}/messages` with a type 1 `message_reference` to the
    /// message in `src_ch`, no content (MESSAGE_CREATE brings the forward).
    pub async fn forward(&self, src_ch: Id, src_guild: Option<Id>, src_mid: Id, dest: Id, nonce: &str) -> Result<(), ApiError> {
        let url = Self::url(&["channels", &dest.to_string(), "messages"]);
        self.send(Method::POST, url, Some(forward_body(dest, nonce, src_ch, src_mid, src_guild))).await.map(drop)
    }

    /// Create a poll: `POST /channels/{c}/messages` with `poll`, no content. MESSAGE_CREATE
    /// brings the poll.
    pub async fn send_poll(&self, channel_id: Id, nonce: &str, draft: &PollDraft) -> Result<(), ApiError> {
        let url = Self::url(&["channels", &channel_id.to_string(), "messages"]);
        self.send(Method::POST, url, Some(poll_body(channel_id, nonce, draft))).await.map(drop)
    }

    /// `PUT /channels/{c}/polls/{m}/answers/@me {"answer_ids":[…]}` (204); `[]` removes my
    /// votes. The vote events from the gateway carry the counts.
    pub async fn poll_vote(&self, channel_id: Id, message_id: Id, answer_ids: &[u32]) -> Result<(), ApiError> {
        let url = poll_url(channel_id, message_id, "@me");
        self.send(Method::PUT, url, Some(json!({ "answer_ids": answer_ids }))).await.map(drop)
    }

    /// `GET /channels/{c}/polls/{m}/answers/{a}?limit=100[&after=<user id>]` -> `{"users":[…]}`.
    pub async fn poll_voters(&self, channel_id: Id, message_id: Id, answer_id: u32, after: Option<Id>) -> Result<Vec<User>, ApiError> {
        let resp = self.send(Method::GET, voters_url(channel_id, message_id, answer_id, after), None).await?;
        voters(&resp.bytes().await.map_err(net_err)?)
    }
}

fn poll_url(channel_id: Id, message_id: Id, answer: &str) -> Url {
    ApiClient::url(&["channels", &channel_id.to_string(), "polls", &message_id.to_string(), "answers", answer])
}

fn voters_url(channel_id: Id, message_id: Id, answer_id: u32, after: Option<Id>) -> Url {
    let mut url = poll_url(channel_id, message_id, &answer_id.to_string());
    {
        let mut q = url.query_pairs_mut();
        q.append_pair("limit", "100");
        if let Some(a) = after {
            q.append_pair("after", &a.to_string());
        }
    }
    url
}

/// `{"users":[…]}`; a null or missing list is empty.
fn voters(body: &[u8]) -> Result<Vec<User>, ApiError> {
    #[derive(Deserialize)]
    struct Wire {
        #[serde(default, deserialize_with = "null_default")]
        users: Vec<User>,
    }
    serde_json::from_slice::<Wire>(body).map(|w| w.users).map_err(|e| ApiError::Decode(e.to_string()))
}

/// A send body without text: forwards (and polls) carry no `content` key at all.
fn bare_body(channel_id: Id, nonce: &str) -> Value {
    let mut body = send_body(channel_id, "", nonce, None);
    if let Some(o) = body.as_object_mut() {
        o.remove("content");
    }
    body
}

/// `duration` is always sent: the documented default differs (1 h or 24 h).
fn poll_body(channel_id: Id, nonce: &str, d: &PollDraft) -> Value {
    let mut body = bare_body(channel_id, nonce);
    body["poll"] = json!({
        "question": { "text": d.question },
        "answers": d.answers.iter().map(|a| json!({ "poll_media": { "text": a } })).collect::<Vec<_>>(),
        "duration": d.hours,
        "allow_multiselect": d.multi,
        "layout_type": 1,
    });
    body
}

fn forward_body(dest: Id, nonce: &str, src_ch: Id, src_mid: Id, src_guild: Option<Id>) -> Value {
    let mut body = bare_body(dest, nonce);
    let mut reference = json!({ "type": 1, "message_id": src_mid, "channel_id": src_ch });
    if let Some(g) = src_guild {
        reference["guild_id"] = json!(g);
    }
    body["message_reference"] = reference;
    body
}

fn pins_url(channel_id: Id, before: Option<&str>) -> Url {
    let mut url = ApiClient::url(&["channels", &channel_id.to_string(), "messages", "pins"]);
    {
        let mut q = url.query_pairs_mut();
        if let Some(b) = before.filter(|b| !b.is_empty()) {
            q.append_pair("before", b);
        }
        q.append_pair("limit", "50");
    }
    url
}

fn pin_url(channel_id: Id, message_id: Id) -> Url {
    ApiClient::url(&["channels", &channel_id.to_string(), "messages", "pins", &message_id.to_string()])
}

/// Slowmode cooldown from a 200 typing answer; 0 or missing means none.
fn typing_cooldown(body: &[u8]) -> Option<u64> {
    let v: Value = serde_json::from_slice(body).ok()?;
    let ms = v.get("message_send_cooldown_ms")?;
    ms.as_u64().or_else(|| ms.as_f64().filter(|f| f.is_finite() && *f > 0.0).map(|f| f.ceil() as u64)).filter(|&ms| ms > 0)
}

fn search_url(scope: SearchScope, q: &SearchQuery) -> Url {
    let mut url = match scope {
        SearchScope::Guild(g) => ApiClient::url(&["guilds", &g.to_string(), "messages", "search"]),
        SearchScope::Dm(c) => ApiClient::url(&["channels", &c.to_string(), "messages", "search"]),
    };
    {
        let mut p = url.query_pairs_mut();
        if let Some(c) = q.content.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
            p.append_pair("content", c);
        }
        // Lists are repeated keys (`author_id=1&author_id=2`).
        for id in &q.author_ids {
            p.append_pair("author_id", &id.to_string());
        }
        for id in &q.mentions {
            p.append_pair("mentions", &id.to_string());
        }
        for h in &q.has {
            p.append_pair("has", h);
        }
        // `channel_id` is a guild-search filter only.
        if matches!(scope, SearchScope::Guild(_)) {
            for id in &q.channel_ids {
                p.append_pair("channel_id", &id.to_string());
            }
        }
        if let Some(pinned) = q.pinned {
            p.append_pair("pinned", if pinned { "true" } else { "false" });
        }
        if let Some(id) = q.min_id {
            p.append_pair("min_id", &id.to_string());
        }
        if let Some(id) = q.max_id {
            p.append_pair("max_id", &id.to_string());
        }
        match q.sort {
            SearchSort::Newest => p.append_pair("sort_by", "timestamp").append_pair("sort_order", "desc"),
            SearchSort::Oldest => p.append_pair("sort_by", "timestamp").append_pair("sort_order", "asc"),
            SearchSort::Relevance => p.append_pair("sort_by", "relevance"),
        };
        if q.offset > 0 {
            p.append_pair("offset", &q.offset.to_string());
        }
        if q.include_nsfw {
            p.append_pair("include_nsfw", "true");
        }
        p.append_pair("limit", &SEARCH_PAGE.to_string());
    }
    url
}

/// Wire shape of a search answer: each inner array holds the hit (`"hit": true`), formerly
/// with context messages around it.
#[derive(Deserialize, Default)]
#[serde(default)]
struct SearchWire {
    #[serde(deserialize_with = "null_default")]
    total_results: u32,
    #[serde(deserialize_with = "null_default")]
    messages: Vec<Vec<Value>>,
    #[serde(deserialize_with = "null_default")]
    threads: Vec<Channel>,
    #[serde(deserialize_with = "null_default")]
    doing_deep_historical_index: bool,
}

fn search_page(body: &[u8]) -> Result<SearchPage, ApiError> {
    let decode = |e: serde_json::Error| ApiError::Decode(e.to_string());
    let wire: SearchWire = serde_json::from_slice(body).map_err(decode)?;
    let mut messages = Vec::with_capacity(wire.messages.len());
    for mut group in wire.messages {
        let i = group.iter().position(|m| m.get("hit").and_then(Value::as_bool) == Some(true)).unwrap_or(0);
        if i < group.len() {
            messages.push(serde_json::from_value(group.swap_remove(i)).map_err(decode)?);
        }
    }
    Ok(SearchPage {
        total_results: wire.total_results,
        messages,
        threads: wire.threads,
        doing_deep_historical_index: wire.doing_deep_historical_index,
    })
}

/// Seconds to wait on a 202 "index not ready": `retry_after`, 0/missing -> 5, at most 10.
pub(super) fn index_wait(body: &[u8]) -> f32 {
    let secs = serde_json::from_slice::<Value>(body).ok().and_then(|v| v.get("retry_after")?.as_f64()).unwrap_or(0.0);
    if secs.is_finite() && secs > 0.0 { secs.min(10.0) as f32 } else { 5.0 }
}

fn gifs_url(query: Option<&str>, locale: &str) -> Url {
    let q = query.map(str::trim).filter(|q| !q.is_empty());
    let mut url = ApiClient::url(&["gifs", if q.is_some() { "search" } else { "trending" }]);
    {
        let mut p = url.query_pairs_mut();
        if let Some(q) = q {
            p.append_pair("q", q);
        }
        // Animated GIFs only: egui can't play the video formats.
        p.append_pair("media_format", "gif");
        if !locale.is_empty() {
            p.append_pair("locale", locale);
        }
        if q.is_some() {
            p.append_pair("limit", &GIF_LIMIT.to_string());
        }
    }
    url
}

/// `/gifs/search` answers `[GIF]`, `/gifs/trending` `{"categories":[{name, src}],"gifs":[GIF]}`
/// (its categories come first, as items with `category` set).
fn gif_list(body: &[u8]) -> Result<Vec<GifItem>, ApiError> {
    #[derive(Deserialize)]
    struct Category {
        #[serde(default, deserialize_with = "null_default")]
        name: String,
        #[serde(default, deserialize_with = "null_default")]
        src: String,
    }
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Wire {
        List(Vec<GifItem>),
        Trending {
            #[serde(default, deserialize_with = "null_default")]
            categories: Vec<Category>,
            #[serde(default, deserialize_with = "null_default")]
            gifs: Vec<GifItem>,
        },
    }
    match serde_json::from_slice(body) {
        Ok(Wire::List(g)) => Ok(g),
        Ok(Wire::Trending { categories, gifs }) => {
            let tiles = categories.into_iter().filter(|c| !c.name.is_empty()).map(|c| GifItem { category: Some(c.name), src: c.src, ..Default::default() });
            Ok(tiles.chain(gifs).collect())
        }
        Err(e) => Err(ApiError::Decode(e.to_string())),
    }
}

fn mark_unread_body(mention_count: u32) -> Value {
    json!({ "manual": true, "mention_count": mention_count })
}

fn around_url(channel_id: Id, message_id: Id, limit: u8) -> Url {
    let mut url = ApiClient::url(&["channels", &channel_id.to_string(), "messages"]);
    url.query_pairs_mut()
        .append_pair("around", &message_id.to_string())
        .append_pair("limit", &limit.clamp(1, 100).to_string());
    url
}

fn reaction_users_url(channel_id: Id, message_id: Id, emoji: &str, burst: bool, after: Option<Id>) -> Url {
    let mut url = ApiClient::url(&["channels", &channel_id.to_string(), "messages", &message_id.to_string(), "reactions", emoji]);
    {
        let mut q = url.query_pairs_mut();
        q.append_pair("type", if burst { "1" } else { "0" });
        q.append_pair("limit", "100");
        if let Some(a) = after {
            q.append_pair("after", &a.to_string());
        }
    }
    url
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::ratelimit::route_key;

    #[test]
    fn typing_cooldown_from_body() {
        assert_eq!(typing_cooldown(b""), None, "204 has no body");
        assert_eq!(typing_cooldown(br#"{"message_send_cooldown_ms":4500}"#), Some(4500));
        assert_eq!(typing_cooldown(br#"{"message_send_cooldown_ms":1200.4}"#), Some(1201));
        assert_eq!(typing_cooldown(br#"{"message_send_cooldown_ms":0}"#), None);
        assert_eq!(typing_cooldown(br#"{"thread_create_cooldown_ms":9000}"#), None);
        assert_eq!(typing_cooldown(b"<html>"), None);
    }

    #[test]
    fn forward_body_has_no_content_and_a_type_1_reference() {
        let b = forward_body(Id(9), "7", Id(5), Id(6), Some(Id(3)));
        assert!(b.get("content").is_none());
        assert_eq!(b["message_reference"], json!({"type":1,"message_id":"6","channel_id":"5","guild_id":"3"}));
        assert_eq!(b["nonce"], "7");
        // A forward out of a DM has no guild.
        assert!(forward_body(Id(9), "7", Id(5), Id(6), None)["message_reference"].get("guild_id").is_none());
    }

    #[test]
    fn poll_body_has_no_content_and_always_a_duration() {
        let d = PollDraft { question: "Best?".into(), answers: vec!["A".into(), "B".into()], hours: 72, multi: true };
        let b = poll_body(Id(9), "7", &d);
        assert!(b.get("content").is_none());
        assert_eq!(b["nonce"], "7");
        assert_eq!(
            b["poll"],
            json!({"question":{"text":"Best?"},"answers":[{"poll_media":{"text":"A"}},{"poll_media":{"text":"B"}}],"duration":72,"allow_multiselect":true,"layout_type":1})
        );
    }

    #[test]
    fn poll_vote_url_and_body() {
        let u = poll_url(Id(1), Id(2), "@me");
        assert_eq!(u.as_str(), "https://discord.com/api/v9/channels/1/polls/2/answers/@me");
        assert_eq!(route_key("PUT", u.path()).route, "PUT /channels/{channel_id}/polls/{id}/answers/@me");
        assert_eq!(json!({ "answer_ids": [1u32, 3] }), json!({"answer_ids":[1,3]}));
        assert_eq!(json!({ "answer_ids": &[] as &[u32] }), json!({"answer_ids":[]}));
    }

    #[test]
    fn poll_voters_url_and_list() {
        let u = voters_url(Id(1), Id(2), 3, None);
        assert_eq!(u.as_str(), "https://discord.com/api/v9/channels/1/polls/2/answers/3?limit=100");
        assert_eq!(voters_url(Id(1), Id(2), 3, Some(Id(77))).query(), Some("limit=100&after=77"));
        let users = voters(br#"{"users":[{"id":"5","username":"a"},{"id":"6","username":"b"}]}"#).unwrap();
        assert_eq!(users.iter().map(|u| u.id).collect::<Vec<_>>(), [Id(5), Id(6)]);
        assert!(voters(br#"{"users":null}"#).unwrap().is_empty() && voters(b"{}").unwrap().is_empty());
        assert!(voters(b"<html>").is_err());
        // Listing voters never queues behind my own vote.
        let vote = poll_url(Id(1), Id(2), "@me");
        assert_ne!(route_key("GET", u.path()), route_key("PUT", vote.path()));
        assert_eq!(route_key("GET", u.path()).route, "GET /channels/{channel_id}/polls/{id}/answers/{id}");
    }

    #[test]
    fn pin_routes() {
        let u = pins_url(Id(1), None);
        assert_eq!(u.as_str(), "https://discord.com/api/v9/channels/1/messages/pins?limit=50");
        let u = pins_url(Id(1), Some("2026-01-02T03:04:05.678000+00:00"));
        assert_eq!(u.query(), Some("before=2026-01-02T03%3A04%3A05.678000%2B00%3A00&limit=50"));
        assert_eq!(pins_url(Id(1), Some("")).query(), Some("limit=50"));
        let u = pin_url(Id(1), Id(2));
        assert_eq!(u.as_str(), "https://discord.com/api/v9/channels/1/messages/pins/2");
        assert_eq!(route_key("PUT", u.path()).route, "PUT /channels/{channel_id}/messages/pins/{id}");
        assert_eq!(route_key("PUT", u.path()).major, "1");
    }

    #[test]
    fn search_query_uses_repeated_keys() {
        let q = SearchQuery {
            content: Some(" hello world ".into()),
            author_ids: vec![Id(1), Id(2)],
            mentions: vec![Id(3)],
            has: vec!["image".into(), "-link".into()],
            channel_ids: vec![Id(4), Id(5)],
            pinned: Some(true),
            min_id: Some(Id(6)),
            max_id: Some(Id(7)),
            offset: 25,
            sort: SearchSort::Oldest,
            include_nsfw: true,
        };
        let u = search_url(SearchScope::Guild(Id(9)), &q);
        assert_eq!(u.path(), "/api/v9/guilds/9/messages/search");
        assert_eq!(
            u.query(),
            Some(
                "content=hello+world&author_id=1&author_id=2&mentions=3&has=image&has=-link&channel_id=4&channel_id=5\
                 &pinned=true&min_id=6&max_id=7&sort_by=timestamp&sort_order=asc&offset=25&include_nsfw=true&limit=25"
            )
        );
        assert_eq!(route_key("GET", u.path()).route, "GET /guilds/{guild_id}/messages/search");

        // DM scope: no channel filter; defaults are newest first, no offset.
        let q = SearchQuery { content: Some("  ".into()), channel_ids: vec![Id(4)], ..Default::default() };
        let u = search_url(SearchScope::Dm(Id(8)), &q);
        assert_eq!(u.path(), "/api/v9/channels/8/messages/search");
        assert_eq!(u.query(), Some("sort_by=timestamp&sort_order=desc&limit=25"));
        let u = search_url(SearchScope::Dm(Id(8)), &SearchQuery { sort: SearchSort::Relevance, ..Default::default() });
        assert_eq!(u.query(), Some("sort_by=relevance&limit=25"));
    }

    #[test]
    fn search_page_takes_the_hit_of_each_group() {
        let body = br#"{"analytics_id":"x","doing_deep_historical_index":true,"total_results":2,
            "messages":[
              [{"id":"10","channel_id":"1","content":"ctx","author":{"id":"2"}},
               {"id":"11","channel_id":"1","content":"hit","hit":true,"author":{"id":"2"}}],
              [{"id":"12","channel_id":"3","content":"only","author":{"id":"2"}}],
              []
            ],
            "threads":[{"id":"3","type":11,"name":"t"}]}"#;
        let page = search_page(body).unwrap();
        assert_eq!(page.total_results, 2);
        assert!(page.doing_deep_historical_index);
        let ids: Vec<Id> = page.messages.iter().map(|m| m.id).collect();
        assert_eq!(ids, [Id(11), Id(12)]);
        assert_eq!(page.threads[0].name.as_deref(), Some("t"));

        let empty = search_page(br#"{"total_results":0,"messages":[],"threads":null}"#).unwrap();
        assert!(empty.messages.is_empty() && empty.threads.is_empty());
        assert!(matches!(search_page(b"<html>"), Err(ApiError::Decode(_))));
    }

    #[test]
    fn index_wait_is_bounded() {
        let body = br#"{"message":"Index not yet available. Try again later","code":110000,"documents_indexed":0,"retry_after":2}"#;
        assert_eq!(index_wait(body), 2.0);
        assert_eq!(index_wait(br#"{"retry_after":0}"#), 5.0);
        assert_eq!(index_wait(br#"{"retry_after":120}"#), 10.0);
        assert_eq!(index_wait(b""), 5.0);
    }

    #[test]
    fn gif_routes_and_shapes() {
        let u = gifs_url(None, "en-US");
        assert_eq!(u.as_str(), "https://discord.com/api/v9/gifs/trending?media_format=gif&locale=en-US");
        assert_eq!(gifs_url(Some("  "), "en-US"), u, "blank search = trending");
        let u = gifs_url(Some("cat hug"), "de");
        assert_eq!(u.as_str(), "https://discord.com/api/v9/gifs/search?q=cat+hug&media_format=gif&locale=de&limit=50");
        assert_eq!(gifs_url(Some("x"), "").query(), Some("q=x&media_format=gif&limit=50"));

        let search = br#"[{"id":"1","title":"t","url":"https://klipy.com/gifs/a","src":"https://static.klipy.com/a.gif","gif_src":"x.webp","preview":"p.jpg","width":200,"height":150}]"#;
        let g = gif_list(search).unwrap();
        assert_eq!((g[0].src.as_str(), g[0].width, g[0].height), ("https://static.klipy.com/a.gif", 200, 150));
        let trending = br#"{"categories":[{"name":"happy","src":"h.gif"}],"gifs":[{"url":"u1"},{"url":"u2"}]}"#;
        let t = gif_list(trending).unwrap();
        assert_eq!(t.len(), 3);
        assert_eq!((t[0].category.as_deref(), t[0].src.as_str()), (Some("happy"), "h.gif"), "categories first");
        assert!(t[1].category.is_none() && t[1].url == "u1");
        assert!(gif_list(br#"{"categories":[]}"#).unwrap().is_empty());
        assert!(gif_list(b"nope").is_err());
    }

    #[test]
    fn message_extras_requests() {
        assert_eq!(mark_unread_body(3), json!({"manual":true,"mention_count":3}));

        let u = around_url(Id(1), Id(2), 50);
        assert_eq!(u.as_str(), "https://discord.com/api/v9/channels/1/messages?around=2&limit=50");
        assert_eq!(around_url(Id(1), Id(2), 0).query(), Some("around=2&limit=1"));
        assert_eq!(around_url(Id(1), Id(2), 200).query(), Some("around=2&limit=100"));

        assert_eq!(json!({ "flags": 32u64 | SUPPRESS_EMBEDS }), json!({"flags": 36}));

        let thumbs = reaction_path_emoji(&Emoji { id: None, name: Some("👍".into()), animated: false }).unwrap();
        let u = reaction_users_url(Id(1), Id(2), &thumbs, false, None);
        assert_eq!(u.as_str(), "https://discord.com/api/v9/channels/1/messages/2/reactions/%F0%9F%91%8D?type=0&limit=100");
        let u = reaction_users_url(Id(1), Id(2), "blob:9", true, Some(Id(77)));
        assert_eq!(u.query(), Some("type=1&limit=100&after=77"));
        // Viewing who reacted must not queue behind (or slow down) adding reactions.
        let k = route_key("GET", u.path());
        assert_eq!(k.route, "GET /channels/{channel_id}/messages/{id}/reactions/{emoji}");
        assert_ne!(k, route_key("PUT", "/api/v9/channels/1/messages/2/reactions/blob:9/@me"));
    }
}
