//! Game detection and activity icons (crate::activity): the detectable-games list and public
//! application metadata (icons), both GETs the official client makes too, through the limiter.
//!
//! OWNERSHIP: activities engineer.

use super::{ApiClient, ApiError, net_err, send_body};
use crate::auth::Secret;
use crate::model::Id;
use reqwest::{Method, Url};
use serde::Deserialize;

/// Application ids per `/applications/public` request.
// shortcut: 25 like `/games`; the server's own limit is unconfirmed.
pub const MAX_APP_IDS: usize = 25;

/// `/applications/public` entry; the icon and the name are used.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct PublicApp {
    pub id: Id,
    pub icon: Option<String>,
    pub name: Option<String>,
}

impl ApiClient {
    /// `GET /games/detectable` (the official client's detection list, ~13 MB of JSON): the raw
    /// body, parsed off the async workers by the caller.
    pub async fn detectable_games(&self) -> Result<Vec<u8>, ApiError> {
        let resp = self.send(Method::GET, Self::url(&["games", "detectable"]), None).await?;
        Ok(resp.bytes().await.map_err(net_err)?.to_vec())
    }

    /// `GET /users/{user}/sessions/{session}/activities/{app}/metadata`: the URLs of someone's
    /// rich-presence buttons (the presence carries only the labels); 204 = none.
    pub async fn activity_button_urls(&self, user: Id, session: &str, app: Id) -> Result<Vec<String>, ApiError> {
        let Some(url) = metadata_url(user, session, app) else { return Ok(Vec::new()) };
        let resp = self.send(Method::GET, url, None).await?;
        let bytes = resp.bytes().await.map_err(net_err)?;
        Ok(button_urls(&bytes))
    }

    /// `GET /applications/public?application_ids=<id>&…` (1-25 ids); unknown ids are left out.
    pub async fn public_apps(&self, ids: &[Id]) -> Result<Vec<PublicApp>, ApiError> {
        self.send_json(Method::GET, public_apps_url(ids), None).await
    }

    /// `GET /users/{user}/sessions/{session}/activities/{app}/1` (+ `?channel_id&message_id` for a
    /// rich-presence invite): the secret that joins their activity, handed to my game.
    pub async fn activity_secret(&self, user: Id, session: &str, app: Id, invite: Option<(Id, Id)>) -> Result<Secret, ApiError> {
        let url = secret_url(user, session, app, invite).ok_or(ApiError::NotFound)?;
        secret_of(&self.send(Method::GET, url, None).await?.bytes().await.map_err(net_err)?)
    }

    /// Ask to Join: `POST /channels/{dm}/messages` with a type 5 activity naming my gateway session
    /// and their party (the DM comes from `open_dm`).
    pub async fn ask_to_join(&self, dm: Id, app: Id, my_session: &str, party_id: &str, nonce: &str) -> Result<(), ApiError> {
        let url = Self::url(&["channels", &dm.to_string(), "messages"]);
        self.send(Method::POST, url, Some(ask_body(dm, app, my_session, party_id, nonce))).await.map(drop)
    }

    /// `POST /applications/{app}/external-assets {"urls":[…]}` (1-2 https URLs): the media-proxy
    /// path of each one it accepted, as `(url, path)`. A path that is not plain is left out.
    pub async fn external_assets(&self, app: Id, urls: &[String]) -> Result<Vec<(String, String)>, ApiError> {
        let url = Self::url(&["applications", &app.to_string(), "external-assets"]);
        let answer: Vec<ExternalAsset> = self.send_json(Method::POST, url, Some(serde_json::json!({ "urls": urls }))).await?;
        Ok(proxied_paths(answer, urls))
    }
}

#[derive(Deserialize)]
struct ExternalAsset {
    url: String,
    external_asset_path: Option<String>,
}

/// What Discord answers for the asked `urls`, minus anything that is not a plain relative path
/// (it becomes `mp:<path>` in my presence, so it is Discord's text only after this check).
fn proxied_paths(answer: Vec<ExternalAsset>, asked: &[String]) -> Vec<(String, String)> {
    let plain = |p: &str| !p.is_empty() && p.len() <= 512 && p.bytes().all(|b| b.is_ascii_graphic()) && !p.starts_with('/') && !p.contains("..");
    answer.into_iter().filter_map(|a| Some((a.url, a.external_asset_path?))).filter(|(u, p)| asked.contains(u) && plain(p)).collect()
}

/// None for a session id that is not plain letters and digits (it comes from other users).
fn metadata_url(user: Id, session: &str, app: Id) -> Option<Url> {
    let plain = !session.is_empty() && session.len() <= 64 && session.bytes().all(|b| b.is_ascii_alphanumeric());
    plain.then(|| ApiClient::url(&["users", &user.to_string(), "sessions", session, "activities", &app.to_string(), "metadata"]))
}

