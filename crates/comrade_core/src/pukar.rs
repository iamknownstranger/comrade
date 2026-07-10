/*!
 * Pukar — real-time audio/video call engine (Telegram-style calls).
 *
 * Architecture: the actual media transport is WebRTC, owned by the *platform*
 * layer (Android `org.webrtc`, or the Tauri webview's built-in WebRTC). This
 * module owns everything else:
 *
 *  1. **Signaling transport** — call-control messages ([`CallSignal`]) travel
 *     as Nostr **ephemeral** events (kind [`SIGNAL_KIND`], in the 20000–29999
 *     range relays do not store), with the payload NIP-04-encrypted to the
 *     peer — the same crypto path as the Vault DM engine. Relays see only an
 *     opaque blob addressed to a pubkey; nothing about the call persists.
 *  2. **Call state machine** — [`CallManager`] is pure (clock injected, no
 *     I/O) and fully unit-tested: ringing, accept/reject, busy handling, ring
 *     timeouts, hang-up, and a bounded call log.
 *  3. **Network engine** — [`PukarEngine`] mirrors `VaultEngine`: it
 *     subscribes to signaling events addressed to us, decrypts + validates
 *     them, drives the [`CallManager`], sends auto-replies (e.g. `busy`), and
 *     surfaces [`CallEvent`]s for the UI bridge.
 *
 * SDP offers/answers and ICE candidates are treated as **opaque strings**
 * produced and consumed by the platform's WebRTC stack; the core never
 * interprets them.
 */

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use nostr_sdk::nips::nip44;
use nostr_sdk::prelude::*;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::error::PukarError;

// ── Constants ─────────────────────────────────────────────────────────────────

/// Ephemeral Nostr kind used for call signaling. Kinds 20000–29999 are
/// ephemeral per NIP-01: relays forward but do not store them.
pub const SIGNAL_KIND: u16 = 25050;

/// Envelope version — bump on breaking payload changes.
pub const SIGNAL_VERSION: u32 = 1;

/// How long an unanswered call rings before timing out (Telegram rings ~50 s).
pub const DEFAULT_RING_TIMEOUT_SECS: u64 = 60;

/// How long a call may sit in `Connecting` (answer exchanged, ICE running)
/// before it is declared failed — without this a lost signal or dead media
/// path would wedge the single call slot forever.
pub const CONNECT_TIMEOUT_SECS: u64 = 30;

/// Signals whose event timestamp is older than this are dropped as
/// stale/replayed. Ephemeral events are not stored by honest relays, but a
/// hostile relay could replay one — freshness bounds the window.
pub const SIGNAL_MAX_AGE_SECS: u64 = 120;

/// Maximum tolerated future clock skew on a signal's timestamp. Kept tight:
/// honest peers' events are never meaningfully in the future, and a wide
/// future window would extend the replay budget.
pub const SIGNAL_MAX_FUTURE_SKEW_SECS: u64 = 10;

/// Maximum ended calls retained in the in-memory call log.
const CALL_LOG_CAP: usize = 200;

/// Maximum buffered out-of-order ICE candidates, and how long they are held.
/// With a multi-relay pool, ICE can arrive via a fast relay before the Offer
/// arrives via a slow one — dropping it would make call setup flaky.
const ICE_BUFFER_CAP: usize = 64;
const ICE_BUFFER_TTL_SECS: u64 = 60;

// ── Wire types ────────────────────────────────────────────────────────────────

/// What kind of media the call carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallMedia {
    Audio,
    Video,
}

/// Why a callee refused an incoming call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RejectReason {
    Declined,
    Busy,
    Unsupported,
}

/// Why an established (or ringing) call ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndReason {
    Hangup,
    Timeout,
    Failed,
}

/// A call-control message exchanged between the two peers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CallSignal {
    /// Caller → callee: start ringing. `sdp` is the WebRTC offer.
    Offer {
        call_id: String,
        media: CallMedia,
        sdp: String,
    },
    /// Callee → caller: call accepted. `sdp` is the WebRTC answer.
    Answer { call_id: String, sdp: String },
    /// Either direction: trickle-ICE candidate.
    Ice {
        call_id: String,
        candidate: String,
        sdp_mid: Option<String>,
        sdp_mline_index: Option<u32>,
    },
    /// Callee → caller: refused while ringing.
    Reject {
        call_id: String,
        reason: RejectReason,
    },
    /// Either direction: terminate (ringing or active).
    End { call_id: String, reason: EndReason },
}

impl CallSignal {
    pub fn call_id(&self) -> &str {
        match self {
            CallSignal::Offer { call_id, .. }
            | CallSignal::Answer { call_id, .. }
            | CallSignal::Ice { call_id, .. }
            | CallSignal::Reject { call_id, .. }
            | CallSignal::End { call_id, .. } => call_id,
        }
    }
}

/// Versioned envelope so signaling payloads are self-identifying inside the
/// encrypted content and unrelated DMs are never misparsed as call signals.
#[derive(Debug, Serialize, Deserialize)]
struct SignalEnvelope {
    comrade_pukar: u32,
    signal: CallSignal,
}

/// Serialize a signal into its versioned wire envelope (pre-encryption).
pub fn encode_signal(signal: &CallSignal) -> Result<String, PukarError> {
    serde_json::to_string(&SignalEnvelope {
        comrade_pukar: SIGNAL_VERSION,
        signal: signal.clone(),
    })
    .map_err(|e| PukarError::Malformed(e.to_string()))
}

/// Try to parse decrypted content as a call signal.
///
/// Returns `None` when the content is not a Pukar envelope at all (an ordinary
/// DM), and `Some(Err(_))` when it claims to be one but is malformed or from
/// an unsupported version.
pub fn parse_signal(content: &str) -> Option<Result<CallSignal, PukarError>> {
    let value: serde_json::Value = serde_json::from_str(content).ok()?;
    let version = value.get("comrade_pukar")?;
    let Some(version) = version.as_u64() else {
        return Some(Err(PukarError::Malformed("non-numeric version".into())));
    };
    if version != u64::from(SIGNAL_VERSION) {
        return Some(Err(PukarError::UnsupportedVersion(version)));
    }
    match serde_json::from_value::<SignalEnvelope>(value) {
        Ok(env) => Some(Ok(env.signal)),
        Err(e) => Some(Err(PukarError::Malformed(e.to_string()))),
    }
}

// ── Sessions ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallDirection {
    Outgoing,
    Incoming,
}

/// Lifecycle of a call. `Ended` carries its cause in [`CallSession::cause`]
/// (kept as a separate field so the JSON crossing the FFI stays flat).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallState {
    /// Offer sent (outgoing) or received (incoming); waiting for a human.
    Ringing,
    /// Answer exchanged; platform WebRTC is completing ICE.
    Connecting,
    /// Media flowing.
    Active,
    Ended,
}

/// Why a call reached [`CallState::Ended`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndCause {
    /// Normal hang-up after connecting.
    Completed,
    /// Callee declined (their side: `Declined`; our incoming we rejected).
    Declined,
    /// Callee was on another call.
    PeerBusy,
    /// Incoming call that rang out or was cancelled by the caller.
    Missed,
    /// Outgoing call nobody answered.
    NoAnswer,
    /// Caller cancelled while ringing.
    Cancelled,
    /// Transport/protocol failure.
    Failed,
}

/// One call, live or ended. Peers are identified by hex pubkey.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallSession {
    pub call_id: String,
    /// Remote peer's public key (hex).
    pub peer: String,
    pub media: CallMedia,
    pub direction: CallDirection,
    pub state: CallState,
    pub cause: Option<EndCause>,
    /// Unix seconds when the call started ringing.
    pub started_at: u64,
    /// Unix seconds when the answer was exchanged (entered `Connecting`).
    #[serde(default)]
    pub connecting_since: Option<u64>,
    /// Unix seconds when media was established, if it ever was.
    pub connected_at: Option<u64>,
    pub ended_at: Option<u64>,
}

