package mullu.comrade.ui

import androidx.compose.material.icons.materialIcon
import androidx.compose.material.icons.materialPath
import androidx.compose.ui.graphics.vector.ImageVector

/*
 * Material glyphs the bottom navigation needs but material-icons-core doesn't
 * ship (chat bubble, article, mic). Inlined as ImageVectors so the app never
 * depends on the multi-megabyte material-icons-extended artifact.
 */

/** Material "chat bubble" (filled). */
val ChatBubbleIcon: ImageVector = materialIcon(name = "Filled.ChatBubble") {
    materialPath {
        moveTo(20.0f, 2.0f)
        horizontalLineTo(4.0f)
        curveToRelative(-1.1f, 0.0f, -2.0f, 0.9f, -2.0f, 2.0f)
        verticalLineToRelative(18.0f)
        lineToRelative(4.0f, -4.0f)
        horizontalLineToRelative(14.0f)
        curveToRelative(1.1f, 0.0f, 2.0f, -0.9f, 2.0f, -2.0f)
        verticalLineTo(4.0f)
        curveToRelative(0.0f, -1.1f, -0.9f, -2.0f, -2.0f, -2.0f)
        close()
    }
}

/**
 * Material "queue music" (filled) — listening together.
 *
 * Inlined like every other icon here rather than taking
 * `material-icons-extended` for one glyph, which is the position this repo has
 * held since the first icon.
 *
 * A note over a list, not a bare note: the tab is a *shared* session, and the
 * lines read as something laid out for two people rather than a single track
 * playing. A speaker or a headphone glyph would both say "audio on this device",
 * which is the one thing this feature is not.
 */
val QueueMusicIcon: ImageVector = materialIcon(name = "Filled.QueueMusic") {
    materialPath {
        // The three list lines.
        moveTo(15.0f, 6.0f)
        horizontalLineTo(3.0f)
        verticalLineToRelative(2.0f)
        horizontalLineToRelative(12.0f)
        verticalLineTo(6.0f)
        close()
        moveTo(15.0f, 10.0f)
        horizontalLineTo(3.0f)
        verticalLineToRelative(2.0f)
        horizontalLineToRelative(12.0f)
        verticalLineToRelative(-2.0f)
        close()
        moveTo(3.0f, 16.0f)
        horizontalLineToRelative(8.0f)
        verticalLineToRelative(-2.0f)
        horizontalLineTo(3.0f)
        verticalLineToRelative(2.0f)
        close()
        // The note: stem, flag, and the head as a filled oval.
        moveTo(17.0f, 6.0f)
        verticalLineToRelative(8.18f)
        curveToRelative(-0.31f, -0.11f, -0.65f, -0.18f, -1.0f, -0.18f)
        curveToRelative(-1.66f, 0.0f, -3.0f, 1.34f, -3.0f, 3.0f)
        reflectiveCurveToRelative(1.34f, 3.0f, 3.0f, 3.0f)
        reflectiveCurveToRelative(3.0f, -1.34f, 3.0f, -3.0f)
        verticalLineTo(8.0f)
        horizontalLineToRelative(3.0f)
        verticalLineTo(6.0f)
        horizontalLineToRelative(-5.0f)
        close()
    }
}

/**
 * 🫂 — two people holding each other. The Together tab.
 *
 * It replaced [QueueMusicIcon] there on 2026-08-08, and the reason is the same
 * one that made the tab the only way into a session: a note over a list is a
 * *playlist*, and the tab is not about the music. Every other bottom-nav glyph
 * names the people it is for (a chat bubble, a heart) rather than the medium it
 * uses, and this one now does too.
 *
 * Hand-authored rather than taken from a font, like every icon in this file —
 * there is no Material Symbol for an embrace, so this is the emoji's silhouette
 * reduced to what survives 24dp: two heads leaning until they nearly touch, over
 * one body with a notch where the two of them meet. **The notch is the whole
 * glyph.** Without it this is Material's "group", which is two people standing
 * near each other; with it they are wrapped in each other's arms.
 *
 * Kept to a single mass rather than two overlapping figures with drawn-on arms:
 * `materialPath` fills non-zero, so an arm laid across the far figure's torso
 * unions with it and disappears, and the crossing limbs that survive that end up
 * as noise at tab size.
 */
val PeopleHugIcon: ImageVector = materialIcon(name = "Filled.PeopleHug") {
    // The left head.
    materialPath {
        moveTo(8.4f, 4.2f)
        arcToRelative(2.8f, 2.8f, 0.0f, true, true, 0.0f, 5.6f)
        arcToRelative(2.8f, 2.8f, 0.0f, true, true, 0.0f, -5.6f)
        close()
    }
    // The right head.
    materialPath {
        moveTo(15.6f, 4.2f)
        arcToRelative(2.8f, 2.8f, 0.0f, true, true, 0.0f, 5.6f)
        arcToRelative(2.8f, 2.8f, 0.0f, true, true, 0.0f, -5.6f)
        close()
    }
    // The embrace: shoulders out to both edges, and the notch between them.
    materialPath {
        moveTo(2.6f, 14.8f)
        curveTo(2.6f, 12.6f, 5.2f, 11.4f, 8.4f, 11.4f)
        curveTo(9.9f, 11.4f, 11.2f, 12.0f, 12.0f, 13.1f)
        curveTo(12.8f, 12.0f, 14.1f, 11.4f, 15.6f, 11.4f)
        curveTo(18.8f, 11.4f, 21.4f, 12.6f, 21.4f, 14.8f)
        verticalLineTo(20.0f)
        curveTo(21.4f, 20.6f, 20.9f, 21.1f, 20.3f, 21.1f)
        horizontalLineTo(3.7f)
        curveTo(3.1f, 21.1f, 2.6f, 20.6f, 2.6f, 20.0f)
        close()
    }
}

/** Material "article" (filled) — the public feed. */
val ArticleIcon: ImageVector = materialIcon(name = "Filled.Article") {
    materialPath {
        moveTo(19.0f, 3.0f)
        horizontalLineTo(5.0f)
        curveToRelative(-1.1f, 0.0f, -2.0f, 0.9f, -2.0f, 2.0f)
        verticalLineToRelative(14.0f)
        curveToRelative(0.0f, 1.1f, 0.9f, 2.0f, 2.0f, 2.0f)
        horizontalLineToRelative(14.0f)
        curveToRelative(1.1f, 0.0f, 2.0f, -0.9f, 2.0f, -2.0f)
        verticalLineTo(5.0f)
        curveToRelative(0.0f, -1.1f, -0.9f, -2.0f, -2.0f, -2.0f)
        close()
        moveTo(14.0f, 17.0f)
        horizontalLineTo(7.0f)
        verticalLineToRelative(-2.0f)
        horizontalLineToRelative(7.0f)
        verticalLineToRelative(2.0f)
        close()
        moveTo(17.0f, 13.0f)
        horizontalLineTo(7.0f)
        verticalLineToRelative(-2.0f)
        horizontalLineToRelative(10.0f)
        verticalLineToRelative(2.0f)
        close()
        moveTo(17.0f, 9.0f)
        horizontalLineTo(7.0f)
        verticalLineTo(7.0f)
        horizontalLineToRelative(10.0f)
        verticalLineToRelative(2.0f)
        close()
    }
}

/** Material "book" (filled) — the private journal. */
val BookIcon: ImageVector = materialIcon(name = "Filled.Book") {
    materialPath {
        moveTo(18.0f, 2.0f)
        horizontalLineTo(6.0f)
        curveToRelative(-1.1f, 0.0f, -2.0f, 0.9f, -2.0f, 2.0f)
        verticalLineToRelative(16.0f)
        curveToRelative(0.0f, 1.1f, 0.9f, 2.0f, 2.0f, 2.0f)
        horizontalLineToRelative(12.0f)
        curveToRelative(1.1f, 0.0f, 2.0f, -0.9f, 2.0f, -2.0f)
        verticalLineTo(4.0f)
        curveToRelative(0.0f, -1.1f, -0.9f, -2.0f, -2.0f, -2.0f)
        close()
        moveTo(6.0f, 4.0f)
        horizontalLineToRelative(5.0f)
        verticalLineToRelative(8.0f)
        lineToRelative(-2.5f, -1.5f)
        lineTo(6.0f, 12.0f)
        verticalLineTo(4.0f)
        close()
    }
}

/** Material "favorite" (filled) — Tara, the reflective companion. */
val HeartIcon: ImageVector = materialIcon(name = "Filled.Favorite") {
    materialPath {
        moveTo(12.0f, 21.35f)
        lineToRelative(-1.45f, -1.32f)
        curveTo(5.4f, 15.36f, 2.0f, 12.28f, 2.0f, 8.5f)
        curveTo(2.0f, 5.42f, 4.42f, 3.0f, 7.5f, 3.0f)
        curveToRelative(1.74f, 0.0f, 3.41f, 0.81f, 4.5f, 2.09f)
        curveTo(13.09f, 3.81f, 14.76f, 3.0f, 16.5f, 3.0f)
        curveTo(19.58f, 3.0f, 22.0f, 5.42f, 22.0f, 8.5f)
        curveToRelative(0.0f, 3.78f, -3.4f, 6.86f, -8.55f, 11.54f)
        lineTo(12.0f, 21.35f)
        close()
    }
}

