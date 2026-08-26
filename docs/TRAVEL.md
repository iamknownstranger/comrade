# Travel — where the locals eat, and what this place is

_Added 2026-08-16, from an owner request: "a travel tab, which actually lists out
all the legendary local restaurants, which are reviewed by thousands of people on
Google Maps. Also, it should show things to do — attractions or any interesting
facts of the place using the current location of the user."_

A tab that answers two questions from one coordinate. They sound like one
question and they are not, and conflating them is what makes this area
awkward:

1. **Where do I eat?** — the places thousands of people have already reviewed.
2. **What is this place?** — attractions, landmarks, and the sentence that makes
   a street worth walking down.

The first has exactly one credible public source and it costs money. The second
has several and they are free. This document is the design record: what is
sourced from where, what "legendary" is defined as, what leaves the device, and
which half of it is checked before CI.

---

## 1. What this is not

**Not a maps app.** There is no map view, no routing, no navigation. Tapping a
place hands it to whatever the user already uses for maps
(`TravelDecisions.openTarget`). Building a map surface would mean either a tile
budget or an embedded SDK, and neither is what was asked for.

**Not a review platform.** Comrade stores no ratings, collects none, and syncs
none. Every number on the screen is attributed to the source that supplied it,
on the card, next to the number.

**Not a place that ships an API key.** See §4. This is the single most
consequential constraint in the feature, and it shapes the whole default
experience.

**Not a location beacon.** Nothing about the user's position is broadcast to a
peer, put on a relay, or written to the vault. The one thing that travels is a
deliberately blurred coordinate, to a public API, on demand. See §3.

## 2. Where the data comes from

| source | supplies | needs an account | notes |
| --- | --- | --- | --- |
| **Google Places (New)** | ratings, **review counts**, canonical map links, editorial one-liners | **yes — the user's own key** | the only public source with review counts in the thousands |
| **OpenStreetMap** (Overpass) | names, kinds, coordinates, cuisine tags, addresses | no | holds no opinions, so never a rating |
| **Wikipedia** (`generator=geosearch` + `prop=extracts`) | the facts panel, one round trip | no | trimmed to two sentences by `first_sentences` |

`comrade_core::travel::merge_places` folds the OSM and Google views of one
restaurant into one row: the rating always comes from whichever record has one,
and everything else prefers the record that already has a value, so an OSM
cuisine tag survives a Google row that has none. Two records are the same place
when their names normalise identically **and** they are within 80 m — the
proximity half is what stops two branches of one chain on the same road merging
into one.

**Why Google at all, in an app whose pitch is sovereignty.** Because the request
was for places "reviewed by thousands of people on Google Maps", and there is no
open dataset with a million reviews in it. Pretending otherwise — shipping an
OSM-only tab and calling the results legendary — would be the polite lie. What
the design does instead is make the dependency *optional, visible, and the
user's own*: the tab works without it, says so on the guide itself, and never
carries a credential Comrade owns.

## 3. What leaves the device

**The coordinate that goes out is not the coordinate of the phone.** Every
outbound query is built from `travel::coarse_origin` — the centre of the ~150 m
geohash cell the user is standing in (`geo::Precision::Block`). Distances shown
on screen are computed against the *real* fix, locally, so the UI stays accurate
while the wire stays vague.

Why that precision and not another:

- `Neighborhood` (~1.2 km) was the first instinct and is wrong: the blur becomes
  comparable to the whole search radius, and the answer stops being about where
  the user actually is.
- `Building` (~40 m) is a doorway.
- `Block` (~150 m) hides the building without moving the neighbourhood, and is
  an order of magnitude smaller than the smallest legal radius (500 m).

Two fixes a few metres apart produce a byte-identical request
(`two_fixes_in_one_block_produce_the_same_query`), which is the property that
makes this a blur rather than a rounding.

Beyond the coordinate and a radius, a request carries nothing: no npub, no
contact, no indication of who is asking. The Google request additionally carries
the user's key — in a header, never the query string, so it does not land in
every proxy log between here and Mountain View.

