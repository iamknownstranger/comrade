package mullu.comrade.ui.theme

import android.app.Activity
import android.os.Build
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.*
import androidx.compose.runtime.Composable
import androidx.compose.runtime.SideEffect
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.core.view.WindowCompat

/*
 * Fallback brand palette, used when Material You dynamic color is unavailable
 * (below Android 12). Mirrors the desktop shell's visual system: indigo
 * accent, emerald "good", amber "warn", deep navy surfaces
 * (desktop/ui/styles.css `:root`).
 */

private val DarkColorScheme = darkColorScheme(
    primary = Color(0xFF818CF8),
    onPrimary = Color(0xFF1E1B4B),
    primaryContainer = Color(0xFF3730A3),
    onPrimaryContainer = Color(0xFFE0E7FF),
    secondary = Color(0xFF34D399),
    onSecondary = Color(0xFF022C22),
    tertiary = Color(0xFFFBBF24),
    onTertiary = Color(0xFF2A1B06),
    background = Color(0xFF0A0E1A),
    onBackground = Color(0xFFE6EBF5),
    surface = Color(0xFF0F1525),
    onSurface = Color(0xFFE6EBF5),
    surfaceVariant = Color(0xFF1A2438),
    onSurfaceVariant = Color(0xFF9AA7C2),
    outline = Color(0xFF6B7894),
)

private val LightColorScheme = lightColorScheme(
    primary = Color(0xFF4F46E5),
    onPrimary = Color(0xFFFFFFFF),
    primaryContainer = Color(0xFFE0E7FF),
    onPrimaryContainer = Color(0xFF1E1B4B),
    secondary = Color(0xFF059669),
    onSecondary = Color(0xFFFFFFFF),
    tertiary = Color(0xFFB45309),
    onTertiary = Color(0xFFFFFFFF),
)

@Composable
fun ComradeTheme(
    darkTheme: Boolean = isSystemInDarkTheme(),
    dynamicColor: Boolean = true,
    content: @Composable () -> Unit,
) {
    val colorScheme = when {
        dynamicColor && Build.VERSION.SDK_INT >= Build.VERSION_CODES.S -> {
            val context = LocalContext.current
            if (darkTheme) dynamicDarkColorScheme(context)
            else dynamicLightColorScheme(context)
        }
        darkTheme -> DarkColorScheme
        else -> LightColorScheme
    }

    val view = LocalView.current
    if (!view.isInEditMode) {
        SideEffect {
            val window = (view.context as Activity).window
            // Blend the status bar with the top app bar (both sit on surface).
            window.statusBarColor = colorScheme.surface.toArgb()
            WindowCompat.getInsetsController(window, view).isAppearanceLightStatusBars = !darkTheme
        }
    }

    MaterialTheme(
        colorScheme = colorScheme,
        content = content,
    )
}