/** Material "call" (filled) — place a voice call. */
val CallIcon: ImageVector = materialIcon(name = "Filled.Call") {
    materialPath {
        moveTo(6.62f, 10.79f)
        curveToRelative(1.44f, 2.83f, 3.76f, 5.14f, 6.59f, 6.59f)
        lineToRelative(2.2f, -2.2f)
        curveToRelative(0.27f, -0.27f, 0.67f, -0.36f, 1.02f, -0.24f)
        curveToRelative(1.12f, 0.37f, 2.33f, 0.57f, 3.57f, 0.57f)
        curveToRelative(0.55f, 0.0f, 1.0f, 0.45f, 1.0f, 1.0f)
        verticalLineTo(20.0f)
        curveToRelative(0.0f, 0.55f, -0.45f, 1.0f, -1.0f, 1.0f)
        curveTo(10.29f, 21.0f, 3.0f, 13.71f, 3.0f, 4.0f)
        curveToRelative(0.0f, -0.55f, 0.45f, -1.0f, 1.0f, -1.0f)
        horizontalLineToRelative(3.5f)
        curveToRelative(0.55f, 0.0f, 1.0f, 0.45f, 1.0f, 1.0f)
        curveToRelative(0.0f, 1.25f, 0.2f, 2.45f, 0.57f, 3.57f)
        curveToRelative(0.11f, 0.35f, 0.03f, 0.74f, -0.25f, 1.02f)
        lineToRelative(-2.2f, 2.2f)
        close()
    }
}

/** Material "call_end" (filled) — hang up / decline. */
val CallEndIcon: ImageVector = materialIcon(name = "Filled.CallEnd") {
    materialPath {
        moveTo(12.0f, 9.0f)
        curveToRelative(-1.6f, 0.0f, -3.15f, 0.25f, -4.6f, 0.72f)
        verticalLineToRelative(3.1f)
        curveToRelative(0.0f, 0.39f, -0.23f, 0.74f, -0.56f, 0.9f)
        curveToRelative(-0.98f, 0.49f, -1.87f, 1.12f, -2.66f, 1.85f)
        curveToRelative(-0.18f, 0.18f, -0.43f, 0.28f, -0.7f, 0.28f)
        curveToRelative(-0.28f, 0.0f, -0.53f, -0.11f, -0.71f, -0.29f)
        lineTo(0.29f, 13.08f)
        curveToRelative(-0.18f, -0.17f, -0.29f, -0.42f, -0.29f, -0.7f)
        curveToRelative(0.0f, -0.28f, 0.11f, -0.53f, 0.29f, -0.71f)
        curveTo(3.34f, 8.78f, 7.46f, 7.0f, 12.0f, 7.0f)
        reflectiveCurveToRelative(8.66f, 1.78f, 11.71f, 4.67f)
        curveToRelative(0.18f, 0.18f, 0.29f, 0.43f, 0.29f, 0.71f)
        curveToRelative(0.0f, 0.28f, -0.11f, 0.53f, -0.29f, 0.71f)
        lineToRelative(-2.48f, 2.48f)
        curveToRelative(-0.18f, 0.18f, -0.43f, 0.29f, -0.71f, 0.29f)
        curveToRelative(-0.27f, 0.0f, -0.52f, -0.11f, -0.7f, -0.28f)
        curveToRelative(-0.79f, -0.73f, -1.68f, -1.36f, -2.66f, -1.85f)
        curveToRelative(-0.33f, -0.16f, -0.56f, -0.5f, -0.56f, -0.9f)
        verticalLineToRelative(-3.1f)
        curveTo(15.15f, 9.25f, 13.6f, 9.0f, 12.0f, 9.0f)
        close()
    }
}

/** Material "videocam" (filled) — place a video call. */
val VideocamIcon: ImageVector = materialIcon(name = "Filled.Videocam") {
    materialPath {
        moveTo(17.0f, 10.5f)
        verticalLineTo(7.0f)
        curveToRelative(0.0f, -0.55f, -0.45f, -1.0f, -1.0f, -1.0f)
        horizontalLineTo(4.0f)
        curveToRelative(-0.55f, 0.0f, -1.0f, 0.45f, -1.0f, 1.0f)
        verticalLineToRelative(10.0f)
        curveToRelative(0.0f, 0.55f, 0.45f, 1.0f, 1.0f, 1.0f)
        horizontalLineToRelative(12.0f)
        curveToRelative(0.55f, 0.0f, 1.0f, -0.45f, 1.0f, -1.0f)
        verticalLineToRelative(-3.5f)
        lineToRelative(4.0f, 4.0f)
        verticalLineTo(6.5f)
        lineToRelative(-4.0f, 4.0f)
        close()
    }
}

/** Material "videocam_off" (filled) — camera currently disabled (mid-call toggle state). */
val VideocamOffIcon: ImageVector = materialIcon(name = "Filled.VideocamOff") {
    materialPath {
        moveTo(21.0f, 6.5f)
        lineToRelative(-4.0f, 4.0f)
        verticalLineTo(7.0f)
        curveToRelative(0.0f, -0.55f, -0.45f, -1.0f, -1.0f, -1.0f)
        horizontalLineTo(9.82f)
        lineTo(21.0f, 17.18f)
        verticalLineTo(6.5f)
        close()
        moveTo(3.27f, 2.0f)
        lineTo(2.0f, 3.27f)
        lineTo(4.73f, 6.0f)
        horizontalLineTo(4.0f)
        curveToRelative(-0.55f, 0.0f, -1.0f, 0.45f, -1.0f, 1.0f)
        verticalLineToRelative(10.0f)
        curveToRelative(0.0f, 0.55f, 0.45f, 1.0f, 1.0f, 1.0f)
        horizontalLineToRelative(12.0f)
        curveToRelative(0.21f, 0.0f, 0.39f, -0.08f, 0.54f, -0.18f)
        lineTo(19.73f, 21.0f)
        lineTo(21.0f, 19.73f)
        lineTo(3.27f, 2.0f)
        close()
    }
}

/** Material "autorenew" (filled) — the flip-front/back-camera control on the self-preview tile. */
val FlipCameraIcon: ImageVector = materialIcon(name = "Filled.FlipCamera") {
    materialPath {
        moveTo(12.0f, 6.0f)
        verticalLineToRelative(3.0f)
        lineToRelative(4.0f, -4.0f)
        lineToRelative(-4.0f, -4.0f)
        verticalLineToRelative(3.0f)
        curveToRelative(-4.42f, 0.0f, -8.0f, 3.58f, -8.0f, 8.0f)
        curveToRelative(0.0f, 1.57f, 0.46f, 3.03f, 1.24f, 4.26f)
        lineTo(6.7f, 14.8f)
        curveToRelative(-0.45f, -0.83f, -0.7f, -1.79f, -0.7f, -2.8f)
        curveToRelative(0.0f, -3.31f, 2.69f, -6.0f, 6.0f, -6.0f)
        close()
        moveTo(18.76f, 7.74f)
        lineTo(17.3f, 9.2f)
        curveToRelative(0.44f, 0.84f, 0.7f, 1.79f, 0.7f, 2.8f)
        curveToRelative(0.0f, 3.31f, -2.69f, 6.0f, -6.0f, 6.0f)
        verticalLineToRelative(-3.0f)
        lineToRelative(-4.0f, 4.0f)
        lineToRelative(4.0f, 4.0f)
        verticalLineToRelative(-3.0f)
        curveToRelative(4.42f, 0.0f, 8.0f, -3.58f, 8.0f, -8.0f)
        curveToRelative(0.0f, -1.57f, -0.46f, -3.03f, -1.24f, -4.26f)
        close()
    }
}

/** Material "volume_up" (filled) — speakerphone toggle. */
val SpeakerIcon: ImageVector = materialIcon(name = "Filled.VolumeUp") {
    materialPath {
        moveTo(3.0f, 9.0f)
        verticalLineToRelative(6.0f)
        horizontalLineToRelative(4.0f)
        lineToRelative(5.0f, 5.0f)
        verticalLineTo(4.0f)
        lineToRelative(-5.0f, 5.0f)
        horizontalLineTo(3.0f)
        close()
        moveTo(16.5f, 12.0f)
        curveToRelative(0.0f, -1.77f, -1.02f, -3.29f, -2.5f, -4.03f)
        verticalLineToRelative(8.05f)
        curveToRelative(1.48f, -0.73f, 2.5f, -2.25f, 2.5f, -4.02f)
        close()
        moveTo(14.0f, 3.23f)
        verticalLineToRelative(2.06f)
        curveToRelative(2.89f, 0.86f, 5.0f, 3.54f, 5.0f, 6.71f)
        reflectiveCurveToRelative(-2.11f, 5.85f, -5.0f, 6.71f)
        verticalLineToRelative(2.06f)
        curveToRelative(4.01f, -0.91f, 7.0f, -4.49f, 7.0f, -8.77f)
        reflectiveCurveToRelative(-2.99f, -7.86f, -7.0f, -8.77f)
        close()
    }
}

