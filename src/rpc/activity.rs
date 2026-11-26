//! What a game sends in `SET_ACTIVITY` (docs/RPC_DESIGN.md §3.1), checked against the presence
//! limits (§3.2) and turned into op 3's activity. The op 3 JSON is built from the checked
//! fields only, never from the game's JSON.

use crate::auth::Secret;
use crate::model::{Id, RichPresence};
use reqwest::Url;
use serde::Deserialize;
use serde_json::{Map, Value, json};

const INSTANCE: u32 = 1;

/// What the game's own JSON may carry: unknown keys are ignored, wrong types are errors.
#[derive(Deserialize, Default)]
#[serde(default)]
struct Raw {
    #[serde(rename = "type")]
    kind: Option<i64>,
    details: Option<String>,
    state: Option<String>,
    details_url: Option<String>,
    state_url: Option<String>,
    status_display_type: Option<i64>,
    timestamps: Option<Stamps>,
    assets: Option<Assets>,
    party: Option<Party>,
    secrets: Option<Secrets>,
    instance: Option<bool>,
    buttons: Option<Vec<Button>>,
}

/// Floats too: some libraries send `time.time()`.
#[derive(Deserialize, Default)]
#[serde(default)]
struct Stamps {
    start: Option<f64>,
    end: Option<f64>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Assets {
    large_image: Option<String>,
    large_text: Option<String>,
    small_image: Option<String>,
    small_text: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Party {
    id: Option<String>,
    size: Option<(i64, i64)>,
    privacy: Option<i64>,
}

/// `spectate` and `match` are deprecated and left out.
#[derive(Deserialize, Default)]
#[serde(default)]
struct Secrets {
    join: Option<String>,
}

#[derive(Deserialize)]
struct Button {
    label: String,
    url: String,
}

/// "Friends can join my activities" (proto 8.1) and "Voice channel members can join" (8.2).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Privacy {
    pub friends: bool,
    pub voice: bool,
}

/// A game's activity after the checks. `Debug` never shows the join secret (`Secret`).
#[derive(Clone, Debug, PartialEq)]
pub struct Activity {
    pub kind: u8,
    details: Option<String>,
    state: Option<String>,
    details_url: Option<String>,
    state_url: Option<String>,
    status_display_type: Option<u8>,
    /// Unix ms.
    start: Option<u64>,
    end: Option<u64>,
    large_image: Option<String>,
    large_text: Option<String>,
    small_image: Option<String>,
    small_text: Option<String>,
    party_id: Option<String>,
    party_size: Option<(u32, u32)>,
    /// `party.privacy` was 1.
    public: bool,
    join: Option<Secret>,
    instance: bool,
    /// (label, url).
    buttons: Vec<(String, String)>,
}

/// An activity with what op 3 needs besides it: the application and its name, and the privacy
/// settings the flags follow.
#[derive(Clone, Debug, PartialEq)]
pub struct Presence {
    pub app: Id,
    pub name: String,
    pub privacy: Privacy,
    pub activity: Box<Activity>,
}

impl Presence {
    /// The activity as op 3 carries it.
    pub fn op3(&self) -> Value {
        self.activity.op3(self.app, &self.name, self.privacy, true)
    }

    /// As my own cards show it.
    pub fn to_activity(&self) -> crate::model::Activity {
        let a = &self.activity;
        let rich = RichPresence {
            details: a.details.clone(),
            state: a.state.clone(),
            start: a.start,
            end: a.end,
            application_id: Some(self.app),
            large_image: a.large_image.clone(),
            large_text: a.large_text.clone(),
            small_image: a.small_image.clone(),
            small_text: a.small_text.clone(),
            party: a.party_size.map(|(c, m)| [c, m]),
            buttons: a.buttons.iter().map(|b| b.0.clone()).collect(),
            button_urls: a.buttons.iter().map(|b| b.1.clone()).collect(),
            flags: a.flags(self.privacy),
            ..Default::default()
        };
        crate::model::Activity {
            kind: a.kind,
            name: self.name.clone(),
            rich: Some(Box::new(rich)),
        }
    }
}

impl Activity {
    /// `INSTANCE`; `JOIN` with a join secret and a party id; the privacy flags with `JOIN`, a
    /// public party and the matching setting. Unverified against Discord: RPC_DESIGN §10.1.
    pub fn flags(&self, p: Privacy) -> u32 {
        let join = self.join.is_some() && self.party_id.is_some();
        let public = join && self.public;
        let bit = |on: bool, flag: u32| if on { flag } else { 0 };
        bit(self.instance, INSTANCE)
            | bit(join, RichPresence::JOIN)
            | bit(public && p.friends, RichPresence::PARTY_PRIVACY_FRIENDS)
            | bit(public && p.voice, RichPresence::PARTY_PRIVACY_VOICE_CHANNEL)
    }

