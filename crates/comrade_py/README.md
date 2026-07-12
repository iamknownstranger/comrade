# comrade_py

Python bindings for two of Comrade's core Nostr engines, built with
[PyO3](https://pyo3.rs) and packaged as a wheel via
[maturin](https://www.maturin.rs). Drop the compiled library into a script
for local data analysis or a custom integration — no Android/Tauri UI
required.

Scope is deliberately narrow: only the two engines the root
[README](../../README.md#what-it-does) marks **✅ Wired** are exposed —

| Engine | Fetch | Publish |
|--------|-------|---------|
| **Sabha** (public feed) | `fetch_sabha_timeline()` | `broadcast_chitthi()` |
| **Vault** (E2E DMs) | `conversations()` / `messages_with()` | `send_dm()` |

Calls, media, journal, contacts, and the experimental Saathi/Sakha engines
are intentionally not bound here — they're app UX, not core data-fetching /
message-publishing, and some are still 🧪 experimental.

## Build

```sh
cd crates/comrade_py
python -m venv .venv && source .venv/bin/activate
pip install maturin
maturin develop            # build + install into the active venv for local testing
# or: maturin build --release   # produce a wheel in target/wheels/
```

## Usage

```python
import comrade_py

# Identity — a secp256k1 keypair, generated entirely locally.
keys = comrade_py.generate_keypair()   # {"npub": "npub1…", "nsec": "nsec1…"}

client = comrade_py.ComradeClient()
identity = client.unlock_vault("./comrade-data", "a strong passphrase")

# Data fetching
timeline = client.fetch_sabha_timeline()   # list[dict]
history = client.messages_with("npub1...peer")

# Message publishing
event_id = client.broadcast_chitthi("hello from a Python script")
client.send_dm("npub1...peer", "hi!")
```

Errors raise `comrade_py.ComradeError` (a plain `Exception` subclass) rather
than returning an error payload, so use ordinary `try`/`except`.

## Status

No OSS license has been chosen for the `comrade` repository yet (tracked as
an open question in the root [`AUDIT.md`](../../AUDIT.md)) — treat this
wheel as source-available for now, not cleared for redistribution.