**Android asks for `ACCESS_COARSE_LOCATION` only.** Fine location would be asking
for precision the app immediately throws away. The platform `LocationManager` is
used rather than Play Services' fused client, which also avoids the
`app/`-compiles-`android/`'s-Kotlin dependency trap in `CLAUDE.md`.

**The guide cache is in memory and dies with the process.** The guide's contents
are public facts about public places, but the set of cells it is keyed by is a
record of where this person has stood — so it is never persisted, and
`ComradeRuntime::lock_vault` clears it.

## 4. "Legendary" is a definition, not a vibe

A 5.0 from three friends of the owner is not a legend; 4.6 across twelve thousand
strangers is. `travel::legend_score` therefore never ranks on the raw star
average:

```
score = bayesian_rating(rating, votes) × crowd_weight(votes)
```

- `bayesian_rating` pulls the average toward 3.9 with 250 imaginary middling
  reviews, so a perfect score from four people cannot top the list.
- `crowd_weight` is logarithmic, reaching 1.0 at `LEGEND_MIN_REVIEWS` (1 000 —
  "thousands of people", taken literally) and **capped at 1.4**, so a mediocre
  place with 400 000 reviews (an airport, a chain by a station) cannot buy its
  way past a genuinely loved one with 8 000.

`is_legendary` is a stricter and separate question — the *badge*, needing both
≥ 4.3 stars and ≥ 1 000 reviews. It is separate from the ranking because a list
has to be ordered even when nothing in it earns a badge, and it lives in core so
three frontends cannot disagree about what a legend is.

An unrated place scores `0.0` and falls back to distance. That is deliberate: an
unrated place is not a badly rated one, and hiding it would empty the tab for
every user without a key.

## 5. The key is the user's

There is no Google Places key in this repository and there will not be one. An
app binary is not a secret store; a shipped key is a key that gets scraped,
billed to whoever owns it, and revoked out from under every install.

- The user supplies their own in **Settings → Google Places API key**.
- It is stored in the **encrypted vault** (`app_settings`/`travel_places_api_key`),
  not a plaintext preference, because it is billable.
- It is **write-only from the UI**: `travel_ratings_configured()` returns whether
  there is one, never the key itself. `ApiKey` has a hand-written `Debug` that
  redacts, so it cannot reach a crash report through a `{:?}` on a struct.
- A blank key **clears** the setting rather than storing something that will fail
  every request with an opaque 403.
- Changing the key clears the guide cache, so adding one does not appear to do
  nothing until the TTL expires.

**A missing key is a notice, not a failure**, and this is the one place the
"fail fast on missing config" rule is applied as *loud* rather than as *fatal*.
Failing the whole guide when OpenStreetMap and Wikipedia both answered would
throw away the working two thirds of the feature; so the guide comes back with
`ratings_from: None` and a sentence saying ratings need a key
(`TRAVEL_NO_KEY_NOTICE`). What must never happen is those two rendering the same:
"no ratings configured" and "nothing legendary near you" are different answers,
and `TravelDecisions.ratingsExplainer` is the one function that keeps them apart.

## 6. Degrading honestly

| situation | what the user sees |
| --- | --- |
| no location permission | a prompt, with the privacy line, and nothing pretending to load |
| permission but no fix | "couldn't get a fix", with a retry — not a spinner that never stops |
| a fix older than 10 minutes, nothing on screen | the same as no fix; a guide built on it would be about where they were |
| ratings provider unconfigured | the full OSM + Wikipedia guide, plus the key notice |
| a provider fails mid-fetch | the rest of the guide, plus a notice naming what failed |
| everything fails, but a cell was fetched before | the old guide, marked stale, with "checked N h ago" |
| everything fails, nothing cached | `UiError::Travel` with the reason |
| built without `travel-http` | `UiError::TravelUnavailable` — *never* an empty guide |

The last row is the same argument `UiError::CatalogueUnavailable` was added for:
"this build cannot look places up" and "there is nothing near you" must not be
the same screen.

## 7. Layering

