package mullu.comrade.ui

/**
 * Pure display-name helpers, kept free of Compose/Android imports so plain
 * JVM unit tests (`DisplayNameTest`) can pin the precedence rules.
 */

/** Short display form of an npub: `npub1abcd…wxyz`. */
fun shortNpub(npub: String): String =
    if (npub.length > 16) "${npub.take(10)}…${npub.takeLast(4)}" else npub

/**
 * Display title for a peer, in trust order:
 *  1. the alias *you* chose for the contact (yours, can't be spoofed),
 *  2. the `@handle` *they* published — a self-declared claim, so screens
 *     showing it keep the key visible alongside,
 *  3. the shortened key.
 */
fun peerTitle(peer: String, alias: String?, username: String?): String {
    alias?.takeIf { it.isNotBlank() }?.let { return it }
    username?.takeIf { it.isNotBlank() }?.let { return "@${it.removePrefix("@")}" }
    return shortNpub(peer)
}

/**
 * Deterministic palette index for a peer avatar: the same key always gets
 * the same colour, on every device — identity-stable like the key itself.
 */
fun avatarColorIndex(seed: String, paletteSize: Int): Int {
    if (paletteSize <= 0) return 0
    var hash = 0
    for (c in seed) hash = (hash * 31 + c.code) and 0x7FFFFFFF
    return hash % paletteSize
}

/** Rough relative timestamp for list rows. */
fun relativeTime(epochSecs: Long): String {
    val d = System.currentTimeMillis() / 1000 - epochSecs
    return when {
        d < 60 -> "now"
        d < 3600 -> "${d / 60}m"
        d < 86_400 -> "${d / 3600}h"
        else -> "${d / 86_400}d"
    }
}