impl CallSession {
    /// Talk time in seconds, if the call connected.
    pub fn duration_secs(&self) -> Option<u64> {
        Some(self.ended_at?.saturating_sub(self.connected_at?))
    }
}

/// A state change the UI layer needs to react to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CallEvent {
    /// Someone is calling us — start ringing UI; `sdp_offer` feeds WebRTC.
    IncomingCall {
        call: CallSession,
        sdp_offer: String,
    },
    /// Our outgoing call was accepted — apply the answer SDP.
    CallAnswered { call_id: String, sdp_answer: String },
    /// Remote trickle-ICE candidate — add to the peer connection.
    RemoteIce {
        call_id: String,
        candidate: String,
        sdp_mid: Option<String>,
        sdp_mline_index: Option<u32>,
    },
    /// Media established (platform reported ICE completion).
    CallConnected { call_id: String },
    /// The call is over; `call.cause` says why.
    CallEnded { call: CallSession },
}

// ── Pure call state machine ───────────────────────────────────────────────────

/// Who is allowed to make this device ring.
///
/// Anyone on the network can address us an Offer; without gating, any pubkey
/// can ring a stranger's phone — an unacceptable harassment vector for this
/// app's audience. Offers from disallowed callers are **silently dropped**
/// (no busy reply: answering at all is a presence oracle).
///
/// The default is `ContactsOnly` with an **empty** set — deny-by-default.
/// The runtime installs the user's saved contacts on unlock; an app that
/// wants open inbound calling must opt in to [`CallPolicy::AllowAll`].
#[derive(Debug, Clone)]
pub enum CallPolicy {
    /// Ring for anyone (explicit opt-in).
    AllowAll,
    /// Ring only for the listed hex pubkeys (e.g. saved contacts).
    ContactsOnly(std::collections::HashSet<String>),
}

impl Default for CallPolicy {
    fn default() -> Self {
        CallPolicy::ContactsOnly(std::collections::HashSet::new())
    }
}

impl CallPolicy {
    fn allows(&self, peer_hex: &str) -> bool {
        match self {
            CallPolicy::AllowAll => true,
            CallPolicy::ContactsOnly(set) => set.contains(peer_hex),
        }
    }
}

/// An ICE candidate that arrived before its session was ready for it
/// (cross-relay reordering, or trickle-ICE landing while we are still
/// ringing). Held briefly and flushed once the call reaches `Connecting`.
#[derive(Debug, Clone)]
struct PendingIce {
    received_at: u64,
    from_peer: String,
    call_id: String,
    candidate: String,
    sdp_mid: Option<String>,
    sdp_mline_index: Option<u32>,
}

/// Deterministic, I/O-free call manager. One active call at a time (like
/// Telegram): a second incoming offer is auto-refused with `busy`. The clock
/// is injected (`now` = unix seconds) so every path is unit-testable.
#[derive(Debug, Default)]
pub struct CallManager {
    active: Option<CallSession>,
    /// Ended calls, newest last, capped at [`CALL_LOG_CAP`].
    log: Vec<CallSession>,
    ring_timeout_secs: u64,
    policy: CallPolicy,
    /// Out-of-order / pre-accept ICE candidates, capped and TTL-expired.
    pending_ice: Vec<PendingIce>,
}

impl CallManager {
    pub fn new() -> Self {
        Self {
            active: None,
            log: Vec::new(),
            ring_timeout_secs: DEFAULT_RING_TIMEOUT_SECS,
            // Deny-by-default: nobody rings this device until a contact set
            // (or an explicit AllowAll) is installed. See [`CallPolicy`].
            policy: CallPolicy::default(),
            pending_ice: Vec::new(),
        }
    }

    pub fn with_ring_timeout(mut self, secs: u64) -> Self {
        self.ring_timeout_secs = secs;
        self
    }

    /// Install the incoming-call gate. See [`CallPolicy`].
    pub fn set_policy(&mut self, policy: CallPolicy) {
        self.policy = policy;
    }

    pub fn active_call(&self) -> Option<&CallSession> {
        self.active.as_ref()
    }

    /// Ended calls, newest first.
    pub fn call_log(&self) -> Vec<CallSession> {
        self.log.iter().rev().cloned().collect()
    }

    /// Start an outgoing call. Returns the new session and the `Offer` signal
    /// to send. Fails with [`PukarError::AlreadyInCall`] if a call is live.
    pub fn place_call(
        &mut self,
        peer: &str,
        media: CallMedia,
        sdp_offer: &str,
        now: u64,
    ) -> Result<(CallSession, CallSignal), PukarError> {
        if self.active.is_some() {
            return Err(PukarError::AlreadyInCall);
        }
        let session = CallSession {
            call_id: new_call_id(),
            peer: peer.to_string(),
            media,
            direction: CallDirection::Outgoing,
            state: CallState::Ringing,
            cause: None,
            started_at: now,
            connecting_since: None,
            connected_at: None,
            ended_at: None,
        };
        let signal = CallSignal::Offer {
            call_id: session.call_id.clone(),
            media,
            sdp: sdp_offer.to_string(),
        };
        self.active = Some(session.clone());
        Ok((session, signal))
    }

    /// Accept the ringing incoming call. Returns the `Answer` signal to send
    /// plus any ICE candidates that arrived while ringing — they are withheld
    /// until the user consents (forwarding ICE pre-accept would let a caller
    /// probe the callee's network without an answered call) and flushed here
    /// for the platform WebRTC layer to apply after the answer.
    pub fn accept(
        &mut self,
        call_id: &str,
        sdp_answer: &str,
        now: u64,
    ) -> Result<(CallSignal, Vec<CallEvent>), PukarError> {
        let session = self.active_mut(call_id)?;
        if session.direction != CallDirection::Incoming || session.state != CallState::Ringing {
            return Err(PukarError::InvalidState(
                "accept requires an incoming ringing call".into(),
            ));
        }
        session.state = CallState::Connecting;
        session.connecting_since = Some(now);
        let peer = session.peer.clone();
        let flushed = self.drain_pending_ice(&peer, call_id);
        Ok((
            CallSignal::Answer {
                call_id: call_id.to_string(),
                sdp: sdp_answer.to_string(),
            },
            flushed,
        ))
    }

    /// Decline the ringing incoming call. Returns the `Reject` signal to send.
    pub fn reject(&mut self, call_id: &str, now: u64) -> Result<CallSignal, PukarError> {
        let session = self.active_mut(call_id)?;
        if session.direction != CallDirection::Incoming || session.state != CallState::Ringing {
            return Err(PukarError::InvalidState(
                "reject requires an incoming ringing call".into(),
            ));
        }
        self.end_active(EndCause::Declined, now);
        Ok(CallSignal::Reject {
            call_id: call_id.to_string(),
            reason: RejectReason::Declined,
        })
    }

    /// Hang up (active/connecting call) or cancel (outgoing ringing call).
    /// Returns the `End` signal to send.
    pub fn hangup(&mut self, call_id: &str, now: u64) -> Result<CallSignal, PukarError> {
        let session = self.active_mut(call_id)?;
        let cause = match (session.direction, session.state) {
            // Only a call whose media actually flowed logs as Completed.
            (_, CallState::Active) => EndCause::Completed,
            (_, CallState::Connecting) => EndCause::Cancelled,
            (CallDirection::Outgoing, CallState::Ringing) => EndCause::Cancelled,
            // Hanging up a ringing incoming call is just a decline.
            (CallDirection::Incoming, CallState::Ringing) => {
                return self.reject(call_id, now);
            }
            (_, CallState::Ended) => {
                return Err(PukarError::InvalidState("call already ended".into()))
            }
        };
        self.end_active(cause, now);
        Ok(CallSignal::End {
            call_id: call_id.to_string(),
            reason: EndReason::Hangup,
        })
    }