/** Material "mic" (filled) — the voice assistant. */
val MicIcon: ImageVector = materialIcon(name = "Filled.Mic") {
    materialPath {
        moveTo(12.0f, 14.0f)
        curveToRelative(1.66f, 0.0f, 3.0f, -1.34f, 3.0f, -3.0f)
        verticalLineTo(5.0f)
        curveToRelative(0.0f, -1.66f, -1.34f, -3.0f, -3.0f, -3.0f)
        reflectiveCurveTo(9.0f, 3.34f, 9.0f, 5.0f)
        verticalLineToRelative(6.0f)
        curveToRelative(0.0f, 1.66f, 1.34f, 3.0f, 3.0f, 3.0f)
        close()
        moveTo(17.0f, 11.0f)
        curveToRelative(0.0f, 2.76f, -2.24f, 5.0f, -5.0f, 5.0f)
        reflectiveCurveToRelative(-5.0f, -2.24f, -5.0f, -5.0f)
        horizontalLineTo(5.0f)
        curveToRelative(0.0f, 3.53f, 2.61f, 6.43f, 6.0f, 6.92f)
        verticalLineTo(21.0f)
        horizontalLineToRelative(2.0f)
        verticalLineToRelative(-3.08f)
        curveToRelative(3.39f, -0.49f, 6.0f, -3.39f, 6.0f, -6.92f)
        horizontalLineToRelative(-2.0f)
        close()
    }
}

/**
 * Material "star" (filled) — a chosen comrade (see [ComradesScreen]).
 *
 * Defined here rather than taken from `Icons.Filled`, like every other
 * feature glyph in this file, so the app never depends on which subset of
 * Material icons the core artifact happens to ship.
 */
val StarIcon: ImageVector = materialIcon(name = "Filled.Star") {
    materialPath {
        moveTo(12.0f, 17.27f)
        lineTo(18.18f, 21.0f)
        lineTo(16.54f, 13.97f)
        lineTo(22.0f, 9.24f)
        lineTo(14.81f, 8.63f)
        lineTo(12.0f, 2.0f)
        lineTo(9.19f, 8.63f)
        lineTo(2.0f, 9.24f)
        lineTo(7.46f, 13.97f)
        lineTo(5.82f, 21.0f)
        close()
    }
}

/** Material "star_outline" — not (yet) a comrade; the off state of [StarIcon]. */
val StarOutlineIcon: ImageVector = materialIcon(name = "Filled.StarOutline") {
    materialPath {
        moveTo(22.0f, 9.24f)
        lineTo(14.81f, 8.62f)
        lineTo(12.0f, 2.0f)
        lineTo(9.19f, 8.63f)
        lineTo(2.0f, 9.24f)
        lineTo(7.46f, 13.97f)
        lineTo(5.82f, 21.0f)
        lineTo(12.0f, 17.27f)
        lineTo(18.18f, 21.0f)
        lineTo(16.55f, 13.97f)
        close()
        // The inner cut-out, wound the other way so it renders as a hole.
        moveTo(12.0f, 15.4f)
        lineTo(8.24f, 17.67f)
        lineTo(9.24f, 13.39f)
        lineTo(5.92f, 10.51f)
        lineTo(10.3f, 10.13f)
        lineTo(12.0f, 6.1f)
        lineTo(13.71f, 10.14f)
        lineTo(18.09f, 10.52f)
        lineTo(14.77f, 13.4f)
        lineTo(15.77f, 17.68f)
        close()
    }
}

/** Material "content copy" (filled) — copying a key to the clipboard. */
val CopyIcon: ImageVector = materialIcon(name = "Filled.ContentCopy") {
    materialPath {
        moveTo(16.0f, 1.0f)
        horizontalLineTo(4.0f)
        curveToRelative(-1.1f, 0.0f, -2.0f, 0.9f, -2.0f, 2.0f)
        verticalLineToRelative(14.0f)
        horizontalLineToRelative(2.0f)
        verticalLineTo(3.0f)
        horizontalLineToRelative(12.0f)
        verticalLineTo(1.0f)
        close()
        moveTo(19.0f, 5.0f)
        horizontalLineTo(8.0f)
        curveToRelative(-1.1f, 0.0f, -2.0f, 0.9f, -2.0f, 2.0f)
        verticalLineToRelative(14.0f)
        curveToRelative(0.0f, 1.1f, 0.9f, 2.0f, 2.0f, 2.0f)
        horizontalLineToRelative(11.0f)
        curveToRelative(1.1f, 0.0f, 2.0f, -0.9f, 2.0f, -2.0f)
        verticalLineTo(7.0f)
        curveToRelative(0.0f, -1.1f, -0.9f, -2.0f, -2.0f, -2.0f)
        close()
        // The inner cut-out, wound the other way so it renders as a hole.
        moveTo(19.0f, 21.0f)
        horizontalLineTo(8.0f)
        verticalLineTo(7.0f)
        horizontalLineToRelative(11.0f)
        verticalLineToRelative(14.0f)
        close()
    }
}

/** Material "notifications" (filled) — the ⋮ menu's mute row. */
val NotificationsIcon: ImageVector = materialIcon(name = "Filled.Notifications") {
    materialPath {
        moveTo(12.0f, 22.0f)
        curveToRelative(1.1f, 0.0f, 2.0f, -0.9f, 2.0f, -2.0f)
        horizontalLineToRelative(-4.0f)
        curveToRelative(0.0f, 1.1f, 0.9f, 2.0f, 2.0f, 2.0f)
        close()
        moveTo(18.0f, 16.0f)
        verticalLineToRelative(-5.0f)
        curveToRelative(0.0f, -3.07f, -1.63f, -5.64f, -4.5f, -6.32f)
        verticalLineTo(4.0f)
        curveToRelative(0.0f, -0.83f, -0.67f, -1.5f, -1.5f, -1.5f)
        reflectiveCurveToRelative(-1.5f, 0.67f, -1.5f, 1.5f)
        verticalLineToRelative(0.68f)
        curveTo(7.64f, 5.36f, 6.0f, 7.92f, 6.0f, 11.0f)
        verticalLineToRelative(5.0f)
        lineToRelative(-2.0f, 2.0f)
        verticalLineToRelative(1.0f)
        horizontalLineToRelative(16.0f)
        verticalLineToRelative(-1.0f)
        lineToRelative(-2.0f, -2.0f)
        close()
    }
}

/**
 * Material "notifications off" (filled) — the unmute row. Drawn as the bell
 * with the diagonal bar Material uses for every "off" glyph, so it reads the
 * same way [VideocamOffIcon] does next to it.
 */
val NotificationsOffIcon: ImageVector = materialIcon(name = "Filled.NotificationsOff") {
    materialPath {
        moveTo(20.0f, 18.69f)
        lineTo(7.84f, 6.14f)
        curveTo(8.47f, 5.69f, 9.2f, 5.35f, 10.0f, 5.18f)
        verticalLineTo(4.0f)
        curveToRelative(0.0f, -0.83f, 0.67f, -1.5f, 1.5f, -1.5f)
        reflectiveCurveToRelative(1.5f, 0.67f, 1.5f, 1.5f)
        verticalLineToRelative(0.68f)
        curveToRelative(2.87f, 0.68f, 4.5f, 3.25f, 4.5f, 6.32f)
        verticalLineToRelative(5.0f)
        lineToRelative(2.0f, 2.0f)
        verticalLineToRelative(0.69f)
        close()
        moveTo(12.0f, 22.0f)
        curveToRelative(1.1f, 0.0f, 2.0f, -0.9f, 2.0f, -2.0f)
        horizontalLineToRelative(-4.0f)
        curveToRelative(0.0f, 1.1f, 0.9f, 2.0f, 2.0f, 2.0f)
        close()
        moveTo(6.0f, 11.0f)
        curveToRelative(0.0f, -0.5f, 0.05f, -0.98f, 0.14f, -1.44f)
        lineTo(4.41f, 7.83f)
        curveTo(4.15f, 8.82f, 4.0f, 9.88f, 4.0f, 11.0f)
        verticalLineToRelative(5.0f)
        lineToRelative(-2.0f, 2.0f)
        verticalLineToRelative(1.0f)
        horizontalLineToRelative(14.14f)
        lineTo(6.0f, 12.86f)
        close()
        // The diagonal bar every Material "off" glyph carries.
        moveTo(2.81f, 2.81f)
        lineTo(1.39f, 4.22f)
        lineTo(19.78f, 22.61f)
        lineTo(21.19f, 21.19f)
        close()
    }
}

/**
 * Material "screen_share" (filled) — share your screen during a call.
 *
 * A monitor with an upward arrow: the same glyph Telegram/Meet use, so the
 * control reads without a label. Inlined here for the same reason as every
 * other icon in this file — the app never depends on which subset of Material
 * icons the core artifact happens to ship.
 */