    /// Op 3's activity (§3.2). `secrets` false: what `SET_ACTIVITY`'s answer echoes to the game
    /// (it knows its secret).
    pub fn op3(&self, app: Id, name: &str, p: Privacy, secrets: bool) -> Value {
        let mut a =
            json!({"type": self.kind, "name": name, "application_id": app, "flags": self.flags(p)});
        let text = |v: &Option<String>| v.as_ref().map(|s| json!(s));
        for (key, v) in [
            ("details", &self.details),
            ("state", &self.state),
            ("details_url", &self.details_url),
            ("state_url", &self.state_url),
        ] {
            if let Some(v) = text(v) {
                a[key] = v;
            }
        }
        if let Some(t) = self.status_display_type {
            a["status_display_type"] = json!(t);
        }
        let parts = [
            (
                "timestamps",
                object([
                    ("start", self.start.map(Value::from)),
                    ("end", self.end.map(Value::from)),
                ]),
            ),
            (
                "assets",
                object([
                    ("large_image", text(&self.large_image)),
                    ("large_text", text(&self.large_text)),
                    ("small_image", text(&self.small_image)),
                    ("small_text", text(&self.small_text)),
                ]),
            ),
            (
                "party",
                object([
                    ("id", text(&self.party_id)),
                    ("size", self.party_size.map(|(c, m)| json!([c, m]))),
                ]),
            ),
            (
                "secrets",
                object([(
                    "join",
                    self.join
                        .as_ref()
                        .filter(|_| secrets)
                        .map(|j| json!(j.expose())),
                )]),
            ),
        ];
        for (key, v) in parts {
            if let Some(v) = v {
                a[key] = v;
            }
        }
        if !self.buttons.is_empty() {
            a["buttons"] = json!(self.buttons.iter().map(|b| &b.0).collect::<Vec<_>>());
            a["metadata"] =
                json!({"button_urls": self.buttons.iter().map(|b| &b.1).collect::<Vec<_>>()});
        }
        a
    }
}

/// The present entries as an object; None when there are none.
fn object<const N: usize>(pairs: [(&str, Option<Value>); N]) -> Option<Value> {
    let map: Map<String, Value> = pairs
        .into_iter()
        .filter_map(|(k, v)| v.map(|v| (k.to_owned(), v)))
        .collect();
    (!map.is_empty()).then_some(Value::Object(map))
}

/// Trimmed, `min..=max` characters; shorter is dropped, longer is an error.
fn text(v: Option<String>, field: &str, min: usize, max: usize) -> Result<Option<String>, String> {
    let Some(s) = v else { return Ok(None) };
    let s = s.trim();
    let n = s.chars().count();
    if n > max {
        return Err(format!("{field} is longer than {max} characters"));
    }
    Ok((n >= min).then(|| s.to_owned()))
}

/// An https URL of at most `max` bytes; empty is none.
fn https(v: Option<String>, field: &str, max: usize) -> Result<Option<String>, String> {
    let Some(s) = v.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    let ok =
        s.len() <= max && Url::parse(&s).is_ok_and(|u| u.scheme() == "https" && u.host().is_some());
    if ok {
        Ok(Some(s))
    } else {
        Err(format!(
            "{field} is not an https URL of at most {max} characters"
        ))
    }
}

/// An asset key or `mp:…` as is. Anything with a scheme is dropped for now.
// shortcut: https:// images are dropped; the media-proxy path (RPC_DESIGN §3.4) is the next slice.
fn image(v: Option<String>) -> Option<String> {
    v.map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty() && s.len() <= 256 && (s.starts_with("mp:") || !s.contains("://")))
}

/// Seconds (below 10^11) or ms to ms; 0 is none.
fn ms(v: Option<f64>, field: &str) -> Result<Option<u64>, String> {
    match v {
        None => Ok(None),
        Some(t) if !t.is_finite() || t < 0.0 => Err(format!("{field} is not a time")),
        Some(0.0) => Ok(None),
        Some(t) => Ok(Some(if t < 1e11 { t * 1000.0 } else { t } as u64)),
    }
}