    /// Wrap a locally-gathered ICE candidate as a signal to send.
    pub fn local_ice(
        &mut self,
        call_id: &str,
        candidate: &str,
        sdp_mid: Option<String>,
        sdp_mline_index: Option<u32>,
    ) -> Result<CallSignal, PukarError> {
        let session = self.active_mut(call_id)?;
        if session.state == CallState::Ended {
            return Err(PukarError::InvalidState("call already ended".into()));
        }
        Ok(CallSignal::Ice {
            call_id: call_id.to_string(),
            candidate: candidate.to_string(),
            sdp_mid,
            sdp_mline_index,
        })
    }

    /// Platform WebRTC reports media is flowing (ICE completed).
    pub fn mark_connected(&mut self, call_id: &str, now: u64) -> Result<CallSession, PukarError> {
        let session = self.active_mut(call_id)?;
        if session.state != CallState::Connecting && session.state != CallState::Ringing {
            return Err(PukarError::InvalidState(
                "connect requires a ringing/connecting call".into(),
            ));
        }
        session.state = CallState::Active;
        session.connected_at = Some(now);
        Ok(session.clone())
    }

    /// Locally fail the active call (e.g. the signaling send failed).
    pub fn fail_call(&mut self, call_id: &str, now: u64) {
        if self.active.as_ref().is_some_and(|s| s.call_id == call_id) {
            self.end_active(EndCause::Failed, now);
        }
    }

    /// Feed a validated remote signal into the state machine.
    ///
    /// `from_peer` is the hex pubkey the (already signature-checked and
    /// decrypted) event came from. Signals are accepted only when **both** the
    /// call id and the sender match the live session — a third party who
    /// guesses a call id still cannot inject signals into it.
    ///
    /// Returns UI events plus an optional auto-reply signal to send back to
    /// `from_peer` (e.g. `Reject(busy)` for a second simultaneous offer).
    pub fn handle_signal(
        &mut self,
        from_peer: &str,
        signal: CallSignal,
        now: u64,
    ) -> (Vec<CallEvent>, Option<CallSignal>) {
        match signal {
            CallSignal::Offer {
                call_id,
                media,
                sdp,
            } => self.on_offer(from_peer, call_id, media, sdp, now),

            CallSignal::Answer { call_id, sdp } => {
                let Some(session) = self.matching_session(from_peer, &call_id) else {
                    return (vec![], None);
                };
                if session.direction != CallDirection::Outgoing
                    || session.state != CallState::Ringing
                {
                    return (vec![], None);
                }
                session.state = CallState::Connecting;
                session.connecting_since = Some(now);
                let mut events = vec![CallEvent::CallAnswered {
                    call_id: call_id.clone(),
                    sdp_answer: sdp,
                }];
                // ICE that overtook the answer across relays is now usable.
                events.extend(self.drain_pending_ice(from_peer, &call_id));
                (events, None)
            }

            CallSignal::Ice {
                call_id,
                candidate,
                sdp_mid,
                sdp_mline_index,
            } => {
                let deliver = match self.matching_session(from_peer, &call_id) {
                    // Remote ICE is only usable once the answer is exchanged
                    // (Connecting/Active). While ringing it is withheld: the
                    // callee must not exchange connectivity checks (and leak
                    // network info) before the user accepts, and the caller
                    // cannot apply candidates before the answer arrives.
                    Some(s) => s.state == CallState::Connecting || s.state == CallState::Active,
                    // No session (yet): the Offer/Answer may still be in
                    // flight on a slower relay. Buffer briefly.
                    None => false,
                };
                if deliver {
                    (
                        vec![CallEvent::RemoteIce {
                            call_id,
                            candidate,
                            sdp_mid,
                            sdp_mline_index,
                        }],
                        None,
                    )
                } else {
                    self.buffer_ice(PendingIce {
                        received_at: now,
                        from_peer: from_peer.to_string(),
                        call_id,
                        candidate,
                        sdp_mid,
                        sdp_mline_index,
                    });
                    (vec![], None)
                }
            }

            CallSignal::Reject { call_id, reason } => {
                let Some(session) = self.matching_session(from_peer, &call_id) else {
                    return (vec![], None);
                };
                if session.direction != CallDirection::Outgoing {
                    return (vec![], None);
                }
                let cause = match reason {
                    RejectReason::Busy => EndCause::PeerBusy,
                    RejectReason::Declined | RejectReason::Unsupported => EndCause::Declined,
                };
                let ended = self.end_active(cause, now);
                (
                    ended
                        .map(|call| CallEvent::CallEnded { call })
                        .into_iter()
                        .collect(),
                    None,
                )
            }

            CallSignal::End { call_id, reason } => {
                let Some(session) = self.matching_session(from_peer, &call_id) else {
                    return (vec![], None);
                };
                let cause = match (session.direction, session.state, reason) {
                    // Media flowed, then the peer hung up: a completed call.
                    (_, CallState::Active, _) => EndCause::Completed,
                    // Answer exchanged but media never established.
                    (_, CallState::Connecting, _) => EndCause::Failed,
                    // Caller gave up / timed out while our phone was ringing.
                    (CallDirection::Incoming, CallState::Ringing, _) => EndCause::Missed,
                    // Callee's device ended an outgoing ring (e.g. their timeout).
                    (CallDirection::Outgoing, CallState::Ringing, EndReason::Failed) => {
                        EndCause::Failed
                    }
                    (CallDirection::Outgoing, CallState::Ringing, _) => EndCause::NoAnswer,
                    (_, CallState::Ended, _) => return (vec![], None),
                };
                let ended = self.end_active(cause, now);
                (
                    ended
                        .map(|call| CallEvent::CallEnded { call })
                        .into_iter()
                        .collect(),
                    None,
                )
            }
        }
    }

    /// Advance time: expire a ringing call past the ring timeout, a
    /// `Connecting` call past the connect timeout, and stale buffered ICE.
    /// Returns UI events plus an optional `(peer_hex, End)` signal to send.
    pub fn tick(&mut self, now: u64) -> (Vec<CallEvent>, Option<(String, CallSignal)>) {
        self.pending_ice
            .retain(|p| now.saturating_sub(p.received_at) < ICE_BUFFER_TTL_SECS);

        let Some(session) = self.active.as_ref() else {
            return (vec![], None);
        };
        let (cause, reply) = match session.state {
            CallState::Ringing
                if now.saturating_sub(session.started_at) >= self.ring_timeout_secs =>
            {
                match session.direction {
                    // Nobody picked up: stop their phone ringing too.
                    CallDirection::Outgoing => (
                        EndCause::NoAnswer,
                        Some((
                            session.peer.clone(),
                            CallSignal::End {
                                call_id: session.call_id.clone(),
                                reason: EndReason::Timeout,
                            },
                        )),
                    ),
                    // We never answered; the caller times out on their side.
                    CallDirection::Incoming => (EndCause::Missed, None),
                }
            }
            // Answer exchanged but ICE never completed: without this, a lost
            // signal or dead media path wedges the call slot forever.
            CallState::Connecting
                if session
                    .connecting_since
                    .is_some_and(|t| now.saturating_sub(t) >= CONNECT_TIMEOUT_SECS) =>
            {
                (
                    EndCause::Failed,
                    Some((
                        session.peer.clone(),
                        CallSignal::End {
                            call_id: session.call_id.clone(),
                            reason: EndReason::Failed,
                        },
                    )),
                )
            }
            _ => return (vec![], None),
        };
        let ended = self.end_active(cause, now);
        (
            ended
                .map(|call| CallEvent::CallEnded { call })
                .into_iter()
                .collect(),
            reply,
        )
    }