val ScreenShareIcon: ImageVector = materialIcon(name = "Filled.ScreenShare") {
    materialPath {
        // The monitor body.
        moveTo(20.0f, 18.0f)
        curveToRelative(1.1f, 0.0f, 2.0f, -0.9f, 2.0f, -2.0f)
        verticalLineTo(6.0f)
        curveToRelative(0.0f, -1.1f, -0.9f, -2.0f, -2.0f, -2.0f)
        horizontalLineTo(4.0f)
        curveToRelative(-1.1f, 0.0f, -2.0f, 0.9f, -2.0f, 2.0f)
        verticalLineToRelative(10.0f)
        curveToRelative(0.0f, 1.1f, 0.9f, 2.0f, 2.0f, 2.0f)
        horizontalLineTo(0.0f)
        verticalLineToRelative(2.0f)
        horizontalLineToRelative(24.0f)
        verticalLineToRelative(-2.0f)
        horizontalLineToRelative(-4.0f)
        close()
        // The arrow rising out of it.
        moveTo(13.0f, 12.4f)
        verticalLineToRelative(2.6f)
        horizontalLineToRelative(-2.0f)
        verticalLineToRelative(-2.6f)
        horizontalLineTo(8.0f)
        lineToRelative(4.0f, -4.0f)
        lineToRelative(4.0f, 4.0f)
        horizontalLineToRelative(-3.0f)
        close()
    }
}

/**
 * Material "hourglass_empty" (filled) — the Focus tab (attention practice).
 *
 * An hourglass rather than a stopwatch on purpose: a stopwatch measures
 * performance, an hourglass just marks that time is passing, which is all a
 * focus session asks anyone to do.
 */
val TimerIcon: ImageVector = materialIcon(name = "Filled.HourglassEmpty") {
    materialPath {
        moveTo(6.0f, 2.0f)
        verticalLineToRelative(6.0f)
        horizontalLineToRelative(0.01f)
        lineTo(6.0f, 8.01f)
        lineTo(10.0f, 12.0f)
        lineToRelative(-4.0f, 4.0f)
        lineToRelative(0.01f, 0.01f)
        horizontalLineTo(6.0f)
        verticalLineTo(22.0f)
        horizontalLineToRelative(12.0f)
        verticalLineToRelative(-5.99f)
        horizontalLineToRelative(-0.01f)
        lineTo(18.0f, 16.0f)
        lineToRelative(-4.0f, -4.0f)
        lineToRelative(4.0f, -3.99f)
        lineToRelative(-0.01f, -0.01f)
        horizontalLineTo(18.0f)
        verticalLineTo(2.0f)
        horizontalLineTo(6.0f)
        close()
        moveTo(16.0f, 16.5f)
        verticalLineTo(20.0f)
        horizontalLineTo(8.0f)
        verticalLineToRelative(-3.5f)
        lineToRelative(4.0f, -4.0f)
        lineToRelative(4.0f, 4.0f)
        close()
        moveTo(12.0f, 11.5f)
        lineToRelative(-4.0f, -4.0f)
        verticalLineTo(4.0f)
        horizontalLineToRelative(8.0f)
        verticalLineToRelative(3.5f)
        lineToRelative(-4.0f, 4.0f)
        close()
    }
}

/** Material "insert emoticon" (outlined-ish) — the composer's emoji button. */
val EmojiIcon: ImageVector = materialIcon(name = "Filled.InsertEmoticon") {
    materialPath {
        // Face outline as a ring, wound so the inside stays hollow.
        moveTo(12.0f, 2.0f)
        curveToRelative(-5.52f, 0.0f, -10.0f, 4.48f, -10.0f, 10.0f)
        reflectiveCurveToRelative(4.48f, 10.0f, 10.0f, 10.0f)
        reflectiveCurveToRelative(10.0f, -4.48f, 10.0f, -10.0f)
        reflectiveCurveTo(17.52f, 2.0f, 12.0f, 2.0f)
        close()
        moveTo(12.0f, 20.0f)
        curveToRelative(-4.42f, 0.0f, -8.0f, -3.58f, -8.0f, -8.0f)
        reflectiveCurveToRelative(3.58f, -8.0f, 8.0f, -8.0f)
        reflectiveCurveToRelative(8.0f, 3.58f, 8.0f, 8.0f)
        reflectiveCurveToRelative(-3.58f, 8.0f, -8.0f, 8.0f)
        close()
        // Eyes.
        moveTo(15.5f, 11.0f)
        curveToRelative(0.83f, 0.0f, 1.5f, -0.67f, 1.5f, -1.5f)
        reflectiveCurveTo(16.33f, 8.0f, 15.5f, 8.0f)
        reflectiveCurveTo(14.0f, 8.67f, 14.0f, 9.5f)
        reflectiveCurveToRelative(0.67f, 1.5f, 1.5f, 1.5f)
        close()
        moveTo(8.5f, 11.0f)
        curveToRelative(0.83f, 0.0f, 1.5f, -0.67f, 1.5f, -1.5f)
        reflectiveCurveTo(9.33f, 8.0f, 8.5f, 8.0f)
        reflectiveCurveTo(7.0f, 8.67f, 7.0f, 9.5f)
        reflectiveCurveToRelative(0.67f, 1.5f, 1.5f, 1.5f)
        close()
        // Smile.
        moveTo(12.0f, 17.5f)
        curveToRelative(2.33f, 0.0f, 4.31f, -1.46f, 5.11f, -3.5f)
        horizontalLineTo(6.89f)
        curveToRelative(0.8f, 2.04f, 2.78f, 3.5f, 5.11f, 3.5f)
        close()
    }
}

/** Material "attach file" (filled) — the composer's paper clip. */
val AttachFileIcon: ImageVector = materialIcon(name = "Filled.AttachFile") {
    materialPath {
        moveTo(16.5f, 6.0f)
        verticalLineToRelative(11.5f)
        curveToRelative(0.0f, 2.21f, -1.79f, 4.0f, -4.0f, 4.0f)
        reflectiveCurveToRelative(-4.0f, -1.79f, -4.0f, -4.0f)
        verticalLineTo(5.0f)
        curveToRelative(0.0f, -1.38f, 1.12f, -2.5f, 2.5f, -2.5f)
        reflectiveCurveToRelative(2.5f, 1.12f, 2.5f, 2.5f)
        verticalLineToRelative(10.5f)
        curveToRelative(0.0f, 0.55f, -0.45f, 1.0f, -1.0f, 1.0f)
        reflectiveCurveToRelative(-1.0f, -0.45f, -1.0f, -1.0f)
        verticalLineTo(6.0f)
        horizontalLineTo(10.0f)
        verticalLineToRelative(9.5f)
        curveToRelative(0.0f, 1.38f, 1.12f, 2.5f, 2.5f, 2.5f)
        reflectiveCurveToRelative(2.5f, -1.12f, 2.5f, -2.5f)
        verticalLineTo(5.0f)
        curveToRelative(0.0f, -2.21f, -1.79f, -4.0f, -4.0f, -4.0f)
        reflectiveCurveTo(7.0f, 2.79f, 7.0f, 5.0f)
        verticalLineToRelative(12.5f)
        curveToRelative(0.0f, 3.04f, 2.46f, 5.5f, 5.5f, 5.5f)
        reflectiveCurveToRelative(5.5f, -2.46f, 5.5f, -5.5f)
        verticalLineTo(6.0f)
        horizontalLineToRelative(-1.5f)
        close()
    }
}

/** Material "photo camera" (filled) — the composer's photo capture mode. */
val PhotoCameraIcon: ImageVector = materialIcon(name = "Filled.PhotoCamera") {
    materialPath {
        moveTo(9.0f, 2.0f)
        lineTo(7.17f, 4.0f)
        horizontalLineTo(4.0f)
        curveToRelative(-1.1f, 0.0f, -2.0f, 0.9f, -2.0f, 2.0f)
        verticalLineToRelative(12.0f)
        curveToRelative(0.0f, 1.1f, 0.9f, 2.0f, 2.0f, 2.0f)
        horizontalLineToRelative(16.0f)
        curveToRelative(1.1f, 0.0f, 2.0f, -0.9f, 2.0f, -2.0f)
        verticalLineTo(6.0f)
        curveToRelative(0.0f, -1.1f, -0.9f, -2.0f, -2.0f, -2.0f)
        horizontalLineToRelative(-3.17f)
        lineTo(15.0f, 2.0f)
        horizontalLineTo(9.0f)
        close()
        // Lens, wound the other way so it reads as a ring.
        moveTo(12.0f, 17.0f)
        curveToRelative(-2.76f, 0.0f, -5.0f, -2.24f, -5.0f, -5.0f)
        reflectiveCurveToRelative(2.24f, -5.0f, 5.0f, -5.0f)
        reflectiveCurveToRelative(5.0f, 2.24f, 5.0f, 5.0f)
        reflectiveCurveToRelative(-2.24f, 5.0f, -5.0f, 5.0f)
        close()
        moveTo(12.0f, 15.0f)
        curveToRelative(1.66f, 0.0f, 3.0f, -1.34f, 3.0f, -3.0f)
        reflectiveCurveToRelative(-1.34f, -3.0f, -3.0f, -3.0f)
        reflectiveCurveToRelative(-3.0f, 1.34f, -3.0f, 3.0f)
        reflectiveCurveToRelative(1.34f, 3.0f, 3.0f, 3.0f)
        close()
    }
}

