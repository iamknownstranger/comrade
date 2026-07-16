# Comrade TURN relay (coturn)

Makes voice/video calls connect when a direct/STUN path can't (CGNAT, some
corporate/campus networks) — AUDIT.md COMMS-02. STUN-first behavior is
unconditional on the client side (`comrade_core::call::IceStrategy`): every
call's first attempt is STUN-only, and this relay is only contacted after
that attempt fails to reach a connected ICE state.

## Deploy

1. **DNS.** Point an A/AAAA record (e.g. `turn.example.com`) at this host.
2. **TLS.** `turns:` (TLS) is the default this deployment assumes — a plain
   `turn:` relay is observable and easy to block in transit. Get a cert:
   ```sh
   sudo certbot certonly --standalone -d turn.example.com
   sudo mkdir -p deploy/coturn/certs
   sudo cp /etc/letsencrypt/live/turn.example.com/fullchain.pem deploy/coturn/certs/
   sudo cp /etc/letsencrypt/live/turn.example.com/privkey.pem deploy/coturn/certs/
   ```
   Renew the same way certbot renews any other cert on this host (a cron/systemd
   timer calling `certbot renew`, then restarting the `coturn` container so it
   picks up the refreshed files).
3. **Fill in `turnserver.conf`**: `static-auth-secret` (a long random value —
   e.g. `openssl rand -hex 32`) and `realm` (your domain). Keep the secret out
   of version control; the checked-in file is a template.
4. `docker compose -f deploy/coturn/docker-compose.yml up -d`

## Time-limited credentials, not a static shared secret

This relay is configured for coturn's `use-auth-secret` ("TURN REST API")
mode: the *shared secret* lives only on this server (never inside the
Comrade app — a secret baked into every install could never be revoked
without rotating it for everyone at once), and each caller instead receives a
**minted, expiring** username/password pair.

Minting is: `username = "<unix-expiry>"`, `password =
base64(HMAC-SHA1(shared_secret, username))`. Comrade ships the pure, tested
implementation of this at `comrade_core::call::mint_turn_rest_credentials` —
but no broker service of its own (Comrade has no central account server to
host one on). An operator who wants real time-limited credentials runs their
own small broker that:

1. Holds `static-auth-secret` (never sent to any client).
2. On request, calls `mint_turn_rest_credentials(secret, now, ttl_secs, None)`
   (or the equivalent one-line computation below) and returns the pair.
3. Rate-limits/authenticates that endpoint however fits the deployment (this
   repo intentionally doesn't prescribe an accounts/auth model).

The equivalent computation without building any Rust, for a quick manual
test or a non-Rust broker:

```sh
SECRET="the-static-auth-secret-from-turnserver.conf"
EXPIRY=$(( $(date +%s) + 3600 ))   # valid for 1 hour
PASSWORD=$(printf '%s' "$EXPIRY" | openssl dgst -sha1 -hmac "$SECRET" -binary | base64)
echo "username=$EXPIRY"
echo "password=$PASSWORD"
```

The app's Settings screen (`ComradeCore.setTurnServerTyped`) takes whatever
username/password it's given — either this minted, expiring pair from an
operator's broker, or (the simpler, lower-effort option for a
single-operator/self-hosted deployment) a long-term static user configured
directly in `turnserver.conf` via coturn's `user=` directive instead of
`use-auth-secret`. Both are legitimate choices; only the latter carries the
"this credential works forever until someone manually revokes it" tradeoff
the REST mode exists to avoid.

## Baked-in default relay (the CI `TURN_URL`/`TURN_USERNAME`/`TURN_PASSWORD` values)

The Android workflows bake `TURN_URL`/`TURN_USERNAME`/`TURN_PASSWORD` (GitHub
Actions secrets, or repository variables as a fallback) into the APK as the
default relay (`CallRelayDefaults`). That pair is sent to coturn **as-is, as a
long-term credential** — the app never re-derives anything from it. So the
server and the baked-in values must agree on the auth mode:

| coturn mode | GitHub values that work |
|---|---|
| `lt-cred-mech` + `user=alice:s3cret` | `TURN_USERNAME=alice`, `TURN_PASSWORD=s3cret` |
| `use-auth-secret` (this template) | Only a pre-minted REST pair (`TURN_USERNAME=<unix-expiry>`, `TURN_PASSWORD=base64(HMAC-SHA1(secret, username))`) — which **expires at `<unix-expiry>`**, silently breaking every install built from it after that moment |

The common failure looks like this: coturn deployed from this template
(`use-auth-secret`), and the `static-auth-secret` — or an arbitrary made-up
user/password — set as `TURN_USERNAME`/`TURN_PASSWORD`. coturn then answers
every allocation with 401, the app gathers no `relay` candidates, and calls
that need the relay (CGNAT, strict NATs) sit on "Connecting…" until the
connect timeout fails them. If you want a baked-in default that keeps working
across builds, run coturn with a static user instead:

```
# turnserver.conf — static-credential variant (replaces the REST block)
lt-cred-mech
user=comrade:REPLACE_ME_WITH_A_LONG_RANDOM_PASSWORD
realm=REPLACE_ME_WITH_YOUR_DOMAIN
```

and set the same pair as the GitHub values. (Accepting the tradeoff above:
that pair ships inside every APK and works until you rotate it.)

**Verify before shipping**: the app's Settings screen has a TURN relay
diagnostic (`CallManager.testTurnConnectivity`) that performs a real
allocation against the configured relay and reports whether a `relay`
candidate came back — run it on a build with the baked-in values (or after
typing the same values into Settings) rather than assuming the handshake
works. From a shell, `turnutils_uclient -u <user> -w <password> -y
turn.example.com` exercises the same path.

## Never logged

Neither the shared secret nor any minted credential is ever written to a log
by Comrade — `ComradeCore.setTurnServerTyped`'s username/credential
parameters are write-only from the moment they're persisted (see
`turnServerStatusTyped`, which reports only the relay URL). coturn itself is
configured (`no-stdout-log`, `simple-log`) to keep its own logging away from
relayed packet contents, which are DTLS-SRTP ciphertext regardless.

## Testing without a relay

`deploy/test-relay/` provisions an isolated *Nostr* relay for signaling
tests, not a TURN relay — for a TURN-relay-specific test, either point a
throwaway `docker compose -f deploy/coturn/docker-compose.yml up -d` at
this same host during test setup (self-signed cert is fine for that), or use
`CallManager.forceRelayOnly`/`iceTransportsType = RELAY` (AUDIT.md COMMS-02's
test fixture) to force any configured relay to be exercised.
