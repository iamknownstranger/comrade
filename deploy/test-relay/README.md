# Isolated test relay

An isolated Nostr relay for integration and device testing (AUDIT.md
COMMS-03), so tests never depend on the public relay pool's availability,
rate limits, or content.

**The hermetic Rust suite doesn't need this.** `crates/comrade_ui/tests/`
embeds its own minimal in-process relay (see `tests/support/mod.rs`) and runs
with `cargo test` alone — no Docker, no network. This compose file is for the
tests that need a relay reachable over a real socket instead of in-process:

- The Android two-peer instrumented test
  (`android/app/src/androidTest/java/mullu/comrade/TwoPeerJniIntegrationTest.kt`),
  which needs an address the emulator's own network namespace can reach.
- Manual end-to-end testing (desktop, a sideloaded APK) against a real relay
  implementation instead of the public pool.

## Run it

```sh
docker compose -f deploy/test-relay/docker-compose.yml up -d
```

The relay listens at `ws://localhost:8090` on the host. From inside an
Android emulator, the host's `localhost` is reachable at the standard
emulator-to-host loopback address `10.0.2.2`, so point the app/test at
`ws://10.0.2.2:8090`.

## Wiring it into the Android instrumented test

```sh
adb shell am instrument -w \
  -e comradeTestRelayUrl "ws://10.0.2.2:8090" \
  -e class mullu.comrade.TwoPeerJniIntegrationTest \
  mullu.comrade.test/androidx.test.runner.AndroidJUnitRunner
```

Without `comradeTestRelayUrl`, `TwoPeerJniIntegrationTest` skips itself
(`Assume.assumeTrue`) rather than silently falling back to the public relay
pool — a two-peer test flaking on a relay it doesn't control would defeat the
point of an isolated environment.

## Stop / reset

```sh
docker compose -f deploy/test-relay/docker-compose.yml down -v   # -v also drops the event database
```

## Honesty note

This compose file was written without a Docker daemon available in the
authoring environment, so it has not been run end-to-end here — validate it
locally or in CI before depending on it. `scsibug/nostr-rs-relay` is a
well-established, actively-used NIP-01 relay implementation; `config.toml`
only overrides the fields that matter for testing (address/port, generous
rate limits so the COMMS-04 load test measures Comrade's own bottlenecks, not
the relay's).