/**
 * Material "cloud" (filled) — the relay route in the transport-precedence
 * control.
 */
val CloudIcon: ImageVector = materialIcon(name = "Filled.Cloud") {
    materialPath {
        moveTo(19.35f, 10.04f)
        curveTo(18.67f, 6.59f, 15.64f, 4.0f, 12.0f, 4.0f)
        curveTo(9.11f, 4.0f, 6.6f, 5.64f, 5.35f, 8.04f)
        curveTo(2.34f, 8.36f, 0.0f, 10.91f, 0.0f, 14.0f)
        curveToRelative(0.0f, 3.31f, 2.69f, 6.0f, 6.0f, 6.0f)
        horizontalLineToRelative(13.0f)
        curveToRelative(2.76f, 0.0f, 5.0f, -2.24f, 5.0f, -5.0f)
        curveToRelative(0.0f, -2.64f, -2.05f, -4.78f, -4.65f, -4.96f)
        close()
    }
}

/**
 * Material "wifi_tethering" (filled) — the local-network route.
 *
 * A dot with two radiating arcs. Unlike every other glyph here it is drawn in
 * one shape for all three mesh states (off / searching / reaching): the caller
 * carries that distinction in colour and a peer-count badge, because a glyph
 * that morphs between three silhouettes is harder to read at a glance than one
 * whose colour changes — and the "off" and "searching" variants live in
 * material-icons-extended, which this app deliberately does not depend on.
 */
val WifiTetheringIcon: ImageVector = materialIcon(name = "Filled.WifiTethering") {
    materialPath {
        // The device itself.
        moveTo(12.0f, 11.0f)
        curveToRelative(-1.1f, 0.0f, -2.0f, 0.9f, -2.0f, 2.0f)
        reflectiveCurveToRelative(0.9f, 2.0f, 2.0f, 2.0f)
        reflectiveCurveToRelative(2.0f, -0.9f, 2.0f, -2.0f)
        reflectiveCurveToRelative(-0.9f, -2.0f, -2.0f, -2.0f)
        close()
        // Inner arc.
        moveTo(18.0f, 13.0f)
        curveToRelative(0.0f, -3.31f, -2.69f, -6.0f, -6.0f, -6.0f)
        reflectiveCurveToRelative(-6.0f, 2.69f, -6.0f, 6.0f)
        curveToRelative(0.0f, 2.22f, 1.21f, 4.15f, 3.0f, 5.19f)
        lineToRelative(1.0f, -1.74f)
        curveToRelative(-1.19f, -0.7f, -2.0f, -1.97f, -2.0f, -3.45f)
        curveToRelative(0.0f, -2.21f, 1.79f, -4.0f, 4.0f, -4.0f)
        reflectiveCurveToRelative(4.0f, 1.79f, 4.0f, 4.0f)
        curveToRelative(0.0f, 1.48f, -0.81f, 2.75f, -2.0f, 3.45f)
        lineToRelative(1.0f, 1.74f)
        curveToRelative(1.79f, -1.04f, 3.0f, -2.97f, 3.0f, -5.19f)
        close()
        // Outer arc.
        moveTo(12.0f, 3.0f)
        curveTo(6.48f, 3.0f, 2.0f, 7.48f, 2.0f, 13.0f)
        curveToRelative(0.0f, 3.7f, 2.01f, 6.92f, 4.99f, 8.65f)
        lineToRelative(1.0f, -1.73f)
        curveTo(5.61f, 18.53f, 4.0f, 15.96f, 4.0f, 13.0f)
        curveToRelative(0.0f, -4.42f, 3.58f, -8.0f, 8.0f, -8.0f)
        reflectiveCurveToRelative(8.0f, 3.58f, 8.0f, 8.0f)
        curveToRelative(0.0f, 2.96f, -1.61f, 5.53f, -4.0f, 6.92f)
        lineToRelative(1.0f, 1.73f)
        curveToRelative(2.98f, -1.73f, 4.99f, -4.95f, 4.99f, -8.65f)
        curveTo(22.0f, 7.48f, 17.52f, 3.0f, 12.0f, 3.0f)
        close()
    }
}

/**
 * Material "graphic eq" (filled) — recording a **voice journal entry**.
 *
 * Deliberately not [MicIcon], which the journal composer already spends on
 * dictation. Two mics side by side, one of which silently turns speech into
 * typed text and one of which keeps the voice, is the confusion this whole
 * control exists to end; a level meter says "your voice, kept" in a way a
 * second microphone cannot.
 */
val VoiceEntryIcon: ImageVector = materialIcon(name = "Filled.GraphicEq") {
    materialPath {
        moveTo(7.0f, 18.0f)
        horizontalLineToRelative(2.0f)
        verticalLineTo(6.0f)
        horizontalLineTo(7.0f)
        verticalLineToRelative(12.0f)
        close()
        moveTo(11.0f, 22.0f)
        horizontalLineToRelative(2.0f)
        verticalLineTo(2.0f)
        horizontalLineToRelative(-2.0f)
        verticalLineToRelative(20.0f)
        close()
        moveTo(3.0f, 14.0f)
        horizontalLineToRelative(2.0f)
        verticalLineToRelative(-4.0f)
        horizontalLineTo(3.0f)
        verticalLineToRelative(4.0f)
        close()
        moveTo(15.0f, 18.0f)
        horizontalLineToRelative(2.0f)
        verticalLineTo(6.0f)
        horizontalLineToRelative(-2.0f)
        verticalLineToRelative(12.0f)
        close()
        moveTo(19.0f, 10.0f)
        verticalLineToRelative(4.0f)
        horizontalLineToRelative(2.0f)
        verticalLineToRelative(-4.0f)
        horizontalLineToRelative(-2.0f)
        close()
    }
}

/** Material "stop" (filled) — stop and send an in-progress voice note. */
val StopIcon: ImageVector = materialIcon(name = "Filled.Stop") {
    materialPath {
        moveTo(6.0f, 6.0f)
        horizontalLineToRelative(12.0f)
        verticalLineToRelative(12.0f)
        horizontalLineTo(6.0f)
        close()
    }
}

/**
 * Material "pause" (filled) — the other half of `Icons.Filled.PlayArrow`, which
 * `material-icons-core` ships without.
 */
val PauseIcon: ImageVector = materialIcon(name = "Filled.Pause") {
    materialPath {
        moveTo(6.0f, 19.0f)
        horizontalLineToRelative(4.0f)
        verticalLineTo(5.0f)
        horizontalLineTo(6.0f)
        verticalLineToRelative(14.0f)
        close()
        moveTo(14.0f, 5.0f)
        verticalLineToRelative(14.0f)
        horizontalLineToRelative(4.0f)
        verticalLineTo(5.0f)
        horizontalLineToRelative(-4.0f)
        close()
    }
}

/**
 * Material "skip next" (filled) — the next track in the Together queue.
 *
 * A bar and a triangle, drawn as two subpaths of one glyph, which is how the
 * real Material one is built too.
 */
val SkipNextIcon: ImageVector = materialIcon(name = "Filled.SkipNext") {
    materialPath {
        moveTo(6.0f, 18.0f)
        lineToRelative(8.5f, -6.0f)
        lineTo(6.0f, 6.0f)
        verticalLineToRelative(12.0f)
        close()
        moveTo(16.0f, 6.0f)
        verticalLineToRelative(12.0f)
        horizontalLineToRelative(2.0f)
        verticalLineTo(6.0f)
        horizontalLineToRelative(-2.0f)
        close()
    }
}

/** Material "skip previous" (filled) — [SkipNextIcon] mirrored. */
val SkipPreviousIcon: ImageVector = materialIcon(name = "Filled.SkipPrevious") {
    materialPath {
        moveTo(6.0f, 6.0f)
        horizontalLineToRelative(2.0f)
        verticalLineToRelative(12.0f)
        horizontalLineTo(6.0f)
        close()
        moveTo(9.5f, 12.0f)
        lineTo(18.0f, 18.0f)
        verticalLineTo(6.0f)
        lineToRelative(-8.5f, 6.0f)
        close()
    }
}

/**
 * Material "mic off" (filled) — the Together microphone in its default state.
 *
 * Drawn rather than expressed as [MicIcon] in a dimmer tint, because muted is
 * the state a session *opens* in: a control whose only cue for "your microphone
 * is off" is a shade of grey is one people check by turning it on.
 */