    // ── Internals ────────────────────────────────────────────────────────────

    fn on_offer(
        &mut self,
        from_peer: &str,
        call_id: String,
        media: CallMedia,
        sdp: String,
        now: u64,
    ) -> (Vec<CallEvent>, Option<CallSignal>) {
        // Consent gate FIRST: offers from disallowed callers are dropped
        // silently — no ring, no busy reply, no presence oracle.
        if !self.policy.allows(from_peer) {
            debug!(call_id, "pukar: dropping offer from non-allowed caller");
            return (vec![], None);
        }

        if let Some(active) = &self.active {
            // Duplicate offer for the live call (relay echo): ignore.
            if active.call_id == call_id && active.peer == from_peer {
                return (vec![], None);
            }

            // Glare: we are dialling this exact peer while their offer for a
            // different call arrives. Resolve deterministically — both sides
            // compare the two call ids the same way, so exactly one call
            // survives with no extra round-trip: the LOWER call id wins.
            if active.peer == from_peer
                && active.direction == CallDirection::Outgoing
                && active.state == CallState::Ringing
            {
                if call_id < active.call_id {
                    // Their call wins: silently retire ours (the peer resolves
                    // identically and never answers it) and ring theirs.
                    let cancelled = self.end_active(EndCause::Cancelled, now);
                    let mut events: Vec<CallEvent> = cancelled
                        .map(|call| CallEvent::CallEnded { call })
                        .into_iter()
                        .collect();
                    let (mut ring_events, reply) =
                        self.on_offer(from_peer, call_id, media, sdp, now);
                    events.append(&mut ring_events);
                    return (events, reply);
                }
                // Our call wins: ignore their offer; they retire it themselves.
                return (vec![], None);
            }

            // Any other offer while a call is live: auto-busy, don't disturb.
            return (
                vec![],
                Some(CallSignal::Reject {
                    call_id,
                    reason: RejectReason::Busy,
                }),
            );
        }
        // Replayed offer for a call that already ended: ignore.
        if self.log.iter().any(|s| s.call_id == call_id) {
            debug!(call_id, "pukar: ignoring replayed offer for ended call");
            return (vec![], None);
        }
        let session = CallSession {
            call_id,
            peer: from_peer.to_string(),
            media,
            direction: CallDirection::Incoming,
            state: CallState::Ringing,
            cause: None,
            started_at: now,
            connecting_since: None,
            connected_at: None,
            ended_at: None,
        };
        self.active = Some(session.clone());
        (
            vec![CallEvent::IncomingCall {
                call: session,
                sdp_offer: sdp,
            }],
            None,
        )
    }

    /// Buffer an early ICE candidate (bounded; oldest evicted first).
    fn buffer_ice(&mut self, ice: PendingIce) {
        if self.pending_ice.len() >= ICE_BUFFER_CAP {
            self.pending_ice.remove(0);
        }
        self.pending_ice.push(ice);
    }

    /// Remove and return (as events) all buffered ICE for `(peer, call_id)`.
    fn drain_pending_ice(&mut self, peer: &str, call_id: &str) -> Vec<CallEvent> {
        let mut flushed = Vec::new();
        self.pending_ice.retain(|p| {
            if p.from_peer == peer && p.call_id == call_id {
                flushed.push(CallEvent::RemoteIce {
                    call_id: p.call_id.clone(),
                    candidate: p.candidate.clone(),
                    sdp_mid: p.sdp_mid.clone(),
                    sdp_mline_index: p.sdp_mline_index,
                });
                false
            } else {
                true
            }
        });
        flushed
    }

    fn active_mut(&mut self, call_id: &str) -> Result<&mut CallSession, PukarError> {
        match self.active.as_mut() {
            Some(s) if s.call_id == call_id => Ok(s),
            _ => Err(PukarError::NoSuchCall(call_id.to_string())),
        }
    }

    /// Session for `call_id` only if it is live AND belongs to `from_peer`.
    fn matching_session(&mut self, from_peer: &str, call_id: &str) -> Option<&mut CallSession> {
        self.active
            .as_mut()
            .filter(|s| s.call_id == call_id && s.peer == from_peer)
    }

    fn end_active(&mut self, cause: EndCause, now: u64) -> Option<CallSession> {
        let mut session = self.active.take()?;
        session.state = CallState::Ended;
        session.cause = Some(cause);
        session.ended_at = Some(now);
        self.log.push(session.clone());
        if self.log.len() > CALL_LOG_CAP {
            let excess = self.log.len() - CALL_LOG_CAP;
            self.log.drain(..excess);
        }
        Some(session)
    }
}

/// 128-bit random call id, hex-encoded.
fn new_call_id() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

// ── Network engine ────────────────────────────────────────────────────────────

/// Callback invoked for every [`CallEvent`] produced by incoming signals.
pub type CallCallback = Box<dyn Fn(CallEvent) + Send + Sync + 'static>;

/// The live signaling engine. Mirrors `VaultEngine`: construction is offline
/// (relays registered, not dialled); call [`connect`](Self::connect) then
/// [`subscribe_signals`](Self::subscribe_signals) from a Tokio context.
pub struct PukarEngine {
    client: Client,
    our_keys: Keys,
    manager: Arc<Mutex<CallManager>>,
}

impl PukarEngine {
    pub async fn new(keys: &Keys, relay_urls: Vec<String>) -> Result<Self, PukarError> {
        let client = Client::new(keys.clone());
        for url in &relay_urls {
            client
                .add_relay(url.as_str())
                .await
                .map_err(|e| PukarError::Signaling(e.to_string()))?;
        }
        Ok(Self {
            client,
            our_keys: keys.clone(),
            manager: Arc::new(Mutex::new(CallManager::new())),
        })
    }

    pub async fn connect(&self) {
        self.client.connect().await;
        info!("Pukar engine connected");
    }

    pub fn active_call(&self) -> Option<CallSession> {
        self.lock_manager().active_call().cloned()
    }

    /// Ended calls, newest first.
    pub fn call_log(&self) -> Vec<CallSession> {
        self.lock_manager().call_log()
    }

    /// Install the incoming-call consent gate (see [`CallPolicy`]). The
    /// default denies everyone; the runtime installs the saved contact set on
    /// unlock.
    pub fn set_policy(&self, policy: CallPolicy) {
        self.lock_manager().set_policy(policy);
    }

    /// Place an audio/video call: updates local state and sends the encrypted
    /// `Offer`. On send failure the session is marked `Failed` and the error
    /// propagates.
    pub async fn place_call(
        &self,
        peer: &PublicKey,
        media: CallMedia,
        sdp_offer: &str,
    ) -> Result<CallSession, PukarError> {
        let now = now_secs();
        let (session, signal) =
            self.lock_manager()
                .place_call(&peer.to_hex(), media, sdp_offer, now)?;
        if let Err(e) = self.send_signal(peer, &signal).await {
            self.lock_manager().fail_call(&session.call_id, now_secs());
            return Err(e);
        }
        info!(call_id = %session.call_id, ?media, "pukar: outgoing call ringing");
        Ok(session)
    }

