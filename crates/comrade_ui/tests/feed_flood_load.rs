//! COMMS-04 load test: a public-feed flood must never delay, or drop, a DM
//! or call signal — those live on [`comrade_ui::ComradeRuntime`]'s separate,
//! large "critical" event bus, while `IncomingChitthi` alone rides the small,
//! deliberately-lossy feed bus (see `runtime.rs`'s `EVENT_BUS_CAPACITY` /
//! `FEED_EVENT_BUS_CAPACITY` split). Marked `#[ignore]` — it floods a local
//! relay with ~1,000 events and is a load/soak test, not a per-commit unit
//! test; run it explicitly (`cargo test -- --ignored`) or via the dedicated
//! CI load-test job (`.github/workflows/ci.yml`).

mod support;

use std::sync::Arc;
use std::time::Duration;

use comrade_ui::{BridgeEvent, ComradeRuntime};
use support::TestRelay;
use tempfile::TempDir;

const RECV_TIMEOUT: Duration = Duration::from_secs(10);
const SETTLE: Duration = Duration::from_millis(150);
/// Generous — the point isn't "instant", it's "not held hostage by a flood
/// of unrelated public notes". A real product latency budget would be
/// tighter; this is the load test's outer bound before it fails the build.
const DM_LATENCY_BUDGET: Duration = Duration::from_secs(3);
const FLOOD_SIZE: usize = 1_000;

async fn unlocked_runtime(relay_url: &str, dir: &TempDir) -> ComradeRuntime {
    let mut rt = ComradeRuntime::with_relays(vec![relay_url.to_string()]);
    rt.unlock_vault(dir.path(), "pin").await.unwrap();
    rt.spawn_event_loops();
    rt
}

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
            Ok(Ok(_)) => continue,
            Ok(Err(_)) | Err(_) => return None,
        }
    }
}

#[tokio::test]
#[ignore = "load test: floods a local relay with ~1000 events — run via `cargo test -- --ignored`"]
async fn public_feed_flood_never_delays_a_concurrent_dm() {
    let relay = TestRelay::start().await;
    let alice_dir = TempDir::new().unwrap();
    let bob_dir = TempDir::new().unwrap();
    let flooder_dir = TempDir::new().unwrap();
    let alice = unlocked_runtime(&relay.url, &alice_dir).await;
    let bob = unlocked_runtime(&relay.url, &bob_dir).await;
    // Bob has no contacts yet, so his feed subscription is the no-contacts
    // bootstrap scope (`FeedScope::BoundedGlobal`, no author filter) — the
    // flooder's notes do match it, which is exactly the scenario under test:
    // even a subscription that *does* see the flood must not let it delay
    // critical traffic.
    let flooder = Arc::new(unlocked_runtime(&relay.url, &flooder_dir).await);
    let bob_npub = bob.profile().unwrap().npub;
    let mut bob_critical = bob.subscribe_events();
    let mut bob_feed = bob.subscribe_feed_events();
    tokio::time::sleep(SETTLE).await;

    // Fire the flood and the DM concurrently — the DM genuinely races the
    // flood over the same local relay, rather than being queued behind it.
    let flood = tokio::spawn(async move {
        let mut handles = Vec::with_capacity(FLOOD_SIZE);
        for i in 0..FLOOD_SIZE {
            let flooder = flooder.clone();
            handles.push(tokio::spawn(async move {
                let _ = flooder.broadcast_chitthi(&format!("spam #{i}"), None).await;
            }));
        }
        for h in handles {
            let _ = h.await;
        }
    });

    let dm_started = tokio::time::Instant::now();
    alice.send_dm(&bob_npub, "still here?").await.unwrap();
    let delivered = wait_for(&mut bob_critical, RECV_TIMEOUT, |e| {
        matches!(e, BridgeEvent::IncomingMessageRequest(_))
    })
    .await
    .expect("a DM sent during a public-feed flood must still be delivered, not dropped");
    let latency = dm_started.elapsed();
    assert!(
        latency < DM_LATENCY_BUDGET,
        "DM delivery must stay within budget under a public-feed flood, took {latency:?}"
    );
    let BridgeEvent::IncomingMessageRequest(req) = delivered else {
        unreachable!()
    };
    assert_eq!(req.last_message, "still here?");

    flood.await.unwrap();

    // Memory stays bounded: nobody drained `bob_feed` during the flood, yet
    // reading it now yields nowhere near `FLOOD_SIZE` entries — the small
    // feed channel dropped old, unconsumed Chitthis instead of buffering
    // (or blocking a producer on) the whole flood.
    let mut drained = 0usize;
    loop {
        match tokio::time::timeout(Duration::from_millis(200), bob_feed.recv()).await {
            Ok(Ok(_)) => drained += 1,
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(n))) => {
                assert!(
                    n > 0,
                    "a flood this size hitting an idle receiver must report dropped events"
                );
            }
            _ => break, // drained (timed out) or channel closed
        }
    }
    assert!(
        drained < FLOOD_SIZE / 2,
        "the feed channel must not have buffered the whole flood ({drained}/{FLOOD_SIZE} entries) \
         — event drops must be observable and bounded, not silently unbounded growth"
    );

    relay.stop().await;
}