val MicOffIcon: ImageVector = materialIcon(name = "Filled.MicOff") {
    materialPath {
        moveTo(19.0f, 11.0f)
        horizontalLineToRelative(-1.7f)
        curveToRelative(0.0f, 0.74f, -0.16f, 1.43f, -0.43f, 2.05f)
        lineToRelative(1.23f, 1.23f)
        curveToRelative(0.56f, -0.98f, 0.9f, -2.09f, 0.9f, -3.28f)
        close()
        moveTo(14.98f, 11.17f)
        curveToRelative(0.0f, -0.06f, 0.02f, -0.11f, 0.02f, -0.17f)
        verticalLineTo(5.0f)
        curveToRelative(0.0f, -1.66f, -1.34f, -3.0f, -3.0f, -3.0f)
        reflectiveCurveToRelative(-3.0f, 1.34f, -3.0f, 3.0f)
        verticalLineToRelative(0.18f)
        lineToRelative(5.98f, 5.99f)
        close()
        moveTo(4.27f, 3.0f)
        lineTo(3.0f, 4.27f)
        lineToRelative(6.01f, 6.01f)
        verticalLineTo(11.0f)
        curveToRelative(0.0f, 1.66f, 1.33f, 3.0f, 2.99f, 3.0f)
        curveToRelative(0.22f, 0.0f, 0.44f, -0.03f, 0.65f, -0.08f)
        lineToRelative(1.66f, 1.66f)
        curveToRelative(-0.71f, 0.33f, -1.5f, 0.52f, -2.31f, 0.52f)
        curveToRelative(-2.76f, 0.0f, -5.3f, -2.1f, -5.3f, -5.1f)
        horizontalLineTo(5.0f)
        curveToRelative(0.0f, 3.41f, 2.72f, 6.23f, 6.0f, 6.72f)
        verticalLineTo(21.0f)
        horizontalLineToRelative(2.0f)
        verticalLineToRelative(-3.28f)
        curveToRelative(0.91f, -0.13f, 1.77f, -0.45f, 2.54f, -0.9f)
        lineTo(19.73f, 21.0f)
        lineTo(21.0f, 19.73f)
        lineTo(4.27f, 3.0f)
        close()
    }
}

/** Material "link" (filled) — playing something from a pasted URL. */
val LinkIcon: ImageVector = materialIcon(name = "Filled.Link") {
    materialPath {
        moveTo(17.0f, 7.0f)
        horizontalLineToRelative(-4.0f)
        verticalLineToRelative(2.0f)
        horizontalLineToRelative(4.0f)
        curveToRelative(1.65f, 0.0f, 3.0f, 1.35f, 3.0f, 3.0f)
        reflectiveCurveToRelative(-1.35f, 3.0f, -3.0f, 3.0f)
        horizontalLineToRelative(-4.0f)
        verticalLineToRelative(2.0f)
        horizontalLineToRelative(4.0f)
        curveToRelative(2.76f, 0.0f, 5.0f, -2.24f, 5.0f, -5.0f)
        reflectiveCurveToRelative(-2.24f, -5.0f, -5.0f, -5.0f)
        close()
        moveTo(11.0f, 15.0f)
        horizontalLineTo(7.0f)
        curveToRelative(-1.65f, 0.0f, -3.0f, -1.35f, -3.0f, -3.0f)
        reflectiveCurveToRelative(1.35f, -3.0f, 3.0f, -3.0f)
        horizontalLineToRelative(4.0f)
        verticalLineTo(7.0f)
        horizontalLineTo(7.0f)
        curveToRelative(-2.76f, 0.0f, -5.0f, 2.24f, -5.0f, 5.0f)
        reflectiveCurveToRelative(2.24f, 5.0f, 5.0f, 5.0f)
        horizontalLineToRelative(4.0f)
        verticalLineToRelative(-2.0f)
        close()
        moveTo(8.0f, 11.0f)
        horizontalLineToRelative(8.0f)
        verticalLineToRelative(2.0f)
        horizontalLineTo(8.0f)
        close()
    }
}

/**
 * Material "more_vert" (filled) — the ⋮ that opens the in-call options dock.
 *
 * Three dots rather than a word, because the bar gives each control about a
 * fifth of a phone's width and every alternative label ("Options", "More") is
 * wider than the glyph it would replace.
 */
val MoreVertIcon: ImageVector = materialIcon(name = "Filled.MoreVert") {
    materialPath {
        moveTo(12.0f, 8.0f)
        curveToRelative(1.1f, 0.0f, 2.0f, -0.9f, 2.0f, -2.0f)
        reflectiveCurveToRelative(-0.9f, -2.0f, -2.0f, -2.0f)
        reflectiveCurveToRelative(-2.0f, 0.9f, -2.0f, 2.0f)
        reflectiveCurveToRelative(0.9f, 2.0f, 2.0f, 2.0f)
        close()
        moveTo(12.0f, 10.0f)
        curveToRelative(-1.1f, 0.0f, -2.0f, 0.9f, -2.0f, 2.0f)
        reflectiveCurveToRelative(0.9f, 2.0f, 2.0f, 2.0f)
        reflectiveCurveToRelative(2.0f, -0.9f, 2.0f, -2.0f)
        reflectiveCurveToRelative(-0.9f, -2.0f, -2.0f, -2.0f)
        close()
        moveTo(12.0f, 16.0f)
        curveToRelative(-1.1f, 0.0f, -2.0f, 0.9f, -2.0f, 2.0f)
        reflectiveCurveToRelative(0.9f, 2.0f, 2.0f, 2.0f)
        reflectiveCurveToRelative(2.0f, -0.9f, 2.0f, -2.0f)
        reflectiveCurveToRelative(-0.9f, -2.0f, -2.0f, -2.0f)
        close()
    }
}

/**
 * Material "reply" (filled) — the swipe-to-reply hint that fades in behind a
 * dragged bubble, and the Reply row in the long-press sheet.
 *
 * Drawn here rather than pulled from `Icons.AutoMirrored.Filled.Reply`, which
 * lives in material-icons-extended: the whole reason this file exists is to keep
 * that multi-megabyte artifact out of the APK. It points left, which is correct
 * for an LTR layout and the direction the gesture drags *from*; an RTL mirror
 * would need `automirrored`'s machinery and this app has no RTL locale yet.
 */
val ReplyIcon: ImageVector = materialIcon(name = "Filled.Reply") {
    materialPath {
        moveTo(10.0f, 9.0f)
        verticalLineTo(5.0f)
        lineToRelative(-7.0f, 7.0f)
        lineToRelative(7.0f, 7.0f)
        verticalLineToRelative(-4.1f)
        curveToRelative(5.0f, 0.0f, 8.5f, 1.6f, 11.0f, 5.1f)
        curveToRelative(-1.0f, -5.0f, -4.0f, -10.0f, -11.0f, -11.0f)
        close()
    }
}

/**
 * Material "share" (filled) — the three-node graph, on a journal entry and
 * nowhere else so far.
 *
 * Inlined for the reason the rest of this file exists: `Icons.Filled.Share`
 * lives in material-icons-extended, and one glyph is not worth the artifact.
 */
val ShareIcon: ImageVector = materialIcon(name = "Filled.Share") {
    materialPath {
        moveTo(18.0f, 16.08f)
        curveToRelative(-0.76f, 0.0f, -1.44f, 0.3f, -1.96f, 0.77f)
        lineTo(8.91f, 12.7f)
        curveToRelative(0.05f, -0.23f, 0.09f, -0.46f, 0.09f, -0.7f)
        reflectiveCurveToRelative(-0.04f, -0.47f, -0.09f, -0.7f)
        lineToRelative(7.05f, -4.11f)
        curveToRelative(0.54f, 0.5f, 1.25f, 0.81f, 2.04f, 0.81f)
        curveToRelative(1.66f, 0.0f, 3.0f, -1.34f, 3.0f, -3.0f)
        reflectiveCurveToRelative(-1.34f, -3.0f, -3.0f, -3.0f)
        reflectiveCurveToRelative(-3.0f, 1.34f, -3.0f, 3.0f)
        curveToRelative(0.0f, 0.24f, 0.04f, 0.47f, 0.09f, 0.7f)
        lineTo(8.04f, 9.81f)
        curveTo(7.5f, 9.31f, 6.79f, 9.0f, 6.0f, 9.0f)
        curveToRelative(-1.66f, 0.0f, -3.0f, 1.34f, -3.0f, 3.0f)
        reflectiveCurveToRelative(1.34f, 3.0f, 3.0f, 3.0f)
        curveToRelative(0.79f, 0.0f, 1.5f, -0.31f, 2.04f, -0.81f)
        lineToRelative(7.12f, 4.16f)
        curveToRelative(-0.05f, 0.21f, -0.08f, 0.43f, -0.08f, 0.65f)
        curveToRelative(0.0f, 1.61f, 1.31f, 2.92f, 2.92f, 2.92f)
        reflectiveCurveToRelative(2.92f, -1.31f, 2.92f, -2.92f)
        reflectiveCurveToRelative(-1.31f, -2.92f, -2.92f, -2.92f)
        close()
    }
}

