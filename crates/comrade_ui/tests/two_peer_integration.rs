//! COMMS-03: two independent identities, each with its own `ComradeRuntime`,
//! talking over one isolated in-process relay (`tests/support` — no Docker,
//! no public-internet dependency, so this runs the same in CI as it does
//! locally). This is the layer `CallManagerTest.kt`/`DeviceSmokeTest.kt`
//! cannot reach from a single process: does a message/call signal sent by one
//! *real* identity actually arrive, correctly gated, at a second *real*
//! identity's runtime.
//!
//! Scope, stated honestly: this crate carries call *signaling* only (offer/
//! answer/ICE-candidate/hangup as opaque strings relayed over an encrypted
//! DM) — there is no `RTCPeerConnection` here, so "both a direct and a
//! TURN-relayed call reach active media state" and "no call stuck in
//! ringing/connecting after a timeout" are properties of the Android
//! `CallManager` state machine (see `CallManagerTest.kt`'s lifecycle tests
//! and the Android two-installation harness), not of this Rust layer. What
//! this suite proves is the foundation those depend on: the signaling pipe
//! itself, and the message-request gate that keeps a stranger from ringing
//! anyone.

mod support;

use std::time::Duration;

use comrade_core::call::{CallMediaKind, CallSignal, HangupReason};
use comrade_ui::{BridgeEvent, ComradeRuntime};
use support::TestRelay;
use tempfile::TempDir;

/// Generous for an in-process relay with zero real network latency, but long
/// enough to absorb CI scheduling jitter without being a slow-test problem.
const RECV_TIMEOUT: Duration = Duration::from_secs(5);
/// How long "nothing arrived" tests wait before concluding that. Shorter than
/// [`RECV_TIMEOUT`] — a negative assertion only needs to outlast the relay's
/// own processing, not a generous retry budget.
const ABSENCE_TIMEOUT: Duration = Duration::from_millis(800);
/// Local-relay connection/subscription setup is near-instant, but the vault
/// loops still start on a spawned task — a short yield avoids the first
/// message racing the subscription's `REQ` to the relay.
const SETTLE: Duration = Duration::from_millis(150);

async fn unlocked_runtime(relay_url: &str, dir: &TempDir) -> ComradeRuntime {
    let mut rt = ComradeRuntime::with_relays(vec![relay_url.to_string()]);
    rt.unlock_vault(dir.path(), "pin").await.unwrap();
    rt.spawn_event_loops();
    rt
}

/// Drain [`BridgeEvent`]s from `rx` until `pred` matches one (returned), the
/// timeout elapses (`None`), or the channel closes (`None`) — skipping
/// unrelated chatter (profile-share pushes, receipts, …) along the way,
/// exactly like the real Android/desktop event pumps do.
async fn wait_for(
    rx: &mut tokio::sync::broadcast::Receiver<BridgeEvent>,
    timeout: Duration,
    pred: impl Fn(&BridgeEvent) -> bool,
) -> Option<BridgeEvent> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return None;
        }
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Ok(event)) if pred(&event) => return Some(event),
            Ok(Ok(_)) => continue, // unrelated event — keep waiting
            Ok(Err(_)) | Err(_) => return None, // closed or timed out
        }
    }
}