    /// Accept the ringing incoming call with the platform's SDP answer.
    ///
    /// On success, returns the remote ICE candidates that were withheld while
    /// ringing — feed them to the platform WebRTC layer after applying the
    /// answer. On send failure the session is marked `Failed` (mirroring
    /// [`place_call`](Self::place_call)) so it never wedges in `Connecting`.
    pub async fn accept(
        &self,
        call_id: &str,
        sdp_answer: &str,
    ) -> Result<Vec<CallEvent>, PukarError> {
        let (peer, signal, flushed_ice) = {
            let mut mgr = self.lock_manager();
            let peer = self.session_peer(&mgr, call_id)?;
            let (signal, flushed) = mgr.accept(call_id, sdp_answer, now_secs())?;
            (peer, signal, flushed)
        };
        if let Err(e) = self.send_signal(&peer, &signal).await {
            self.lock_manager().fail_call(call_id, now_secs());
            return Err(e);
        }
        Ok(flushed_ice)
    }

    /// Decline the ringing incoming call. Returns the ended session for the
    /// caller to log/emit.
    pub async fn reject(&self, call_id: &str) -> Result<Option<CallSession>, PukarError> {
        let (peer, signal, ended) = {
            let mut mgr = self.lock_manager();
            let peer = self.session_peer(&mgr, call_id)?;
            let signal = mgr.reject(call_id, now_secs())?;
            (peer, signal, mgr.call_log().into_iter().next())
        };
        self.send_signal(&peer, &signal).await?;
        Ok(ended)
    }

    /// Hang up / cancel the current call. Returns the ended session for the
    /// caller to log/emit.
    pub async fn hangup(&self, call_id: &str) -> Result<Option<CallSession>, PukarError> {
        let (peer, signal, ended) = {
            let mut mgr = self.lock_manager();
            let peer = self.session_peer(&mgr, call_id)?;
            let signal = mgr.hangup(call_id, now_secs())?;
            (peer, signal, mgr.call_log().into_iter().next())
        };
        self.send_signal(&peer, &signal).await?;
        Ok(ended)
    }

    /// Forward a locally-gathered ICE candidate to the peer.
    pub async fn send_ice(
        &self,
        call_id: &str,
        candidate: &str,
        sdp_mid: Option<String>,
        sdp_mline_index: Option<u32>,
    ) -> Result<(), PukarError> {
        let (peer, signal) = {
            let mut mgr = self.lock_manager();
            let peer = self.session_peer(&mgr, call_id)?;
            (
                peer,
                mgr.local_ice(call_id, candidate, sdp_mid, sdp_mline_index)?,
            )
        };
        self.send_signal(&peer, &signal).await
    }

    /// Platform WebRTC reports the media path is up.
    pub fn mark_connected(&self, call_id: &str) -> Result<CallSession, PukarError> {
        self.lock_manager().mark_connected(call_id, now_secs())
    }

    /// Expire ring timeouts. Call periodically (~every 5 s) from a background
    /// task; returns the events to surface to the UI.
    pub async fn tick(&self) -> Vec<CallEvent> {
        let (events, reply) = self.lock_manager().tick(now_secs());
        if let Some((peer_hex, signal)) = reply {
            match PublicKey::from_hex(&peer_hex) {
                Ok(peer) => {
                    if let Err(e) = self.send_signal(&peer, &signal).await {
                        warn!("pukar: failed to send timeout signal: {e}");
                    }
                }
                Err(e) => warn!("pukar: bad peer key in timeout path: {e}"),
            }
        }
        events
    }

    /// Subscribe to signaling events addressed to us and drive the state
    /// machine. `callback` receives every resulting [`CallEvent`]; auto-replies
    /// (busy, etc.) are sent without surfacing an event.
    pub async fn subscribe_signals(&self, callback: CallCallback) -> Result<(), PukarError> {
        let our_pk = self.our_keys.public_key();
        let filter = Filter::new()
            .kind(Kind::from(SIGNAL_KIND))
            .pubkey(our_pk)
            .since(Timestamp::now());

        self.client
            .subscribe(filter, None)
            .await
            .map_err(|e| PukarError::Signaling(e.to_string()))?;
        info!("Pukar signaling subscription active");

        let our_keys = self.our_keys.clone();
        let manager = self.manager.clone();
        let client = self.client.clone();
        let callback = Arc::new(callback);

        self.client
            .handle_notifications(move |notification| {
                let our_keys = our_keys.clone();
                let manager = manager.clone();
                let client = client.clone();
                let callback = callback.clone();

                async move {
                    let RelayPoolNotification::Event { event, .. } = notification else {
                        return Ok::<bool, Box<dyn std::error::Error>>(false);
                    };
                    if event.kind != Kind::from(SIGNAL_KIND) {
                        return Ok(false);
                    }

                    // Freshness: bound the replay window for hostile relays.
                    // The future window is tighter than the past one — honest
                    // events are never meaningfully in the future, and clock
                    // skew is a support issue, not a protocol state.
                    let now = Timestamp::now().as_secs();
                    let ts = event.created_at.as_secs();
                    let too_old = ts < now.saturating_sub(SIGNAL_MAX_AGE_SECS);
                    let too_future = ts > now + SIGNAL_MAX_FUTURE_SKEW_SECS;
                    if too_old || too_future {
                        warn!(
                            event_id = %event.id,
                            skew_secs = now.abs_diff(ts),
                            "pukar: dropping stale/skewed signal (check device clocks)"
                        );
                        return Ok(false);
                    }

                    let sender_pk = event.pubkey;
                    let decrypted = match nip44::decrypt(
                        our_keys.secret_key(),
                        &sender_pk,
                        event.content.clone(),
                    ) {
                        Ok(d) => d,
                        Err(e) => {
                            warn!(event_id = %event.id, "pukar: failed to decrypt signal: {e}");
                            return Ok(false);
                        }
                    };

                    let signal = match parse_signal(&decrypted) {
                        Some(Ok(s)) => s,
                        Some(Err(PukarError::UnsupportedVersion(v))) => {
                            // A future protocol version is calling us. Fail
                            // fast with `unsupported` so their side stops
                            // ringing immediately instead of ghost-ringing
                            // for the full timeout.
                            warn!(event_id = %event.id, version = v, "pukar: unsupported signal version");
                            if let Some(call_id) = extract_call_id_lossy(&decrypted) {
                                let reject = CallSignal::Reject {
                                    call_id,
                                    reason: RejectReason::Unsupported,
                                };
                                if let Err(e) =
                                    send_signal_with(&client, &our_keys, &sender_pk, &reject).await
                                {
                                    debug!("pukar: unsupported-version reject failed: {e}");
                                }
                            }
                            return Ok(false);
                        }
                        Some(Err(e)) => {
                            warn!(event_id = %event.id, "pukar: malformed signal: {e}");
                            return Ok(false);
                        }
                        // Not a pukar payload at all — some other kind-25050 use.
                        None => return Ok(false),
                    };

                    debug!(event_id = %event.id, sender = %sender_pk, "pukar: signal received");

                    // Drive the state machine WITHOUT holding the lock across
                    // the async reply send below.
                    let (events, reply) = match manager.lock() {
                        Ok(mut mgr) => mgr.handle_signal(&sender_pk.to_hex(), signal, now_secs()),
                        Err(poisoned) => {
                            warn!("pukar: manager lock poisoned; recovering");
                            poisoned.into_inner().handle_signal(
                                &sender_pk.to_hex(),
                                signal,
                                now_secs(),
                            )
                        }
                    };

                    if let Some(auto_reply) = reply {
                        if let Err(e) =
                            send_signal_with(&client, &our_keys, &sender_pk, &auto_reply).await
                        {
                            warn!("pukar: failed to send auto-reply: {e}");
                        }
                    }
                    for ev in events {
                        callback(ev);
                    }
                    Ok(false)
                }
            })
            .await
            .map_err(|e| PukarError::Signaling(e.to_string()))
    }