/**
 * Material "tag" (filled) — a topic.
 *
 * A `#`, because that is the sigil `/assign #deposit` uses and the one drawn on
 * every topic chip; a folder glyph would have said "somewhere else" when the
 * whole point of a topic is that the messages never leave the conversation.
 *
 * Inlined like every other icon here rather than taking
 * `material-icons-extended` for one glyph — the position this file has held
 * since the first icon.
 */
val TagIcon: ImageVector = materialIcon(name = "Filled.Tag") {
    materialPath {
        moveTo(20.0f, 10.0f)
        verticalLineTo(8.0f)
        horizontalLineToRelative(-4.0f)
        verticalLineTo(4.0f)
        horizontalLineToRelative(-2.0f)
        verticalLineToRelative(4.0f)
        horizontalLineToRelative(-4.0f)
        verticalLineTo(4.0f)
        horizontalLineTo(8.0f)
        verticalLineToRelative(4.0f)
        horizontalLineTo(4.0f)
        verticalLineToRelative(2.0f)
        horizontalLineToRelative(4.0f)
        verticalLineToRelative(4.0f)
        horizontalLineTo(4.0f)
        verticalLineToRelative(2.0f)
        horizontalLineToRelative(4.0f)
        verticalLineToRelative(4.0f)
        horizontalLineToRelative(2.0f)
        verticalLineToRelative(-4.0f)
        horizontalLineToRelative(4.0f)
        verticalLineToRelative(4.0f)
        horizontalLineToRelative(2.0f)
        verticalLineToRelative(-4.0f)
        horizontalLineToRelative(4.0f)
        verticalLineToRelative(-2.0f)
        horizontalLineToRelative(-4.0f)
        verticalLineToRelative(-4.0f)
        horizontalLineToRelative(4.0f)
        close()
        moveTo(14.0f, 14.0f)
        horizontalLineToRelative(-4.0f)
        verticalLineToRelative(-4.0f)
        horizontalLineToRelative(4.0f)
        verticalLineToRelative(4.0f)
        close()
    }
}

/**
 * A motorcycle, for Ride mode — drawn as an adventure bike rather than the
 * Material "two-wheeler" scooter glyph, because the two read as different
 * vehicles and this feature is about the one you sit on for six hours.
 *
 * The silhouette is a Himalayan 411's: two equal, spoked-looking wheels (a
 * scooter's front wheel is visibly smaller), a high flat handlebar, and the
 * tall screen-and-rack over the front wheel that is the model's one
 * unmistakable line. Inlined like every other icon here rather than taking
 * `material-icons-extended` for one glyph — the position this file has held
 * since the first icon.
 */
val MotorcycleIcon: ImageVector = materialIcon(name = "Filled.Motorcycle") {
    materialPath {
        // Rear wheel.
        moveTo(5.5f, 14.0f)
        curveTo(3.6f, 14.0f, 2.0f, 15.6f, 2.0f, 17.5f)
        curveTo(2.0f, 19.4f, 3.6f, 21.0f, 5.5f, 21.0f)
        curveTo(7.4f, 21.0f, 9.0f, 19.4f, 9.0f, 17.5f)
        curveTo(9.0f, 15.6f, 7.4f, 14.0f, 5.5f, 14.0f)
        close()
        moveTo(5.5f, 19.5f)
        curveTo(4.4f, 19.5f, 3.5f, 18.6f, 3.5f, 17.5f)
        curveTo(3.5f, 16.4f, 4.4f, 15.5f, 5.5f, 15.5f)
        curveTo(6.6f, 15.5f, 7.5f, 16.4f, 7.5f, 17.5f)
        curveTo(7.5f, 18.6f, 6.6f, 19.5f, 5.5f, 19.5f)
        close()
        // Front wheel — the same size as the rear, which is the adventure-bike
        // tell and the thing that stops this reading as a scooter.
        moveTo(18.5f, 14.0f)
        curveTo(16.6f, 14.0f, 15.0f, 15.6f, 15.0f, 17.5f)
        curveTo(15.0f, 19.4f, 16.6f, 21.0f, 18.5f, 21.0f)
        curveTo(20.4f, 21.0f, 22.0f, 19.4f, 22.0f, 17.5f)
        curveTo(22.0f, 15.6f, 20.4f, 14.0f, 18.5f, 14.0f)
        close()
        moveTo(18.5f, 19.5f)
        curveTo(17.4f, 19.5f, 16.5f, 18.6f, 16.5f, 17.5f)
        curveTo(16.5f, 16.4f, 17.4f, 15.5f, 18.5f, 15.5f)
        curveTo(19.6f, 15.5f, 20.5f, 16.4f, 20.5f, 17.5f)
        curveTo(20.5f, 18.6f, 19.6f, 19.5f, 18.5f, 19.5f)
        close()
        // Tank, seat and the frame line joining the two hubs.
        moveTo(15.8f, 10.0f)
        lineTo(13.2f, 10.0f)
        lineTo(11.6f, 12.2f)
        lineTo(8.2f, 12.2f)
        lineTo(7.0f, 10.6f)
        lineTo(5.0f, 10.6f)
        lineTo(5.0f, 12.1f)
        lineTo(6.2f, 12.1f)
        lineTo(7.4f, 13.7f)
        lineTo(5.5f, 13.7f)
        lineTo(5.5f, 15.2f)
        lineTo(12.4f, 15.2f)
        lineTo(14.6f, 12.1f)
        lineTo(17.4f, 15.2f)
        lineTo(18.9f, 15.2f)
        close()
        // High flat bar and the tall screen over the front wheel — the
        // Himalayan's one unmistakable line at this size.
        moveTo(16.4f, 4.4f)
        lineTo(19.6f, 4.4f)
        lineTo(19.6f, 5.8f)
        lineTo(18.6f, 5.8f)
        lineTo(18.0f, 9.2f)
        lineTo(16.5f, 9.2f)
        lineTo(17.0f, 5.8f)
        lineTo(16.4f, 5.8f)
        close()
    }
}

/**
 * Material "explore" (filled) — a compass rose, for the Travel tab.
 *
 * A compass rather than a map pin or a fork: the tab is about *looking around
 * where you already are*, not about navigating to somewhere or about food
 * alone. A pin says "this is a location feature", which is what the permission
 * dialog is for; a fork would misname the half of the tab that is museums and
 * beaches.
 */
val ExploreIcon: ImageVector = materialIcon(name = "Filled.Explore") {
    materialPath {
        // The dial.
        moveTo(12.0f, 10.9f)
        curveTo(11.39f, 10.9f, 10.9f, 11.39f, 10.9f, 12.0f)
        curveTo(10.9f, 12.61f, 11.39f, 13.1f, 12.0f, 13.1f)
        curveTo(12.61f, 13.1f, 13.1f, 12.61f, 13.1f, 12.0f)
        curveTo(13.1f, 11.39f, 12.61f, 10.9f, 12.0f, 10.9f)
        close()
        moveTo(12.0f, 2.0f)
        curveTo(6.48f, 2.0f, 2.0f, 6.48f, 2.0f, 12.0f)
        curveTo(2.0f, 17.52f, 6.48f, 22.0f, 12.0f, 22.0f)
        curveTo(17.52f, 22.0f, 22.0f, 17.52f, 22.0f, 12.0f)
        curveTo(22.0f, 6.48f, 17.52f, 2.0f, 12.0f, 2.0f)
        close()
        moveTo(14.19f, 14.19f)
        lineTo(6.0f, 18.0f)
        lineTo(9.81f, 9.81f)
        lineTo(18.0f, 6.0f)
        lineTo(14.19f, 14.19f)
        close()
    }
}

/**
 * Material "computer" (filled) — a streaming server you own.
 *
 * Inlined like every other icon here rather than taking
 * `material-icons-extended` for one glyph. A monitor-with-base reads as "a
 * machine somewhere else that answers you", which is exactly what a
 * Subsonic/Navidrome card is offering; a cloud would claim somebody else's
 * infrastructure, and this feature's whole point is that there isn't any.
 */
val ComputerIcon: ImageVector = materialIcon(name = "Filled.Computer") {
    materialPath {
        moveTo(20.0f, 18.0f)
        curveToRelative(1.1f, 0.0f, 2.0f, -0.9f, 2.0f, -2.0f)
        verticalLineTo(6.0f)
        curveToRelative(0.0f, -1.1f, -0.9f, -2.0f, -2.0f, -2.0f)
        horizontalLineTo(4.0f)
        curveToRelative(-1.1f, 0.0f, -2.0f, 0.9f, -2.0f, 2.0f)
        verticalLineToRelative(10.0f)
        curveToRelative(0.0f, 1.1f, 0.9f, 2.0f, 2.0f, 2.0f)
        horizontalLineTo(0.0f)
        verticalLineToRelative(2.0f)
        horizontalLineToRelative(24.0f)
        verticalLineToRelative(-2.0f)
        horizontalLineToRelative(-4.0f)
        close()
        moveTo(4.0f, 6.0f)
        horizontalLineToRelative(16.0f)
        verticalLineToRelative(10.0f)
        horizontalLineTo(4.0f)
        verticalLineTo(6.0f)
        close()
    }
}

