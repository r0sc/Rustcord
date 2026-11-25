//! DM / group DM calls (VOICE_NOTES §1.4): which DMs have a call going on and who is being rung
//! (CALL_CREATE / CALL_UPDATE / CALL_DELETE), incoming calls, and the steps of a call I start.
//! Who is *in* a call is `AppState::voice_states[dm id]` (guild_id None), like voice channels.
//! Every network step here comes from a click: starting rings once, for a call that did not
//! exist yet; nothing re-rings, accepts or declines on its own.
//!
//! OWNERSHIP: dm-calls engineer.

use super::{AppState, Notification};
use crate::model::{Id, channel_type, relationship_type};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

/// How long a started call waits for the voice task to take it up (it gives up after 10 s).
const START_WAIT: Duration = Duration::from_secs(15);

/// `AppState::calls`. Not cached; rebuilt from the CALL_CREATEs after READY.
#[derive(Debug, Default)]
pub struct CallState {
    /// Calls going on, by DM / group DM id -> users being rung.
    pub ringing: HashMap<Id, Vec<Id>>,
    /// Incoming calls I declined here, until the server stops ringing me.
    declined: HashSet<Id>,
    /// A call I started or accepted with a click, while its follow-ups are due.
    start: Option<Start>,
}

#[derive(Debug)]
struct Start {
    channel_id: Id,
    /// Ring the others once the call exists: Some(check `ringable` first).
    ring: Option<bool>,
    /// Turn my camera on once connected (video call).
    video: bool,
    /// The voice task took the call up (`voice.channel_id`).
    joined: bool,
    at: Instant,
}

/// Commands a started call needs now (`AppState::call_steps`).
#[derive(Debug, Default, PartialEq)]
pub struct CallSteps {
    /// Ring everyone in (channel, check `ringable` first).
    pub ring: Option<(Id, bool)>,
    /// Start my camera.
    pub camera: bool,
}

impl AppState {
    fn my_id(&self) -> Option<Id> {
        self.me.as_ref().map(|u| u.id)
    }

    /// A call is going on in this DM / group DM.
    pub fn call_in(&self, channel_id: Id) -> bool {
        self.calls.ringing.contains_key(&channel_id)
    }

    /// A call ringing me that I neither joined nor declined (lowest channel id first).
    pub fn incoming_call(&self) -> Option<Id> {
        let me = self.my_id()?;
        let c = &self.calls;
        c.ringing
            .iter()
            .filter(|(ch, ring)| ring.contains(&me) && !c.declined.contains(ch) && self.voice.channel_id != Some(**ch))
            .map(|(ch, _)| *ch)
            .min()
    }

    /// Who calls: someone already in the call, else the other user of a 1:1 DM.
    pub fn caller(&self, channel_id: Id) -> Option<Id> {
        let me = self.my_id();
        let in_call = self.voice_states.get(&channel_id).and_then(|m| m.keys().filter(|u| Some(**u) != me).min().copied());
        in_call.or_else(|| self.channels.get(&channel_id).filter(|c| c.kind == channel_type::DM)?.recipient_ids.first().copied())
    }

    /// The user clicked call / video call / accept in this DM; the caller sends `JoinVoice`
    /// (guild_id None) right after. A call that did not exist yet rings the others once it
    /// does (VOICE_NOTES §1.4: group always, 1:1 friends directly, others if `ringable`).
    pub fn start_call(&mut self, channel_id: Id, video: bool, now: Instant) {
        let ring = (!self.call_in(channel_id)).then(|| {
            let ch = self.channels.get(&channel_id);
            let other = ch.filter(|c| c.kind == channel_type::DM).and_then(|c| c.recipient_ids.first());
            other.is_some_and(|u| self.social.relationships.get(u).is_none_or(|r| r.kind != relationship_type::FRIEND))
        });
        self.calls.declined.remove(&channel_id);
        self.calls.start = Some(Start { channel_id, ring, video, joined: false, at: now });
    }

    /// A call I started or accepted is still on its way (the voice task may not have joined
    /// it yet); `call_steps` drops it within 15 s. The updater counts it as busy.
    pub fn call_starting(&self) -> bool {
        self.calls.start.is_some()
    }

    /// The user declined the incoming call here (the caller sends `StopRinging`).
    pub fn decline_call(&mut self, channel_id: Id) {
        self.calls.declined.insert(channel_id);
    }

    /// Once per frame: the ring and camera a started call is due for. Dropped once done, when
    /// I leave that call, or when the voice task never took it up.
    pub fn call_steps(&mut self, now: Instant) -> CallSteps {
        let mut steps = CallSteps::default();
        let Some(s) = &mut self.calls.start else { return steps };
        let here = self.voice.channel_id == Some(s.channel_id);
        s.joined |= here;
        if !here {
            if s.joined || now.duration_since(s.at) > START_WAIT {
                self.calls.start = None;
            }
            return steps;
        }
        if self.calls.ringing.contains_key(&s.channel_id) {
            steps.ring = s.ring.take().map(|check| (s.channel_id, check));
        }
        if s.video && self.voice.connected {
            (s.video, steps.camera) = (false, true);
        }
        if s.ring.is_none() && !s.video {
            self.calls.start = None;
        }
        steps
    }

    /// CALL_CREATE / CALL_UPDATE. A desktop notification when it starts ringing me.
    pub(super) fn on_call_update(&mut self, channel_id: Id, ringing: Vec<Id>) -> Option<Notification> {
        let me = self.my_id()?;
        let was = self.calls.ringing.get(&channel_id).is_some_and(|r| r.contains(&me));
        let now = ringing.contains(&me);
        if !now {
            self.calls.declined.remove(&channel_id);
        }
        self.calls.ringing.insert(channel_id, ringing);
        (now && !was && self.incoming_call().is_some()).then(|| Notification {
            channel_id,
            title: self.channel_name(channel_id),
            body: "Incoming call".into(),
            author_id: None,
            mentioned: false,
        })
    }