    // ── Internals ────────────────────────────────────────────────────────────

    fn lock_manager(&self) -> std::sync::MutexGuard<'_, CallManager> {
        match self.manager.lock() {
            Ok(g) => g,
            Err(poisoned) => {
                warn!("pukar: manager lock poisoned; recovering");
                poisoned.into_inner()
            }
        }
    }

    fn session_peer(&self, mgr: &CallManager, call_id: &str) -> Result<PublicKey, PukarError> {
        let session = mgr
            .active_call()
            .filter(|s| s.call_id == call_id)
            .ok_or_else(|| PukarError::NoSuchCall(call_id.to_string()))?;
        PublicKey::from_hex(&session.peer)
            .map_err(|e| PukarError::Signaling(format!("bad peer key: {e}")))
    }

    async fn send_signal(&self, peer: &PublicKey, signal: &CallSignal) -> Result<(), PukarError> {
        send_signal_with(&self.client, &self.our_keys, peer, signal).await
    }
}

/// Encrypt `signal` (NIP-44 v2) to `peer` and publish it as an ephemeral
/// event. Signaling is a real-time protocol: acceptance by **zero** relays is
/// a failure, not a success, so the publish result is checked.
async fn send_signal_with(
    client: &Client,
    our_keys: &Keys,
    peer: &PublicKey,
    signal: &CallSignal,
) -> Result<(), PukarError> {
    let plaintext = encode_signal(signal)?;
    let encrypted = nip44::encrypt(
        our_keys.secret_key(),
        peer,
        plaintext.as_bytes(),
        nip44::Version::V2,
    )
    .map_err(|e| PukarError::Signaling(e.to_string()))?;

    let event = EventBuilder::new(Kind::from(SIGNAL_KIND), encrypted)
        .tag(Tag::public_key(*peer))
        .sign_with_keys(our_keys)
        .map_err(|e| PukarError::Signaling(e.to_string()))?;

    let output = client
        .send_event(&event)
        .await
        .map_err(|e| PukarError::Signaling(e.to_string()))?;
    if output.success.is_empty() {
        return Err(PukarError::Signaling(format!(
            "no relay accepted the signal (failures: {:?})",
            output.failed
        )));
    }
    debug!(
        call_id = signal.call_id(),
        relays = output.success.len(),
        "pukar: signal sent"
    );
    Ok(())
}

/// Best-effort `signal.call_id` extraction from a raw (possibly
/// future-version) envelope, for the unsupported-version fast-fail reply.
fn extract_call_id_lossy(raw: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(raw).ok()?;
    value
        .get("signal")?
        .get("call_id")?
        .as_str()
        .map(|s| s.to_string())
}

fn now_secs() -> u64 {
    Timestamp::now().as_secs()
}