/** Material "shuffle" (filled) — the player extras row. */
val ShuffleIcon: ImageVector = materialIcon(name = "Filled.Shuffle") {
    materialPath {
        moveTo(10.59f, 9.17f)
        lineTo(5.41f, 4.0f)
        lineTo(4.0f, 5.41f)
        lineToRelative(5.17f, 5.17f)
        lineToRelative(1.42f, -1.41f)
        close()
        moveTo(14.5f, 4.0f)
        lineToRelative(2.04f, 2.04f)
        lineTo(4.0f, 18.83f)
        lineTo(5.41f, 20.0f)
        lineTo(17.96f, 7.46f)
        lineTo(20.0f, 9.5f)
        verticalLineTo(4.0f)
        horizontalLineToRelative(-5.5f)
        close()
        moveToRelative(0.33f, 9.41f)
        lineToRelative(-1.41f, 1.41f)
        lineToRelative(3.13f, 3.13f)
        lineTo(14.5f, 20.0f)
        horizontalLineTo(20.0f)
        verticalLineToRelative(-5.5f)
        lineToRelative(-2.04f, 2.04f)
        lineToRelative(-3.13f, -3.13f)
        close()
    }
}

/** Material "repeat" (filled). */
val RepeatIcon: ImageVector = materialIcon(name = "Filled.Repeat") {
    materialPath {
        moveTo(7.0f, 7.0f)
        horizontalLineToRelative(10.0f)
        verticalLineToRelative(3.0f)
        lineToRelative(4.0f, -4.0f)
        lineToRelative(-4.0f, -4.0f)
        verticalLineToRelative(3.0f)
        horizontalLineTo(5.0f)
        verticalLineToRelative(6.0f)
        horizontalLineToRelative(2.0f)
        verticalLineTo(7.0f)
        close()
        moveTo(17.0f, 17.0f)
        horizontalLineTo(7.0f)
        verticalLineToRelative(-3.0f)
        lineToRelative(-4.0f, 4.0f)
        lineToRelative(4.0f, 4.0f)
        verticalLineToRelative(-3.0f)
        horizontalLineToRelative(12.0f)
        verticalLineToRelative(-6.0f)
        horizontalLineToRelative(-2.0f)
        verticalLineToRelative(4.0f)
        close()
    }
}

/** Material "repeat_one" (filled) — repeat with the one in the middle. */
val RepeatOneIcon: ImageVector = materialIcon(name = "Filled.RepeatOne") {
    materialPath {
        moveTo(7.0f, 7.0f)
        horizontalLineToRelative(10.0f)
        verticalLineToRelative(3.0f)
        lineToRelative(4.0f, -4.0f)
        lineToRelative(-4.0f, -4.0f)
        verticalLineToRelative(3.0f)
        horizontalLineTo(5.0f)
        verticalLineToRelative(6.0f)
        horizontalLineToRelative(2.0f)
        verticalLineTo(7.0f)
        close()
        moveTo(17.0f, 17.0f)
        horizontalLineTo(7.0f)
        verticalLineToRelative(-3.0f)
        lineToRelative(-4.0f, 4.0f)
        lineToRelative(4.0f, 4.0f)
        verticalLineToRelative(-3.0f)
        horizontalLineToRelative(12.0f)
        verticalLineToRelative(-6.0f)
        horizontalLineToRelative(-2.0f)
        verticalLineToRelative(4.0f)
        close()
        moveTo(13.0f, 15.0f)
        verticalLineTo(9.0f)
        horizontalLineToRelative(-1.0f)
        lineToRelative(-2.0f, 1.0f)
        verticalLineToRelative(1.0f)
        horizontalLineToRelative(1.5f)
        verticalLineToRelative(4.0f)
        horizontalLineTo(13.0f)
        close()
    }
}

/** Material "speed" (filled) — playback rate. */
val SpeedIcon: ImageVector = materialIcon(name = "Filled.Speed") {
    materialPath {
        moveTo(20.38f, 8.57f)
        lineToRelative(-1.23f, 1.85f)
        curveToRelative(0.0f, 0.0f, -0.0f, 0.0f, -0.0f, 0.0f)
        arcToRelative(8.0f, 8.0f, 0.0f, false, true, -0.22f, 7.58f)
        horizontalLineTo(5.07f)
        arcToRelative(8.0f, 8.0f, 0.0f, false, true, 10.51f, -11.15f)
        lineToRelative(1.85f, -1.23f)
        arcTo(10.0f, 10.0f, 0.0f, false, false, 3.35f, 19.0f)
        curveToRelative(-0.36f, 0.62f, -0.31f, 1.39f, 0.12f, 1.98f)
        horizontalLineToRelative(16.95f)
        curveToRelative(0.75f, 0.0f, 1.44f, -0.41f, 1.79f, -1.08f)
        arcTo(10.0f, 10.0f, 0.0f, false, false, 20.38f, 8.57f)
        close()
        moveTo(10.59f, 15.41f)
        curveToRelative(0.78f, 0.78f, 2.05f, 0.78f, 2.83f, 0.0f)
        lineToRelative(5.66f, -8.49f)
        lineToRelative(-8.49f, 5.66f)
        curveToRelative(-0.78f, 0.78f, -0.78f, 2.05f, 0.0f, 2.83f)
        close()
    }
}

/** Material "bedtime" (filled) — the sleep timer. */
val BedtimeIcon: ImageVector = materialIcon(name = "Filled.Bedtime") {
    materialPath {
        moveTo(12.34f, 2.02f)
        curveTo(6.59f, 1.82f, 2.0f, 6.42f, 2.0f, 12.0f)
        curveToRelative(0.0f, 5.52f, 4.48f, 10.0f, 10.0f, 10.0f)
        curveToRelative(3.71f, 0.0f, 6.93f, -2.02f, 8.66f, -5.02f)
        curveToRelative(-7.51f, -0.25f, -12.09f, -8.43f, -8.32f, -14.96f)
        close()
    }
}

/** Material "tune" (filled) — the equalizer. */
val TuneIcon: ImageVector = materialIcon(name = "Filled.Tune") {
    materialPath {
        moveTo(3.0f, 17.0f)
        verticalLineToRelative(2.0f)
        horizontalLineToRelative(6.0f)
        verticalLineToRelative(-2.0f)
        horizontalLineTo(3.0f)
        close()
        moveTo(3.0f, 5.0f)
        verticalLineToRelative(2.0f)
        horizontalLineToRelative(10.0f)
        verticalLineTo(5.0f)
        horizontalLineTo(3.0f)
        close()
        moveTo(13.0f, 21.0f)
        verticalLineToRelative(-2.0f)
        horizontalLineToRelative(8.0f)
        verticalLineToRelative(-2.0f)
        horizontalLineToRelative(-8.0f)
        verticalLineToRelative(-2.0f)
        horizontalLineToRelative(-2.0f)
        verticalLineToRelative(6.0f)
        horizontalLineToRelative(2.0f)
        close()
        moveTo(7.0f, 9.0f)
        verticalLineToRelative(2.0f)
        horizontalLineTo(3.0f)
        verticalLineToRelative(2.0f)
        horizontalLineToRelative(4.0f)
        verticalLineToRelative(2.0f)
        horizontalLineToRelative(2.0f)
        verticalLineTo(9.0f)
        horizontalLineTo(7.0f)
        close()
        moveTo(21.0f, 13.0f)
        verticalLineToRelative(-2.0f)
        horizontalLineTo(11.0f)
        verticalLineToRelative(2.0f)
        horizontalLineToRelative(10.0f)
        close()
        moveTo(15.0f, 9.0f)
        horizontalLineToRelative(2.0f)
        verticalLineTo(7.0f)
        horizontalLineToRelative(4.0f)
        verticalLineTo(5.0f)
        horizontalLineToRelative(-4.0f)
        verticalLineTo(3.0f)
        horizontalLineToRelative(-2.0f)
        verticalLineToRelative(6.0f)
        close()
    }
}

/** Material "lyrics"-shaped note-plus-lines — synced lyrics sheet entry. */
val LyricsIcon: ImageVector = materialIcon(name = "Filled.Lyrics") {
    materialPath {
        moveTo(14.0f, 9.0f)
        horizontalLineTo(3.0f)
        verticalLineToRelative(2.0f)
        horizontalLineToRelative(11.0f)
        verticalLineTo(9.0f)
        close()
        moveTo(14.0f, 5.0f)
        horizontalLineTo(3.0f)
        verticalLineToRelative(2.0f)
        horizontalLineToRelative(11.0f)
        verticalLineTo(5.0f)
        close()
        moveTo(18.0f, 13.0f)
        verticalLineToRelative(6.0f)
        lineToRelative(-5.0f, -3.0f)
        lineTo(18.0f, 13.0f)
        close()
        moveTo(3.0f, 15.0f)
        horizontalLineToRelative(7.0f)
        verticalLineToRelative(-2.0f)
        horizontalLineTo(3.0f)
        verticalLineTo(15.0f)
        close()
    }
}