#[tokio::test]
async fn dm_delivery_is_gated_by_message_request_then_flows_once_accepted() {
    let relay = TestRelay::start().await;
    let alice_dir = TempDir::new().unwrap();
    let bob_dir = TempDir::new().unwrap();
    let alice = unlocked_runtime(&relay.url, &alice_dir).await;
    let bob = unlocked_runtime(&relay.url, &bob_dir).await;
    let bob_npub = bob.profile().unwrap().npub;
    let alice_npub = alice.profile().unwrap().npub;
    let mut bob_events = bob.subscribe_events();
    tokio::time::sleep(SETTLE).await;

    // A stranger's first DM must land as a message request, not a delivered
    // message — the same gate a call signal rides on below.
    alice
        .send_dm(&bob_npub, "hi bob, it's alice")
        .await
        .unwrap();
    let request = wait_for(&mut bob_events, RECV_TIMEOUT, |e| {
        matches!(e, BridgeEvent::IncomingMessageRequest(_))
    })
    .await
    .expect("bob must see alice's first DM as a message request");
    let BridgeEvent::IncomingMessageRequest(req) = request else {
        unreachable!()
    };
    assert_eq!(req.peer, alice_npub);
    assert!(bob
        .message_requests()
        .unwrap()
        .iter()
        .any(|r| r.peer == alice_npub));

    bob.accept_request(&alice_npub).unwrap();
    alice
        .send_dm(&bob_npub, "now that you've accepted")
        .await
        .unwrap();
    let delivered = wait_for(&mut bob_events, RECV_TIMEOUT, |e| {
        matches!(e, BridgeEvent::IncomingDirectMessage(_))
    })
    .await
    .expect("bob must receive the DM as delivered once alice is accepted");
    let BridgeEvent::IncomingDirectMessage(msg) = delivered else {
        unreachable!()
    };
    assert_eq!(msg.content, "now that you've accepted");
    assert_eq!(msg.sender, alice_npub);

    relay.stop().await;
}

#[tokio::test]
async fn a_stranger_cannot_ring_an_unaccepted_target() {
    let relay = TestRelay::start().await;
    let alice_dir = TempDir::new().unwrap();
    let bob_dir = TempDir::new().unwrap();
    let alice = unlocked_runtime(&relay.url, &alice_dir).await;
    let bob = unlocked_runtime(&relay.url, &bob_dir).await;
    let alice_npub = alice.profile().unwrap().npub;
    let mut alice_events = alice.subscribe_events();
    tokio::time::sleep(SETTLE).await;

    // Bob has never exchanged a message with Alice — sends a raw call offer
    // straight over the DM channel (bypassing `place_call`, which itself
    // performs no gating — the receiving side's inbox dispatch is the actual
    // gate under test here, exactly as it is for a real incoming call).
    let offer_json = serde_json::to_string(&CallSignal::Offer {
        sdp: "v=0\r\na=fingerprint:sha-256 AA:BB\r\n".to_string(),
    })
    .unwrap();
    bob.send_call_signal(&alice_npub, "unsolicited-call", "audio", &offer_json)
        .await
        .unwrap();

    let leaked = wait_for(&mut alice_events, ABSENCE_TIMEOUT, |e| {
        matches!(e, BridgeEvent::IncomingCallSignal(_))
    })
    .await;
    assert!(
        leaked.is_none(),
        "a stranger's call offer must never reach the target as IncomingCallSignal"
    );

    relay.stop().await;
}