    /// CALL_DELETE: the call ended (its voice states went with VOICE_STATE_UPDATEs).
    pub(super) fn on_call_delete(&mut self, channel_id: Id) {
        self.calls.ringing.remove(&channel_id);
        self.calls.declined.remove(&channel_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Channel, GatewayEvent, Relationship, User, VoiceState};

    const ME: Id = Id(1);
    const BOB: Id = Id(2);
    const DM: Id = Id(10);
    const GROUP: Id = Id(11);

    fn state() -> AppState {
        let mut s = AppState { me: Some(User { id: ME, ..Default::default() }), ..Default::default() };
        let dm = Channel { id: DM, kind: channel_type::DM, recipient_ids: vec![BOB], ..Default::default() };
        let group = Channel { id: GROUP, kind: channel_type::GROUP_DM, recipient_ids: vec![BOB, Id(3)], ..Default::default() };
        for c in [dm, group] {
            s.apply_gateway(GatewayEvent::ChannelCreate(Box::new(c)), None);
        }
        s
    }

    fn call(s: &mut AppState, ch: Id, ringing: &[Id]) -> Option<Notification> {
        s.apply_gateway(GatewayEvent::CallUpdate { channel_id: ch, ringing: ringing.to_vec() }, None)
    }

    #[test]
    fn incoming_rings_notifies_once_and_ends() {
        let mut s = state();
        let vs = VoiceState { channel_id: Some(DM), user_id: BOB, ..Default::default() };
        s.apply_gateway(GatewayEvent::VoiceStateUpdate(Box::new(vs)), None);
        assert!(call(&mut s, DM, &[ME]).is_some(), "starts ringing me");
        assert!(call(&mut s, DM, &[ME]).is_none(), "an update while ringing does not notify again");
        assert_eq!((s.incoming_call(), s.caller(DM)), (Some(DM), Some(BOB)));
        assert!(s.call_in(DM) && !s.call_in(GROUP));
        // Declined here: the popup goes at once; the server's update confirms it.
        s.decline_call(DM);
        assert_eq!(s.incoming_call(), None);
        assert!(call(&mut s, DM, &[]).is_none());
        // Rung again later: shows again.
        assert!(call(&mut s, DM, &[ME]).is_some());
        assert_eq!(s.incoming_call(), Some(DM));
        // Joined (here or the call ended): no popup.
        s.voice.channel_id = Some(DM);
        assert_eq!(s.incoming_call(), None);
        s.voice.channel_id = None;
        s.apply_gateway(GatewayEvent::CallDelete { channel_id: DM }, None);
        assert!(!s.call_in(DM) && s.incoming_call().is_none());
        // Ringing someone else only; a group call names the group.
        assert!(call(&mut s, GROUP, &[Id(3)]).is_none());
        assert_eq!(s.caller(GROUP), None);
        // A new session forgets every call (READY is followed by fresh CALL_CREATEs).
        s.apply_gateway(GatewayEvent::Ready(Box::default()), None);
        assert!(s.calls.ringing.is_empty());
    }

    #[test]
    fn a_started_call_rings_once_then_turns_the_camera_on() {
        let t0 = Instant::now();
        let mut s = state();
        s.start_call(DM, true, t0);
        assert_eq!(s.call_steps(t0), CallSteps::default(), "voice task not there yet");
        s.voice.channel_id = Some(DM);
        assert_eq!(s.call_steps(t0), CallSteps::default(), "no call yet: nothing to ring");
        call(&mut s, DM, &[]);
        assert_eq!(s.call_steps(t0), CallSteps { ring: Some((DM, true)), camera: false }, "not a friend: ringable check");
        assert_eq!(s.call_steps(t0), CallSteps::default(), "never rings twice");
        s.voice.connected = true;
        assert_eq!(s.call_steps(t0), CallSteps { ring: None, camera: true });
        assert!(s.calls.start.is_none());

        // Friend: direct ring; group: everyone, no check; an existing call: join only.
        let mut s = state();
        let friend: Relationship = serde_json::from_str(r#"{"id":"2","type":1}"#).unwrap();
        s.social.relationships.insert(BOB, friend);
        s.start_call(DM, false, t0);
        assert_eq!(s.calls.start.as_ref().and_then(|x| x.ring), Some(false));
        s.start_call(GROUP, false, t0);
        assert_eq!(s.calls.start.as_ref().and_then(|x| x.ring), Some(false));
        call(&mut s, GROUP, &[]);
        s.start_call(GROUP, false, t0);
        assert!(s.calls.start.as_ref().is_some_and(|x| x.ring.is_none()));
        s.voice.channel_id = Some(GROUP);
        assert_eq!(s.call_steps(t0), CallSteps::default());
        assert!(s.calls.start.is_none(), "nothing left to do");

        // Left before the call came up / the voice task never took it: forgotten.
        let mut s = state();
        s.start_call(DM, true, t0);
        s.voice.channel_id = Some(DM);
        s.call_steps(t0);
        s.voice.channel_id = None;
        s.call_steps(t0);
        assert!(s.calls.start.is_none());
        s.start_call(DM, true, t0);
        s.call_steps(t0 + START_WAIT);
        assert!(s.calls.start.is_some());
        s.call_steps(t0 + START_WAIT + Duration::from_secs(1));
        assert!(s.calls.start.is_none());
    }
}
