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