```
comrade_core::travel     model, scoring, blur, query builders, parsers, merge, cache
        ↓                (the socket is behind `travel-http`; the parsers are not)
comrade_ui               TravelPlaceDto / TravelFactDto / TravelGuideDto,
                         `travel_guide` (a FREE function), the key in app_settings
        ↓
comrade_jni              `Comrade::travel_guide` / `set_travel_api_key` (uniffi)
        ↓
android/travel/          TravelDecisions (pure, JVM-tested) + TravelLocation
android/ui/TravelScreen  widgets only
```

**`travel_guide` is a free function and that is load-bearing.** It reads nothing
from `ComradeRuntime`: the caller clones a `TravelCache` handle and reads the API
key under a short read lock, then calls it with **no lock held**. Up to five HTTP
round trips inside a held `RwLock` is the exact shape of the two deadlocks this
repo has already fixed.

**Response parsers are compiled unconditionally**, unlike `catalogue.rs`'s. The
response format is the part that breaks when an upstream changes, and a fixture
test that only runs under a feature nobody enables in CI is a test that does not
run. Only the socket is behind `travel-http`.

## 8. What is checked, and what is not

**Checked here (this sandbox):**

- 40 `comrade_core::travel` unit tests, in both feature configurations —
  scoring, the blur, ranking, merging, all three parsers, the cache, and the
  HTTPS refusal.
- 7 `comrade_ui` runtime tests — cache hit, stale fallback, the
  `TravelUnavailable` distinction, lock-clears-the-cache, key storage.
- 23 `TravelDecisions` JVM tests, run on a plain `kotlinc` + JUnit.
- Both Android typecheck lanes.

**Not checked anywhere before CI:**

- How the screen *looks*. `.claude/rules/android.md`'s early-return rule was
  followed (every branch is an `if`/`when`), and §10's TRAVEL-1 closed the
  emulator-cover gap this line used to name — but that test is itself unrun
  here (no Android SDK), so CI's emulator lane is still the first place either
  claim is actually checked.
- Every real network response. Every parser test is a fixture; nothing here has
  ever spoken to Overpass, Wikipedia or Google.
- `app/` (Flutter). The Travel tab is **Android-only** for now — see §9.

## 9. Android first, and what that leaves owed

Per `CLAUDE.md` §11 / `docs/FRONTEND_STRATEGY.md` §11, Android is the priority
frontend, and this feature landed there. Recorded plainly rather than implied:

- **`desktop/` does not have a Travel tab.** The Tauri shell's CSP allows
  `connect-src 'self' ipc:` only, so the JS UI cannot reach Overpass directly and
  a Tauri command would have to carry it — and `desktop/src-tauri` is a lane this
  sandbox cannot compile at all.
- **`app/` (Flutter) does not have a Travel tab.** The FFI surface it would need
  (`travel_guide`) is exposed over **uniffi only**; `api.rs` gained the two new
  `UiError` variants (which the bridge must mirror) and nothing else.

Both are parity debt, not a closed decision.

## 10. Follow-ups worth naming

- ~~**TRAVEL-1 — no emulator cover.**~~ **— done 2026-08-21.**
  `MainActivityUiTest` now opens the Travel tab (coarse location pre-granted so
  it actually walks Locating → Loading → a terminal state) and waits until
  either the guide (`travel_guide` tag) or a terminal error/no-fix state
  ("Try again") appears, then takes one more action to prove the screen
  survived the recomposition — the same proof the Tasks and Ride legs already
  give. Still unrun on a device or in CI; this sandbox has no Android SDK.
- **TRAVEL-2 — the guide is not reachable offline.** The cache is per-session and
  in memory, so a guide fetched on hotel WiFi is gone by the time it is useful on
  the street. Persisting it would mean writing where somebody has been into the
  vault, which §3 refuses today. Exit: a deliberate, opt-in "keep this guide"
  that stores one cell, not a history.
- **TRAVEL-3 — no way to hand a place to a comrade.** The obvious next thing
  ("meet me here") is a DM control envelope carrying a place, and it is
  deliberately not built: it would put a coordinate on the wire between two
  people, which is a decision `docs/RIDE.md` §1 already declined once and which
  wants its own argument.
- **TRAVEL-4 — English Wikipedia only.** `WIKIPEDIA_BASE` is `en.wikipedia.org`,
  in an app that ships Hindi strings. Exit: pick the wiki from the device locale.
