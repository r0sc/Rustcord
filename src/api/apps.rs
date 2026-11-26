//! Game detection and activity icons (crate::activity): the detectable-games list and public
//! application metadata (icons), both GETs the official client makes too, through the limiter.
//!
//! OWNERSHIP: activities engineer.

use super::{ApiClient, ApiError, net_err};
use crate::model::Id;
use reqwest::{Method, Url};
use serde::Deserialize;

/// Application ids per `/applications/public` request.
// shortcut: 25 like `/games`; the server's own limit is unconfirmed.
pub const MAX_APP_IDS: usize = 25;

/// `/applications/public` entry; only the icon is used.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct PublicApp {
    pub id: Id,
    pub icon: Option<String>,
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
}

/// None for a session id that is not plain letters and digits (it comes from other users).
fn metadata_url(user: Id, session: &str, app: Id) -> Option<Url> {
    let plain = !session.is_empty() && session.len() <= 64 && session.bytes().all(|b| b.is_ascii_alphanumeric());
    plain.then(|| ApiClient::url(&["users", &user.to_string(), "sessions", session, "activities", &app.to_string(), "metadata"]))
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
}