/// Convenience for callers that track per-call metadata client-side.
pub type CallMetadata = HashMap<String, String>;

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const ALICE: &str = "aa11";
    const BOB: &str = "bb22";

    /// A manager that accepts calls from anyone (tests opt in explicitly —
    /// the production default denies all callers until contacts are loaded).
    fn open_manager() -> CallManager {
        let mut mgr = CallManager::new();
        mgr.set_policy(CallPolicy::AllowAll);
        mgr
    }

    fn ring_pair(now: u64) -> (CallManager, CallManager, String, String) {
        // Alice calls Bob; Bob's manager sees the offer and rings.
        let mut alice = open_manager();
        let mut bob = open_manager();
        let (session, offer) = alice
            .place_call(BOB, CallMedia::Audio, "sdp-offer", now)
            .unwrap();
        let (events, reply) = bob.handle_signal(ALICE, offer, now);
        assert!(reply.is_none());
        assert!(matches!(&events[..], [CallEvent::IncomingCall { .. }]));
        let sdp = match &events[0] {
            CallEvent::IncomingCall { sdp_offer, .. } => sdp_offer.clone(),
            _ => unreachable!(),
        };
        (alice, bob, session.call_id, sdp)
    }

    fn offer(call_id: &str) -> CallSignal {
        CallSignal::Offer {
            call_id: call_id.into(),
            media: CallMedia::Audio,
            sdp: "sdp".into(),
        }
    }

    #[test]
    fn signal_envelope_roundtrip_and_foreign_content() {
        let signal = CallSignal::Offer {
            call_id: "c1".into(),
            media: CallMedia::Video,
            sdp: "v=0...".into(),
        };
        let wire = encode_signal(&signal).unwrap();
        assert!(wire.contains("\"comrade_pukar\":1"));
        assert_eq!(parse_signal(&wire).unwrap().unwrap(), signal);

        // Ordinary DM text and non-pukar JSON are not signals.
        assert!(parse_signal("hey, coffee later?").is_none());
        assert!(parse_signal("{\"foo\":1}").is_none());

        // Future versions are surfaced as typed errors, not silently accepted.
        let future = "{\"comrade_pukar\":99,\"signal\":{}}";
        assert!(matches!(
            parse_signal(future),
            Some(Err(PukarError::UnsupportedVersion(99)))
        ));

        // Claimed-but-broken envelope is Malformed.
        let broken = "{\"comrade_pukar\":1,\"signal\":{\"type\":\"offer\"}}";
        assert!(matches!(
            parse_signal(broken),
            Some(Err(PukarError::Malformed(_)))
        ));
    }

    #[test]
    fn full_happy_path_offer_answer_ice_connect_hangup() {
        let now = 1_000;
        let (mut alice, mut bob, call_id, sdp_offer) = ring_pair(now);
        assert_eq!(sdp_offer, "sdp-offer");
        assert_eq!(bob.active_call().unwrap().state, CallState::Ringing);
        assert_eq!(alice.active_call().unwrap().state, CallState::Ringing);

        // Bob accepts; Alice sees the answer.
        let (answer, _flushed) = bob.accept(&call_id, "sdp-answer", now + 3).unwrap();
        assert_eq!(bob.active_call().unwrap().state, CallState::Connecting);
        let (events, _) = alice.handle_signal(BOB, answer, now + 3);
        assert!(matches!(
            &events[..],
            [CallEvent::CallAnswered { sdp_answer, .. }] if sdp_answer == "sdp-answer"
        ));
        assert_eq!(alice.active_call().unwrap().state, CallState::Connecting);

        // Trickle ICE both ways.
        let ice = alice
            .local_ice(&call_id, "cand-1", Some("0".into()), Some(0))
            .unwrap();
        let (events, _) = bob.handle_signal(ALICE, ice, now + 4);
        assert!(
            matches!(&events[..], [CallEvent::RemoteIce { candidate, .. }] if candidate == "cand-1")
        );

        // Both sides connect.
        alice.mark_connected(&call_id, now + 5).unwrap();
        bob.mark_connected(&call_id, now + 5).unwrap();
        assert_eq!(alice.active_call().unwrap().state, CallState::Active);

        // Alice hangs up; both logs show a completed call with duration.
        let end = alice.hangup(&call_id, now + 65).unwrap();
        let (events, _) = bob.handle_signal(ALICE, end, now + 65);
        assert!(matches!(
            &events[..],
            [CallEvent::CallEnded { call }] if call.cause == Some(EndCause::Completed)
        ));
        assert!(alice.active_call().is_none());
        assert!(bob.active_call().is_none());
        let a_log = alice.call_log();
        assert_eq!(a_log.len(), 1);
        assert_eq!(a_log[0].cause, Some(EndCause::Completed));
        assert_eq!(a_log[0].duration_secs(), Some(60));
    }

    #[test]
    fn reject_ends_both_sides_as_declined() {
        let now = 50;
        let (mut alice, mut bob, call_id, _) = ring_pair(now);
        let reject = bob.reject(&call_id, now + 5).unwrap();
        assert_eq!(bob.call_log()[0].cause, Some(EndCause::Declined));

        let (events, _) = alice.handle_signal(BOB, reject, now + 5);
        assert!(matches!(
            &events[..],
            [CallEvent::CallEnded { call }] if call.cause == Some(EndCause::Declined)
        ));
    }

    #[test]
    fn second_offer_gets_busy_and_current_call_is_undisturbed() {
        let now = 10;
        let (_alice, mut bob, call_id, _) = ring_pair(now);

        // Carol calls Bob while Alice's call is ringing.
        let (events, reply) = bob.handle_signal(
            "cc33",
            CallSignal::Offer {
                call_id: "carol-call".into(),
                media: CallMedia::Audio,
                sdp: "x".into(),
            },
            now + 1,
        );
        assert!(events.is_empty(), "current call must not be disturbed");
        assert!(matches!(
            reply,
            Some(CallSignal::Reject { reason: RejectReason::Busy, call_id }) if call_id == "carol-call"
        ));
        assert_eq!(bob.active_call().unwrap().call_id, call_id);
    }

    #[test]
    fn busy_reject_maps_to_peer_busy_for_caller() {
        let now = 10;
        let mut alice = open_manager();
        let (session, _) = alice.place_call(BOB, CallMedia::Audio, "sdp", now).unwrap();
        let (events, _) = alice.handle_signal(
            BOB,
            CallSignal::Reject {
                call_id: session.call_id,
                reason: RejectReason::Busy,
            },
            now + 1,
        );
        assert!(matches!(
            &events[..],
            [CallEvent::CallEnded { call }] if call.cause == Some(EndCause::PeerBusy)
        ));
    }

    #[test]
    fn cannot_place_second_call_while_active() {
        let mut alice = open_manager();
        alice.place_call(BOB, CallMedia::Audio, "sdp", 1).unwrap();
        assert!(matches!(
            alice.place_call("cc33", CallMedia::Video, "sdp", 2),
            Err(PukarError::AlreadyInCall)
        ));
    }

    #[test]
    fn signals_from_wrong_peer_or_wrong_call_are_ignored() {
        let now = 10;
        let (mut alice, _bob, call_id, _) = ring_pair(now);

        // Mallory tries to answer Alice's call to Bob.
        let (events, reply) = alice.handle_signal(
            "ee55",
            CallSignal::Answer {
                call_id: call_id.clone(),
                sdp: "evil".into(),
            },
            now + 1,
        );
        assert!(events.is_empty() && reply.is_none());
        assert_eq!(alice.active_call().unwrap().state, CallState::Ringing);

        // Bob answers the wrong call id — also ignored.
        let (events, _) = alice.handle_signal(
            BOB,
            CallSignal::Answer {
                call_id: "not-a-real-call".into(),
                sdp: "x".into(),
            },
            now + 1,
        );
        assert!(events.is_empty());

        // Mallory cannot end the call either.
        let (events, _) = alice.handle_signal(
            "ee55",
            CallSignal::End {
                call_id,
                reason: EndReason::Hangup,
            },
            now + 2,
        );
        assert!(events.is_empty());
        assert!(alice.active_call().is_some());
    }

    #[test]
    fn outgoing_ring_timeout_is_no_answer_and_notifies_peer() {
        let now = 100;
        let mut alice = CallManager::new().with_ring_timeout(60);
        let (session, _) = alice.place_call(BOB, CallMedia::Audio, "sdp", now).unwrap();

        // Not yet.
        let (events, reply) = alice.tick(now + 59);
        assert!(events.is_empty() && reply.is_none());

        // Rings out.
        let (events, reply) = alice.tick(now + 60);
        assert!(matches!(
            &events[..],
            [CallEvent::CallEnded { call }] if call.cause == Some(EndCause::NoAnswer)
        ));
        let (peer, signal) = reply.unwrap();
        assert_eq!(peer, BOB);
        assert!(
            matches!(signal, CallSignal::End { call_id, reason: EndReason::Timeout } if call_id == session.call_id)
        );
    }

    #[test]
    fn incoming_ring_timeout_is_missed_without_reply() {
        let now = 100;
        let (_alice, mut bob, _call_id, _) = ring_pair(now);
        let (events, reply) = bob.tick(now + DEFAULT_RING_TIMEOUT_SECS);
        assert!(matches!(
            &events[..],
            [CallEvent::CallEnded { call }] if call.cause == Some(EndCause::Missed)
        ));
        assert!(reply.is_none());
    }

    #[test]
    fn caller_cancel_while_ringing_is_missed_for_callee() {
        let now = 10;
        let (mut alice, mut bob, call_id, _) = ring_pair(now);
        let end = alice.hangup(&call_id, now + 5).unwrap();
        assert_eq!(alice.call_log()[0].cause, Some(EndCause::Cancelled));

        let (events, _) = bob.handle_signal(ALICE, end, now + 5);
        assert!(matches!(
            &events[..],
            [CallEvent::CallEnded { call }] if call.cause == Some(EndCause::Missed)
        ));
    }

    #[test]
    fn replayed_offer_for_ended_call_is_ignored() {
        let now = 10;
        let (_alice, mut bob, call_id, _) = ring_pair(now);
        let reject = bob.reject(&call_id, now + 1).unwrap();
        drop(reject);

        // The same offer arrives again (hostile relay replay).
        let (events, reply) = bob.handle_signal(
            ALICE,
            CallSignal::Offer {
                call_id,
                media: CallMedia::Audio,
                sdp: "sdp-offer".into(),
            },
            now + 2,
        );
        assert!(events.is_empty() && reply.is_none());
        assert!(bob.active_call().is_none());
    }

    #[test]
    fn duplicate_offer_for_live_call_is_ignored_not_busy() {
        let now = 10;
        let (_alice, mut bob, call_id, _) = ring_pair(now);
        let (events, reply) = bob.handle_signal(
            ALICE,
            CallSignal::Offer {
                call_id,
                media: CallMedia::Audio,
                sdp: "sdp-offer".into(),
            },
            now + 1,
        );
        assert!(events.is_empty() && reply.is_none());
        assert!(bob.active_call().is_some());
    }

    #[test]
    fn accept_requires_incoming_ringing() {
        let mut alice = open_manager();
        let (session, _) = alice.place_call(BOB, CallMedia::Audio, "s", 1).unwrap();
        // Caller cannot "accept" their own outgoing call.
        assert!(matches!(
            alice.accept(&session.call_id, "a", 2),
            Err(PukarError::InvalidState(_))
        ));
        // Unknown ids are NoSuchCall.
        assert!(matches!(
            alice.accept("nope", "a", 2),
            Err(PukarError::NoSuchCall(_))
        ));
    }

    #[test]
    fn default_policy_denies_strangers_silently() {
        let mut mgr = CallManager::new(); // deny-by-default
        let (events, reply) = mgr.handle_signal(ALICE, offer("c1"), 10);
        assert!(events.is_empty(), "must not ring");
        assert!(reply.is_none(), "must not reveal presence with a reply");
        assert!(mgr.active_call().is_none());

        // Allow-listing the caller makes the same offer ring.
        let mut allowed = std::collections::HashSet::new();
        allowed.insert(ALICE.to_string());
        mgr.set_policy(CallPolicy::ContactsOnly(allowed));
        let (events, _) = mgr.handle_signal(ALICE, offer("c2"), 11);
        assert!(matches!(&events[..], [CallEvent::IncomingCall { .. }]));
        // ...but an unlisted caller is still dropped (busy slot untouched
        // logic aside, the gate runs first).
        let (events, reply) = mgr.handle_signal("ee55", offer("c3"), 12);
        assert!(events.is_empty() && reply.is_none());
    }

    #[test]
    fn glare_resolves_deterministically_to_one_call() {
        // Alice and Bob dial each other simultaneously. Exactly one call must
        // survive, and both sides must pick the SAME one (lower call id).
        let now = 10;
        let mut alice = open_manager();
        let mut bob = open_manager();
        let (a_session, a_offer) = alice.place_call(BOB, CallMedia::Audio, "a", now).unwrap();
        let (b_session, b_offer) = bob.place_call(ALICE, CallMedia::Audio, "b", now).unwrap();
        let winner = a_session.call_id.clone().min(b_session.call_id.clone());

        let (a_events, a_reply) = alice.handle_signal(BOB, b_offer, now + 1);
        let (b_events, b_reply) = bob.handle_signal(ALICE, a_offer, now + 1);

        // Neither side sends busy — resolution is silent and symmetric.
        assert!(a_reply.is_none() && b_reply.is_none());

        // Both sides end up on the same surviving call.
        assert_eq!(alice.active_call().unwrap().call_id, winner);
        assert_eq!(bob.active_call().unwrap().call_id, winner);

        // Exactly one side sees IncomingCall (the loser-of-dialing rings),
        // and that side also observed its own outgoing call retiring.
        let a_rings = a_events
            .iter()
            .any(|e| matches!(e, CallEvent::IncomingCall { .. }));
        let b_rings = b_events
            .iter()
            .any(|e| matches!(e, CallEvent::IncomingCall { .. }));
        assert!(a_rings ^ b_rings, "exactly one side must ring");
    }

    #[test]
    fn connecting_timeout_fails_the_call_and_notifies_peer() {
        let now = 100;
        let (_alice, mut bob, call_id, _) = ring_pair(now);
        bob.accept(&call_id, "answer", now + 5).unwrap();

        // Still connecting just before the deadline.
        let (events, _) = bob.tick(now + 5 + CONNECT_TIMEOUT_SECS - 1);
        assert!(events.is_empty());

        // ICE never completes: the call must not wedge forever.
        let (events, reply) = bob.tick(now + 5 + CONNECT_TIMEOUT_SECS);
        assert!(matches!(
            &events[..],
            [CallEvent::CallEnded { call }] if call.cause == Some(EndCause::Failed)
        ));
        assert!(
            matches!(reply, Some((peer, CallSignal::End { reason: EndReason::Failed, .. })) if peer == ALICE)
        );
        assert!(bob.active_call().is_none());
    }

    #[test]
    fn ice_while_ringing_is_withheld_until_accept() {
        let now = 10;
        let (_alice, mut bob, call_id, _) = ring_pair(now);

        // Caller trickles ICE while Bob is still ringing: not delivered yet
        // (no network probing before consent).
        let ice = CallSignal::Ice {
            call_id: call_id.clone(),
            candidate: "cand-early".into(),
            sdp_mid: Some("0".into()),
            sdp_mline_index: Some(0),
        };
        let (events, _) = bob.handle_signal(ALICE, ice, now + 1);
        assert!(events.is_empty(), "ICE must be withheld while ringing");

        // Accept flushes the withheld candidate.
        let (_answer, flushed) = bob.accept(&call_id, "answer", now + 2).unwrap();
        assert!(matches!(
            &flushed[..],
            [CallEvent::RemoteIce { candidate, .. }] if candidate == "cand-early"
        ));
    }

    #[test]
    fn ice_arriving_before_answer_is_buffered_for_caller() {
        // Cross-relay reordering: callee's ICE overtakes their Answer.
        let now = 10;
        let mut alice = open_manager();
        let (session, _) = alice.place_call(BOB, CallMedia::Audio, "sdp", now).unwrap();

        let ice = CallSignal::Ice {
            call_id: session.call_id.clone(),
            candidate: "cand-fast-relay".into(),
            sdp_mid: None,
            sdp_mline_index: None,
        };
        // Outgoing+Ringing: candidate is usable only after the answer.
        let (events, _) = alice.handle_signal(BOB, ice, now + 1);
        assert!(events.is_empty());

        let (events, _) = alice.handle_signal(
            BOB,
            CallSignal::Answer {
                call_id: session.call_id.clone(),
                sdp: "answer".into(),
            },
            now + 2,
        );
        // Answer event first, then the flushed buffered candidate.
        assert_eq!(events.len(), 2);
        assert!(matches!(&events[0], CallEvent::CallAnswered { .. }));
        assert!(matches!(
            &events[1],
            CallEvent::RemoteIce { candidate, .. } if candidate == "cand-fast-relay"
        ));
    }

    #[test]
    fn connecting_end_causes_are_honest() {
        // Local hangup during Connecting is a Cancelled call, not Completed.
        let now = 10;
        let (_alice, mut bob, call_id, _) = ring_pair(now);
        bob.accept(&call_id, "answer", now + 1).unwrap();
        bob.hangup(&call_id, now + 2).unwrap();
        assert_eq!(bob.call_log()[0].cause, Some(EndCause::Cancelled));
        assert_eq!(bob.call_log()[0].duration_secs(), None);

        // Remote End during Connecting is Failed (media never flowed).
        let (mut alice, mut bob2, call_id2, _) = ring_pair(now);
        let (answer, _) = bob2.accept(&call_id2, "answer", now + 1).unwrap();
        alice.handle_signal(BOB, answer, now + 1);
        let (events, _) = alice.handle_signal(
            BOB,
            CallSignal::End {
                call_id: call_id2,
                reason: EndReason::Hangup,
            },
            now + 3,
        );
        assert!(matches!(
            &events[..],
            [CallEvent::CallEnded { call }] if call.cause == Some(EndCause::Failed)
        ));
    }

    #[test]
    fn extract_call_id_survives_unknown_versions() {
        let future = r#"{"comrade_pukar":9,"signal":{"type":"warp","call_id":"c9","extra":1}}"#;
        assert_eq!(extract_call_id_lossy(future).as_deref(), Some("c9"));
        assert!(extract_call_id_lossy("not json").is_none());
        assert!(extract_call_id_lossy("{\"signal\":{}}").is_none());
    }

    #[test]
    fn call_log_is_capped() {
        let mut mgr = open_manager();
        for i in 0..(CALL_LOG_CAP + 25) {
            let (s, _) = mgr
                .place_call(BOB, CallMedia::Audio, "s", i as u64)
                .unwrap();
            mgr.hangup(&s.call_id, i as u64 + 1).unwrap();
        }
        assert_eq!(mgr.call_log().len(), CALL_LOG_CAP);
    }

    #[test]
    fn call_ids_are_unique_and_wire_types_serde_roundtrip() {
        assert_ne!(new_call_id(), new_call_id());
        assert_eq!(new_call_id().len(), 32);

        let session = CallSession {
            call_id: "c".into(),
            peer: BOB.into(),
            media: CallMedia::Video,
            direction: CallDirection::Incoming,
            state: CallState::Ended,
            cause: Some(EndCause::Missed),
            started_at: 1,
            connecting_since: None,
            connected_at: None,
            ended_at: Some(2),
        };
        let json = serde_json::to_string(&session).unwrap();
        assert!(json.contains("\"media\":\"video\""));
        assert!(json.contains("\"cause\":\"missed\""));
        let back: CallSession = serde_json::from_str(&json).unwrap();
        assert_eq!(back, session);

        let ev = CallEvent::CallConnected {
            call_id: "c".into(),
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains("\"type\":\"call_connected\""));
    }
}
