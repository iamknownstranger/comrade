# Tara — the reflective companion

_Design note for wellbeing pillar #4 (AUDIT §8). Status: v1 shipped 2026-07-22._

Tara is a private, on-device space to think out loud: she mirrors feelings,
asks reflective questions, scaffolds brainstorming, and nudges journaling.
She is deliberately **not** a chatbot pretending to be a person, and above
all **not therapy**.

## The two honesty gates (non-negotiable)

These come from AUDIT §8 and every future change to Tara must keep them true:

1. **Not therapy, and it says so.** Tara never diagnoses, never treats, never
   presents as a clinician. The user opts in through an explainer that says
   exactly this; a persistent footer repeats it; and when a message carries
   distress cues Tara *stops prompting* and hands off to real crisis
   helplines. The hand-off is regression-tested
   (`comrade_core::tara::tests`, `comrade_ui` lifecycle tests).
2. **On-device or not at all.** Raw mental-health disclosures must never be
   routed to a cloud API — that would contradict the product's core promise
   exactly where it matters most. The v1 engine is deterministic Rust with
   zero network access, so the guarantee holds *by construction*, not by
   policy.

## What shipped in v1

| Layer | What |
|---|---|
| Engine | `comrade_core::tara` — `CompanionEngine` trait + `ReflectiveCompanion` impl: greeting/feeling/advice/default reply families, turn-seeded prompt rotation, `detect_distress` cue matcher, `CRISIS_RESOURCES` (Tele-MANAS, KIRAN, AASRA, findahelpline.com) |
| Storage | `tara_companion` tree in the encrypted store (`TaraMessage`: id, text, `from_tara`, `crisis`, `created_at`); oldest-first thread; `clear_tara_messages`; ciphertext-at-rest proven by test, same as the journal |
| View-model | `ComradeRuntime::tara_send / tara_thread / clear_tara_thread / tara_opener / tara_crisis_resources` + `TaraMessageDto` / `CrisisResourceDto` |
| Android | **Tara** bottom-nav tab → `TaraScreen`: opt-in explainer (stored in `tara` prefs), chat bubbles, crisis card under any flagged reply, persistent "not a therapist" footer, clear-conversation dialog |
| Desktop | The five Tauri commands are registered (`desktop/src-tauri`); the vanilla-JS web UI predates the wellbeing pillars and does not render Tara yet (same state as the journal) |

Sequence-numbered store ids (`{timestamp}-{seq}`) keep user/reply pairs in
exact send order even within the same second — random id tails would let
pairs interleave.

## Privacy posture

- The thread exists only inside the encrypted store (Argon2id + AES-256-GCM);
  no relay, no network, no analytics.
- The opener nudge ("two low days this week…") reads journal **mood markers
  and entry age only** — never journal text. Data minimisation is the point:
  the companion doesn't need your words to invite you to reflect.
- "Clear conversation" deletes every turn; there is no other copy to forget.

## Crisis hand-off behaviour

`detect_distress` is a normalising, whole-phrase cue matcher, deliberately
conservative **in favour of showing help**: a false positive costs one extra
card of helpline numbers; a false negative costs the hand-off itself. When it
fires, both the user turn and the reply are stored with `crisis = true`, the
reply is a fixed hand-off message (no reflective prompt), and every frontend
must render the crisis resources with it — the flag is part of the DTO
contract, not a UI nicety.

## OQ9 and the LLM slot

The AUDIT's OQ9 asks: on-device quantised LLM vs. template-only vs. cloud.
v1 ships the **template-only** option so the surface, storage, safety and
FFI plumbing are real while the model/runtime half of OQ9 stays an owner
decision. The `CompanionEngine` trait is the seam:

- An on-device backend (llama.cpp-class or candle-class runtime, small
  quantised weights fetched like the Vosk model — one-time, sha256-verified,
  in-app) implements `reply`/`opener` and slots in behind the same
  `tara_send` path. **The `detect_distress` gate must stay in front of any
  model** — the crisis hand-off is not delegated to model behaviour.
- A cloud backend must never implement the trait (gate 2).

Open follow-ups: desktop web UI surface, a `tara <text>` voice-dispatcher
command (`ComradeBackend` already has the shape for it), and the OQ9
model/runtime decision itself.