#[tokio::test]
async fn accepted_peers_exchange_offer_answer_ice_and_hangup_and_log_the_call() {
    let relay = TestRelay::start().await;
    let alice_dir = TempDir::new().unwrap();
    let bob_dir = TempDir::new().unwrap();
    let alice = unlocked_runtime(&relay.url, &alice_dir).await;
    let bob = unlocked_runtime(&relay.url, &bob_dir).await;
    let alice_npub = alice.profile().unwrap().npub;
    let bob_npub = bob.profile().unwrap().npub;
    let mut alice_events = alice.subscribe_events();
    let mut bob_events = bob.subscribe_events();
    tokio::time::sleep(SETTLE).await;

    // Get both sides mutually Accepted: sending a DM accepts the recipient on
    // the *sender's* side automatically (so alice→bob already accepts bob for
    // alice); bob still has to accept alice's resulting message request.
    alice
        .send_dm(&bob_npub, "let's talk before I call")
        .await
        .unwrap();
    wait_for(&mut bob_events, RECV_TIMEOUT, |e| {
        matches!(e, BridgeEvent::IncomingMessageRequest(_))
    })
    .await
    .expect("bob sees alice's request");
    bob.accept_request(&alice_npub).unwrap();

    // ── Caller (alice) places the call and sends the offer ──────────────────
    let session = alice.place_call(&bob_npub, "video").unwrap();
    assert!(
        session.ice_servers.iter().all(|s| s.username.is_none()),
        "the initial offer must be STUN-only, never contacting a TURN relay up front"
    );
    let call_id = session.call_id.clone();
    let offer = CallSignal::Offer {
        sdp: "offer-sdp-alice".to_string(),
    };
    alice
        .send_call_signal(
            &bob_npub,
            &call_id,
            "video",
            &serde_json::to_string(&offer).unwrap(),
        )
        .await
        .unwrap();

    let bob_offer = wait_for(
        &mut bob_events,
        RECV_TIMEOUT,
        |e| matches!(e, BridgeEvent::IncomingCallSignal(dto) if dto.call_id == call_id),
    )
    .await
    .expect("bob must receive the offer");
    let BridgeEvent::IncomingCallSignal(dto) = bob_offer else {
        unreachable!()
    };
    assert_eq!(dto.peer, alice_npub);
    assert_eq!(dto.media, "video");
    assert!(matches!(dto.signal, CallSignal::Offer { sdp } if sdp == "offer-sdp-alice"));

    // ── Callee (bob) answers ────────────────────────────────────────────────
    let answer = CallSignal::Answer {
        sdp: "answer-sdp-bob".to_string(),
    };
    bob.send_call_signal(
        &alice_npub,
        &call_id,
        "video",
        &serde_json::to_string(&answer).unwrap(),
    )
    .await
    .unwrap();
    let alice_answer = wait_for(&mut alice_events, RECV_TIMEOUT, |e| {
        matches!(e, BridgeEvent::IncomingCallSignal(dto) if dto.call_id == call_id && matches!(dto.signal, CallSignal::Answer { .. }))
    })
    .await
    .expect("alice must receive the answer");
    let BridgeEvent::IncomingCallSignal(dto) = alice_answer else {
        unreachable!()
    };
    assert!(matches!(dto.signal, CallSignal::Answer { sdp } if sdp == "answer-sdp-bob"));

    // ── Trickled ICE both ways ───────────────────────────────────────────────
    let alice_ice = CallSignal::Ice {
        candidate: "candidate:1 1 UDP 2130706431 192.0.2.1 54321 typ host".to_string(),
        sdp_mid: Some("0".to_string()),
        sdp_m_line_index: Some(0),
    };
    alice
        .send_call_signal(
            &bob_npub,
            &call_id,
            "video",
            &serde_json::to_string(&alice_ice).unwrap(),
        )
        .await
        .unwrap();
    wait_for(&mut bob_events, RECV_TIMEOUT, |e| {
        matches!(e, BridgeEvent::IncomingCallSignal(dto) if dto.call_id == call_id && matches!(dto.signal, CallSignal::Ice { .. }))
    })
    .await
    .expect("bob must receive alice's trickled ICE candidate");

    // ── Hangup ends it, from the callee side ────────────────────────────────
    bob.hangup_call(&alice_npub, &call_id, "video", "normal")
        .await
        .unwrap();
    let alice_hangup = wait_for(&mut alice_events, RECV_TIMEOUT, |e| {
        matches!(e, BridgeEvent::IncomingCallSignal(dto) if dto.call_id == call_id && matches!(dto.signal, CallSignal::Hangup { .. }))
    })
    .await
    .expect("alice must receive the hangup");
    let BridgeEvent::IncomingCallSignal(dto) = alice_hangup else {
        unreachable!()
    };
    assert!(matches!(
        dto.signal,
        CallSignal::Hangup {
            reason: HangupReason::Normal
        }
    ));

    // ── Both sides log a consistent call record ─────────────────────────────
    let alice_record = alice
        .log_call(&bob_npub, &call_id, "video", false, "connected", 0, 42)
        .unwrap();
    let bob_record = bob
        .log_call(&alice_npub, &call_id, "video", true, "connected", 0, 42)
        .unwrap();
    assert_eq!(alice_record.id, call_id);
    assert_eq!(bob_record.id, call_id);
    assert!(!alice_record.incoming, "alice placed the call");
    assert!(bob_record.incoming, "bob received the call");
    assert_eq!(
        alice.call_history(Some(&bob_npub)).unwrap()[0].outcome,
        "connected"
    );
    assert_eq!(
        bob.call_history(Some(&alice_npub)).unwrap()[0].outcome,
        "connected"
    );

    relay.stop().await;
}