/// The checks of §3.2 on a game's `activity` object; Err names the field.
// shortcut: a wrong JSON type is reported with serde's message, which names no field; fine until a game developer asks.
pub fn parse(v: &Value) -> Result<Activity, String> {
    let raw = Raw::deserialize(v).map_err(|e| e.to_string())?;
    let kind = match raw.kind.unwrap_or(0) {
        k @ (0 | 2 | 3 | 5) => k as u8,
        _ => return Err("type is not 0, 2, 3 or 5".into()),
    };
    let status_display_type = match raw.status_display_type {
        None => None,
        Some(t @ 0..=2) => Some(t as u8),
        Some(_) => return Err("status_display_type is not 0, 1 or 2".into()),
    };
    let (stamps, assets, party) = (
        raw.timestamps.unwrap_or_default(),
        raw.assets.unwrap_or_default(),
        raw.party.unwrap_or_default(),
    );
    let party_size = match party.size {
        None => None,
        Some((c, m)) if (1..=m).contains(&c) && m <= i64::from(u32::MAX) => {
            Some((c as u32, m as u32))
        }
        Some(_) => return Err("party.size is not [current, max] with 1 <= current <= max".into()),
    };
    // Secrets and ids are kept as they are, not trimmed.
    let keep = |v: Option<String>, field: &str| match v {
        Some(s) if s.chars().count() > 128 => Err(format!("{field} is longer than 128 characters")),
        v => Ok(v.filter(|s| !s.is_empty())),
    };
    let join = keep(raw.secrets.and_then(|s| s.join), "secrets.join")?.map(Secret::new);
    let mut buttons = Vec::new();
    for (i, b) in raw.buttons.unwrap_or_default().into_iter().enumerate() {
        if i == 2 {
            return Err("buttons has more than 2 entries".into());
        }
        let label = text(Some(b.label), &format!("buttons[{i}].label"), 1, 32)?
            .ok_or(format!("buttons[{i}].label is empty"))?;
        buttons.push((
            label,
            https(Some(b.url), &format!("buttons[{i}].url"), 512)?
                .ok_or(format!("buttons[{i}].url is empty"))?,
        ));
    }
    // Discord's reaction to buttons next to a join secret is unverified (RPC_DESIGN §10.3): dropped.
    if join.is_some() {
        buttons.clear();
    }
    Ok(Activity {
        kind,
        details: text(raw.details, "details", 2, 128)?,
        state: text(raw.state, "state", 2, 128)?,
        details_url: https(raw.details_url, "details_url", 256)?,
        state_url: https(raw.state_url, "state_url", 256)?,
        status_display_type,
        start: ms(stamps.start, "timestamps.start")?,
        end: ms(stamps.end, "timestamps.end")?,
        large_image: image(assets.large_image),
        large_text: text(assets.large_text, "assets.large_text", 1, 128)?,
        small_image: image(assets.small_image),
        small_text: text(assets.small_text, "assets.small_text", 1, 128)?,
        party_id: keep(party.id, "party.id")?,
        party_size,
        public: party.privacy == Some(1),
        join,
        instance: raw.instance.unwrap_or(false),
        buttons,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ON: Privacy = Privacy {
        friends: true,
        voice: true,
    };

    fn ok(v: Value) -> Activity {
        parse(&v).unwrap()
    }

    #[test]
    fn discord_rpc_fixture_to_op3() {
        let a = ok(
            json!({"state":"In a match","details":"Ranked · Map 3","timestamps":{"start":1764183000},
            "assets":{"large_image":"map3","large_text":"Map 3","small_image":"rank_gold","small_text":"Gold"},
            "party":{"id":"party-8f2c","size":[2,4],"privacy":1},
            "secrets":{"join":"j-1a2b3c","spectate":"s-x","match":"m-x"},"instance":true,"name":"ignored"}),
        );
        let want = json!({"type":0,"name":"Game","application_id":"42","details":"Ranked · Map 3","state":"In a match",
            "timestamps":{"start":1_764_183_000_000u64},
            "assets":{"large_image":"map3","large_text":"Map 3","small_image":"rank_gold","small_text":"Gold"},
            "party":{"id":"party-8f2c","size":[2,4]},"secrets":{"join":"j-1a2b3c"},
            "flags":1 | 2 | 64 | 128});
        assert_eq!(a.op3(Id(42), "Game", ON, true), want);
        // The answer to the game has no secret and no name.
        let echo = a.op3(Id(42), "", ON, false);
        assert!(echo.get("secrets").is_none() && echo["name"] == "");
        assert!(
            !format!("{a:?}").contains("j-1a2b3c"),
            "the secret stays out of Debug"
        );
        // Privacy flags follow the settings, and only with JOIN.
        assert_eq!(
            a.flags(Privacy {
                friends: false,
                voice: true
            }),
            1 | 2 | 128
        );
        assert_eq!(
            a.flags(Privacy {
                friends: false,
                voice: false
            }),
            1 | 2
        );
    }

    #[test]
    fn pypresence_fixture_with_ms_buttons_and_flags() {
        let a = ok(
            json!({"state":"Playing","timestamps":{"start":1764183000123u64,"end":1764183900.0},"type":3,
            "buttons":[{"label":"Site","url":"https://example.com/a"},{"label":" Wiki ","url":"https://example.com/b"}]}),
        );
        let j = a.op3(Id(1), "Show", ON, true);
        assert_eq!(
            (&j["type"], &j["timestamps"]),
            (
                &json!(3),
                &json!({"start":1_764_183_000_123u64,"end":1_764_183_900_000u64})
            )
        );
        assert_eq!(
            (&j["buttons"], &j["metadata"]),
            (
                &json!(["Site", "Wiki"]),
                &json!({"button_urls":["https://example.com/a","https://example.com/b"]})
            )
        );
        assert_eq!(j["flags"], 0);
        // A join secret drops the buttons; JOIN needs the party id too; privacy needs privacy 1.
        let with = |party: Value| {
            ok(
                json!({"buttons":[{"label":"a","url":"https://x.example"}],"secrets":{"join":"s"},"party":party}),
            )
        };
        assert!(
            with(json!({"id":"p","size":[1,2],"privacy":0}))
                .op3(Id(1), "x", ON, true)
                .get("buttons")
                .is_none()
        );
        assert_eq!(
            with(json!({"id":"p","privacy":0})).flags(ON),
            RichPresence::JOIN
        );
        assert_eq!(
            with(json!({"size":[1,2],"privacy":1})).flags(ON),
            0,
            "no party id, no JOIN"
        );
        assert_eq!(ok(json!({"instance":true})).flags(ON), 1);
    }

    #[test]
    fn limits_and_odd_values() {
        // Short texts, zero times, scheme images and the game's name go; seconds become ms.
        let a = ok(
            json!({"details":"a","state":"  ok  ","timestamps":{"start":0,"end":99_999_999_999u64},
            "assets":{"large_image":"https://x.example/a.png","small_image":"mp:external/abc","large_text":""}}),
        );
        let j = a.op3(Id(1), "G", ON, true);
        assert_eq!((j.get("details"), &j["state"]), (None, &json!("ok")));
        assert_eq!(
            (&j["timestamps"], &j["assets"]),
            (
                &json!({"end":99_999_999_999_000u64}),
                &json!({"small_image":"mp:external/abc"})
            )
        );
        for (bad, field) in [
            (json!({"type":1}), "type"),
            (json!({"type":4}), "type"),
            (json!({"state":"x".repeat(129)}), "state"),
            (json!({"details_url":"http://x.example"}), "details_url"),
            (
                json!({"state_url":"https://x.example/".to_owned() + &"a".repeat(256)}),
                "state_url",
            ),
            (json!({"status_display_type":3}), "status_display_type"),
            (json!({"timestamps":{"start":-1}}), "timestamps.start"),
            (json!({"party":{"size":[3,2]}}), "party.size"),
            (json!({"party":{"size":[0,2]}}), "party.size"),
            (json!({"party":{"id":"p".repeat(129)}}), "party.id"),
            (json!({"secrets":{"join":"s".repeat(129)}}), "secrets.join"),
            (
                json!({"assets":{"small_text":"t".repeat(129)}}),
                "small_text",
            ),
            (
                json!({"buttons":[{"label":"","url":"https://x.example"}]}),
                "label",
            ),
            (
                json!({"buttons":[{"label":"a","url":"ftp://x.example"}]}),
                "url",
            ),
            (
                json!({"buttons":[{"label":"a","url":"https://x.example"},{"label":"b","url":"https://x.example"},{"label":"c","url":"https://x.example"}]}),
                "more than 2",
            ),
            (
                json!({"buttons":[{"label":"x".repeat(33),"url":"https://x.example"}]}),
                "label",
            ),
        ] {
            let e = parse(&bad).unwrap_err();
            assert!(e.contains(field), "{bad}: {e}");
        }
        assert!(
            parse(&json!({"state":5})).is_err(),
            "wrong types are errors"
        );
        assert_eq!(ok(json!({"type":5,"status_display_type":2})).kind, 5);
        assert_eq!(
            ok(json!({})).op3(Id(1), "G", ON, true),
            json!({"type":0,"name":"G","application_id":"1","flags":0})
        );
    }

    #[test]
    fn my_card_shows_the_rich_presence() {
        let a = ok(
            json!({"state":"In a match","timestamps":{"start":5},"party":{"id":"p","size":[2,4]},"buttons":[{"label":"Site","url":"https://x.example"}]}),
        );
        let m = Presence {
            app: Id(9),
            name: "Game".into(),
            privacy: ON,
            activity: Box::new(a),
        }
        .to_activity();
        let r = m.rich.as_deref().unwrap();
        assert_eq!(
            (m.kind, m.name.as_str(), r.application_id, r.start, r.party),
            (0, "Game", Some(Id(9)), Some(5000), Some([2, 4]))
        );
        assert_eq!(
            (&r.buttons[..], &r.button_urls[..]),
            (
                &["Site".to_owned()][..],
                &["https://x.example".to_owned()][..]
            )
        );
    }
}
