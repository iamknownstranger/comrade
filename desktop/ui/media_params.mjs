/* ============================================================================
 * Comrade desktop — pure media-parameter-building helpers
 *
 * Desktop calling captures video unconstrained today
 * (`getUserMedia({ audio: true, video: c.media === "video" })`, historically
 * around `main.js:900` before later WPs shifted line numbers) and never caps
 * the outgoing video bitrate. Android's `CallManager` (Camera2 capture) is
 * bounded: `CAMERA_WIDTH = 1280`, `CAMERA_HEIGHT = 720`, `CAMERA_FPS = 30`
 * (android/app/src/main/java/mullu/comrade/call/CallManager.kt:91-93). This
 * module gives desktop the same capture bound, plus an outgoing-video
 * bitrate cap Android's encoder doesn't explicitly set today either — the
 * cap is a new, desktop-only policy (~1.5 Mbps, comparable to a mobile-
 * friendly 720p30 encode) introduced to keep a TURN-relayed or otherwise
 * constrained link from being flooded — see
 * docs/COMMS_ARCHITECTURE.md WP17.
 *
 * Deliberately pure and dependency-free: no DOM, no WebRTC, no Tauri, no
 * Node built-ins, no imports at all — same discipline as
 * `desktop/ui/call_decisions.mjs` (see that file's header). That is what
 * makes it runnable under `node --test` with zero npm dependencies. Put any
 * getUserMedia/RTCRtpSender glue in main.js instead; this file only builds
 * the plain-object parameters those calls take.
 * ========================================================================== */

/**
 * The `getUserMedia` video constraints used for a video call. `ideal` (never
 * `min`/`exact`) on every bound, so a webcam that can't do 720p30 still
 * negotiates down to whatever it can do instead of hard-failing the call —
 * mirrors Android's fixed Camera2 capture size
 * (CallManager.kt:91-93: `CAMERA_WIDTH = 1280`, `CAMERA_HEIGHT = 720`,
 * `CAMERA_FPS = 30`) without Android's hard camera-format selection, since a
 * webview has no equivalent capture-format enumeration/selection step.
 *
 * @param {"audio"|"video"|string|null|undefined} media - The call's media kind, as carried on the call session (desktop: `state.call.media`).
 * @returns {{audio: true, video: (false|{width: {ideal: number}, height: {ideal: number}, frameRate: {ideal: number, max: number}})}} A full `getUserMedia` constraints object. Audio is always requested (`audio: true`, unconstrained — voice calls have never bounded audio, and WP17 is scoped to video only). `video` is `false` for anything other than the exact string `"video"`, mirroring the pre-existing `c.media === "video"` check this replaces — an unrecognized/missing media kind is treated as audio-only, never as an error.
 */
export function buildCaptureConstraints(media) {
  if (media !== "video") return { audio: true, video: false };
  return {
    audio: true,
    video: {
      width: { ideal: 1280 },
      height: { ideal: 720 },
      frameRate: { ideal: 30, max: 30 },
    },
  };
}

/**
 * Build a new `RTCRtpSendParameters`-shaped object with `encodings[0].maxBitrate`
 * set to `maxBitrate`, for capping a video `RTCRtpSender`'s outgoing bitrate
 * (desktop: `sender.setParameters(buildVideoSenderParameters(sender.getParameters(), cap))`,
 * §ADR-3/WP17).
 *
 * Purity contract: never mutates `existingParams` (or anything reachable from
 * it) — always returns a **new** top-level object, with a **new** `encodings`
 * array, whose entries are themselves **new** objects (shallow copies).
 * Callers must use the returned object, e.g.:
 *   `await sender.setParameters(buildVideoSenderParameters(sender.getParameters(), cap));`
 * — there is no in-place mutation to rely on.
 *
 * Every field already on `existingParams` (notably `transactionId`, which
 * some implementations require to round-trip unchanged between
 * `getParameters()` and `setParameters()`, plus `codecs`,
 * `degradationPreference`, etc.) is preserved via a shallow spread — only
 * `encodings` is replaced. Handles the shapes a real
 * `RTCRtpSender.getParameters()` can hand back, and the degenerate ones a
 * caller might pass by mistake:
 *  - `encodings` missing entirely (some webviews omit it until a track has
 *    been added) — treated as a single default encoding.
 *  - `encodings` present but empty (`[]`) — same treatment.
 *  - `encodings` already populated (one entry, or several for simulcast) —
 *    only index 0 gets `maxBitrate` set; every other field already on that
 *    entry (`rid`, `active`, `scaleResolutionDownBy`, …) and every other
 *    entry in the array is preserved as-is (copied, not dropped).
 *  - `existingParams` itself missing/nullish/not an object — treated as `{}`,
 *    so this never throws on a defensively-called empty invocation.
 *
 * @param {object|null|undefined} existingParams - The value from `sender.getParameters()` (or any params-shaped object).
 * @param {number} maxBitrate - The bitrate cap in bits per second to set on the first encoding.
 * @returns {object} A new params object equal to `existingParams` except `encodings[0].maxBitrate === maxBitrate` (and `encodings` guaranteed to have at least one entry).
 */
export function buildVideoSenderParameters(existingParams, maxBitrate) {
  const source =
    existingParams && typeof existingParams === "object" ? existingParams : {};
  const sourceEncodings =
    Array.isArray(source.encodings) && source.encodings.length > 0
      ? source.encodings
      : [{}];
  const encodings = sourceEncodings.map((enc, i) => {
    const copy = { ...(enc && typeof enc === "object" ? enc : {}) };
    if (i === 0) copy.maxBitrate = maxBitrate;
    return copy;
  });
  return { ...source, encodings };
}