#[tokio::test]
async fn a_same_call_id_re_offer_reaches_the_callee_as_a_second_incoming_call_signal() {
    // WP5 (COMMS_ARCHITECTURE.md): the transport-layer half of the desktop P0
    // fixed in ADR-3/WP1. A caller falling back to TURN (ICE-restart) or
    // renegotiating mid-call sends a *second* `Offer` for the *same*
    // `call_id` — the wire has no `ice_restart` flag, so a re-offer is just
    // `CallSignal::Offer` reused as-is (`call.rs:123`), identified purely by
    // the unchanged `call_id`. That re-offer must reach the callee's runtime
    // as a second `IncomingCallSignal`: neither the 90s staleness gate
    // (`CALL_SIGNAL_MAX_AGE_SECS`, `runtime.rs:2833`) nor the wrapper-event-id
    // dedup (`CallSignalDedup`, `runtime.rs:2835`) may eat it, because each
    // `send_call_signal` gift-wraps with a fresh one-time key (NIP-59,
    // `vault.rs:235`) and so mints a fresh wrapper event id even though the
    // `call_id` carried inside the envelope is unchanged. This is what desktop
    // got wrong (`main.js:996-1014` answered a same-`call_id` re-offer with
    // `busy`) and what this test pins at the Rust layer so it can never
    // regress silently again.
    //
    // Wrapper-id dedup of a *literal* redelivery (the exact same wrapper
    // event id handed to `dispatch_incoming_dm` twice — relay at-least-once
    // delivery, or the 2-day backfill rescanning on every launch) is already
    // covered at the unit level by
    // `dispatch_drops_a_stale_call_signal_and_dedups_a_fresh_one` in
    // `runtime.rs`; this test deliberately does not repeat that coverage. It
    // proves the complementary, transport-level property: a second *distinct*
    // send sharing a `call_id` is never conflated with that redelivery.
    let relay = TestRelay::start().await;
    let alice_dir = TempDir::new().unwrap();
    let bob_dir = TempDir::new().unwrap();
    let alice = unlocked_runtime(&relay.url, &alice_dir).await;
    let bob = unlocked_runtime(&relay.url, &bob_dir).await;
    let alice_npub = alice.profile().unwrap().npub;
    let bob_npub = bob.profile().unwrap().npub;
    let mut alice_events = alice.subscribe_events();
    let mut bob_events = bob.subscribe_events();
    tokio::time::sleep(SETTLE).await;

    // Mutual accept — same dance as the offer/answer/ICE/hangup test above.
    alice
        .send_dm(&bob_npub, "let's talk before I call")
        .await
        .unwrap();
    wait_for(&mut bob_events, RECV_TIMEOUT, |e| {
        matches!(e, BridgeEvent::IncomingMessageRequest(_))
    })
    .await
    .expect("bob sees alice's request");
    bob.accept_request(&alice_npub).unwrap();

    // ── Caller (alice) places the call and sends the initial offer ─────────
    let call_id = alice.place_call(&bob_npub, "audio").unwrap().call_id;
    let first_offer = CallSignal::Offer {
        sdp: "offer-sdp-alice-stun".to_string(),
    };
    alice
        .send_call_signal(
            &bob_npub,
            &call_id,
            "audio",
            &serde_json::to_string(&first_offer).unwrap(),
        )
        .await
        .unwrap();
    let bob_first = wait_for(&mut bob_events, RECV_TIMEOUT, |e| {
        matches!(e, BridgeEvent::IncomingCallSignal(dto) if dto.call_id == call_id && matches!(dto.signal, CallSignal::Offer { .. }))
    })
    .await
    .expect("bob must receive the first offer");
    let BridgeEvent::IncomingCallSignal(dto) = bob_first else {
        unreachable!()
    };
    assert_eq!(dto.peer, alice_npub);
    assert!(matches!(dto.signal, CallSignal::Offer { sdp } if sdp == "offer-sdp-alice-stun"));

    // ── Callee (bob) answers ────────────────────────────────────────────────
    let answer = CallSignal::Answer {
        sdp: "answer-sdp-bob".to_string(),
    };
    bob.send_call_signal(
        &alice_npub,
        &call_id,
        "audio",
        &serde_json::to_string(&answer).unwrap(),
    )
    .await
    .unwrap();
    wait_for(&mut alice_events, RECV_TIMEOUT, |e| {
        matches!(e, BridgeEvent::IncomingCallSignal(dto) if dto.call_id == call_id && matches!(dto.signal, CallSignal::Answer { .. }))
    })
    .await
    .expect("alice must receive the answer");

    // ── The regression under test: alice re-offers the SAME call_id with a
    //    fresh SDP — a caller's ICE-restart/TURN-fallback re-offer (ADR-3) or
    //    a mid-call renegotiation. Bob must see it as a SECOND offer. ───────
    let re_offer = CallSignal::Offer {
        sdp: "offer-sdp-alice-turn-fallback-ice-restart".to_string(),
    };
    alice
        .send_call_signal(
            &bob_npub,
            &call_id,
            "audio",
            &serde_json::to_string(&re_offer).unwrap(),
        )
        .await
        .unwrap();
    let bob_second = wait_for(&mut bob_events, RECV_TIMEOUT, |e| {
        matches!(e, BridgeEvent::IncomingCallSignal(dto) if dto.call_id == call_id && matches!(dto.signal, CallSignal::Offer { .. }))
    })
    .await
    .expect(
        "a same-call_id re-offer (TURN fallback / ICE restart) must reach bob as a SECOND \
         IncomingCallSignal offer within CALL_SIGNAL_MAX_AGE_SECS — the staleness gate and the \
         wrapper-event-id dedup must not eat it",
    );
    let BridgeEvent::IncomingCallSignal(dto) = bob_second else {
        unreachable!()
    };
    assert_eq!(dto.peer, alice_npub);
    assert_eq!(
        dto.call_id, call_id,
        "the re-offer must keep the original call_id, not mint a new one"
    );
    assert!(
        matches!(dto.signal, CallSignal::Offer { sdp } if sdp == "offer-sdp-alice-turn-fallback-ice-restart"),
        "the second offer's SDP must be the fresh one — this must not be a stale re-delivery of the first offer"
    );

    // ── Hangup, from the caller side ────────────────────────────────────────
    alice
        .hangup_call(&bob_npub, &call_id, "audio", "normal")
        .await
        .unwrap();
    let bob_hangup = wait_for(&mut bob_events, RECV_TIMEOUT, |e| {
        matches!(e, BridgeEvent::IncomingCallSignal(dto) if dto.call_id == call_id && matches!(dto.signal, CallSignal::Hangup { .. }))
    })
    .await
    .expect("bob must receive the hangup");
    let BridgeEvent::IncomingCallSignal(dto) = bob_hangup else {
        unreachable!()
    };
    assert!(matches!(
        dto.signal,
        CallSignal::Hangup {
            reason: HangupReason::Normal
        }
    ));

    relay.stop().await;
}

