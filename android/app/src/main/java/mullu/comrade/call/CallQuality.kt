package mullu.comrade.call

/**
 * A coarse, heuristic read on the current call's live media quality — see
 * [CallManager.connectionQuality], refreshed every couple of seconds from
 * [org.webrtc.PeerConnection.getStats] (round-trip-time/jitter). [UNKNOWN]
 * covers both "no reading yet" and "stats present but unparseable" — in both
 * cases the UI's only correct move is to show nothing.
 */
enum class CallQuality { GOOD, MEDIUM, POOR, UNKNOWN }