/// The secret GET's URL; None for a session id that is not plain (as `metadata_url`).
fn secret_url(user: Id, session: &str, app: Id, invite: Option<(Id, Id)>) -> Option<Url> {
    let mut url = metadata_url(user, session, app)?;
    url.path_segments_mut().ok()?.pop().push("1");
    if let Some((channel, message)) = invite {
        url.query_pairs_mut().append_pair("channel_id", &channel.to_string()).append_pair("message_id", &message.to_string());
    }
    Some(url)
}

/// `{"secret": "…"}` as a `Secret` from the start (never logged; an error names no content).
fn secret_of(body: &[u8]) -> Result<Secret, ApiError> {
    #[derive(Deserialize)]
    struct Wire {
        secret: String,
    }
    let w: Wire = serde_json::from_slice(body).map_err(|_| ApiError::Decode("no secret".into()))?;
    Some(w.secret).filter(|s| !s.is_empty()).map(Secret::new).ok_or_else(|| ApiError::Decode("no secret".into()))
}

fn ask_body(dm: Id, app: Id, my_session: &str, party_id: &str, nonce: &str) -> serde_json::Value {
    let mut body = send_body(dm, "", nonce, None);
    body["application_id"] = serde_json::json!(app);
    body["activity"] = serde_json::json!({"type": 5, "session_id": my_session, "party_id": party_id});
    body
}

/// `button_urls` of a metadata answer, leniently (an empty 204 body, odd entries: none).
fn button_urls(body: &[u8]) -> Vec<String> {
    let v: serde_json::Value = serde_json::from_slice(body).unwrap_or_default();
    v["button_urls"].as_array().into_iter().flatten().filter_map(|u| u.as_str().map(str::to_owned)).take(2).collect()
}

fn public_apps_url(ids: &[Id]) -> Url {
    let mut url = ApiClient::url(&["applications", "public"]);
    url.query_pairs_mut().extend_pairs(ids.iter().take(MAX_APP_IDS).map(|id| ("application_ids", id.to_string())));
    url
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes() {
        let u = public_apps_url(&(1..=30).map(Id).collect::<Vec<_>>());
        assert!(u.as_str().contains("/applications/public?application_ids=1&application_ids=2&"), "{u}");
        assert!(u.as_str().ends_with("&application_ids=25"), "{u}");
        let apps: Vec<PublicApp> = serde_json::from_str(r#"[{"id":"7","name":"x","icon":"abc"},{"id":"8","icon":null}]"#).unwrap();
        assert_eq!((apps[0].id, apps[0].icon.as_deref(), apps[1].icon.as_deref()), (Id(7), Some("abc"), None));
        let url = metadata_url(Id(2), "c0ffee42", Id(8)).unwrap();
        assert!(url.as_str().ends_with("/users/2/sessions/c0ffee42/activities/8/metadata"), "{url}");
        for bad in ["", "..", "a/b", "a?b", &"x".repeat(65)] {
            assert!(metadata_url(Id(2), bad, Id(8)).is_none(), "{bad}");
        }
        assert_eq!(button_urls(br#"{"button_urls":["https://a.example",5,"https://b.example","https://c.example"]}"#), ["https://a.example", "https://b.example"]);
        assert!(button_urls(b"").is_empty() && button_urls(b"{}").is_empty());
    }

    #[test]
    fn proxy_answers_are_checked() {
        let asked = ["https://a.example/1.png".to_owned(), "https://a.example/2.png".to_owned()];
        let answer = |url: &str, p: Option<&str>| vec![ExternalAsset { url: url.into(), external_asset_path: p.map(Into::into) }];
        let ok = proxied_paths(answer("https://a.example/1.png", Some("app/https/a.example/1.png")), &asked);
        assert_eq!(ok, [("https://a.example/1.png".to_owned(), "app/https/a.example/1.png".to_owned())]);
        let long = "x".repeat(513);
        for bad in [None, Some(""), Some("/abs"), Some("a/../b"), Some("a b"), Some("a\nb"), Some("é"), Some(long.as_str())] {
            assert!(proxied_paths(answer("https://a.example/2.png", bad), &asked).is_empty(), "{bad:?}");
        }
        assert!(proxied_paths(answer("https://other.example/x", Some("p/x")), &asked).is_empty(), "an answer for a URL nobody asked");
        let wire: Vec<ExternalAsset> = serde_json::from_str(r#"[{"url":"u","external_asset_path":"p"},{"url":"v","external_asset_path":null}]"#).unwrap();
        assert_eq!((wire[0].external_asset_path.as_deref(), wire[1].external_asset_path.as_deref()), (Some("p"), None));
    }
}