#[tokio::test]
async fn simultaneous_offers_in_both_directions_each_reach_the_other_peer() {
    // WP5 glare case: alice and bob each place a call on the other at the
    // same instant, with different call_ids — true glare, not a single
    // caller backing off and retrying. This suite only proves the
    // *transport* carries both offers through to the other side; picking a
    // winner (by npub, per ADR-2's `decideGlare`) is frontend/CallManager
    // state-machine logic and out of scope for this Rust layer (see the
    // module-level scope note at the top of this file) — so there is
    // deliberately no "who wins" assertion here.
    let relay = TestRelay::start().await;
    let alice_dir = TempDir::new().unwrap();
    let bob_dir = TempDir::new().unwrap();
    let alice = unlocked_runtime(&relay.url, &alice_dir).await;
    let bob = unlocked_runtime(&relay.url, &bob_dir).await;
    let alice_npub = alice.profile().unwrap().npub;
    let bob_npub = bob.profile().unwrap().npub;
    let mut alice_events = alice.subscribe_events();
    let mut bob_events = bob.subscribe_events();
    tokio::time::sleep(SETTLE).await;

    // Mutual accept.
    alice
        .send_dm(&bob_npub, "let's talk before I call")
        .await
        .unwrap();
    wait_for(&mut bob_events, RECV_TIMEOUT, |e| {
        matches!(e, BridgeEvent::IncomingMessageRequest(_))
    })
    .await
    .expect("bob sees alice's request");
    bob.accept_request(&alice_npub).unwrap();

    let alice_call_id = alice.place_call(&bob_npub, "audio").unwrap().call_id;
    let bob_call_id = bob.place_call(&alice_npub, "audio").unwrap().call_id;
    let alice_offer = CallSignal::Offer {
        sdp: "offer-sdp-from-alice".to_string(),
    };
    let bob_offer = CallSignal::Offer {
        sdp: "offer-sdp-from-bob".to_string(),
    };

    // Fire both offers concurrently on the same poll — real glare, not a race
    // decided by test-code ordering. The JSON bodies are bound to locals
    // first: inlining them as `&serde_json::to_string(..).unwrap()` inside
    // `tokio::join!` drops the temporary before the join's internal polling
    // loop is done borrowing it (E0716).
    let alice_offer_json = serde_json::to_string(&alice_offer).unwrap();
    let bob_offer_json = serde_json::to_string(&bob_offer).unwrap();
    let (alice_sent, bob_sent) = tokio::join!(
        alice.send_call_signal(&bob_npub, &alice_call_id, "audio", &alice_offer_json),
        bob.send_call_signal(&alice_npub, &bob_call_id, "audio", &bob_offer_json),
    );
    alice_sent.unwrap();
    bob_sent.unwrap();

    let bob_saw = wait_for(&mut bob_events, RECV_TIMEOUT, |e| {
        matches!(e, BridgeEvent::IncomingCallSignal(dto) if dto.call_id == alice_call_id && matches!(dto.signal, CallSignal::Offer { .. }))
    })
    .await
    .expect("bob must receive alice's offer despite offering bob at the same instant");
    let alice_saw = wait_for(&mut alice_events, RECV_TIMEOUT, |e| {
        matches!(e, BridgeEvent::IncomingCallSignal(dto) if dto.call_id == bob_call_id && matches!(dto.signal, CallSignal::Offer { .. }))
    })
    .await
    .expect("alice must receive bob's offer despite offering alice at the same instant");

    let BridgeEvent::IncomingCallSignal(dto) = bob_saw else {
        unreachable!()
    };
    assert_eq!(dto.peer, alice_npub);
    assert!(matches!(dto.signal, CallSignal::Offer { sdp } if sdp == "offer-sdp-from-alice"));

    let BridgeEvent::IncomingCallSignal(dto) = alice_saw else {
        unreachable!()
    };
    assert_eq!(dto.peer, bob_npub);
    assert!(matches!(dto.signal, CallSignal::Offer { sdp } if sdp == "offer-sdp-from-bob"));

    relay.stop().await;
}

#[tokio::test]
async fn media_kind_defaults_to_audio_when_the_offer_never_arrives_and_call_logs_as_missed() {
    // Not every call reaches an answer — the ring can simply time out (the
    // Android/desktop `CallManager`'s job to enforce, see CallManagerTest.kt).
    // This pins the Rust-side half of that path: a call that never gets an
    // answer still produces a well-formed, queryable "missed" log entry on
    // the caller's side, which is what the call-history screen reads.
    let relay = TestRelay::start().await;
    let alice_dir = TempDir::new().unwrap();
    let alice = unlocked_runtime(&relay.url, &alice_dir).await;
    let stranger_npub = comrade_core::crypto::KeyProfile::generate().unwrap().npub;

    let session = alice
        .place_call(&stranger_npub, "bogus-media-kind")
        .unwrap();
    assert_eq!(
        session.media,
        CallMediaKind::Audio.as_str(),
        "unknown media kind falls back to audio"
    );

    let record = alice
        .log_call(
            &stranger_npub,
            &session.call_id,
            "audio",
            false,
            "missed",
            0,
            0,
        )
        .unwrap();
    assert_eq!(record.outcome, "missed");
    assert_eq!(record.duration_secs, 0);

    relay.stop().await;
}
