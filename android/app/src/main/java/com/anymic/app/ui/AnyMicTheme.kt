package com.anymic.app.ui

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color

// ── Dark scheme (default) ───────────────────────────────────────────────────
private val DarkColors = darkColorScheme(
    primary             = Color(0xFF82CFFF),
    onPrimary           = Color(0xFF003549),
    primaryContainer    = Color(0xFF004C68),
    onPrimaryContainer  = Color(0xFFBFE9FF),
    secondary           = Color(0xFFB3CAD9),
    onSecondary         = Color(0xFF1D333F),
    background          = Color(0xFF0D1B22),
    surface             = Color(0xFF152029),
    onSurface           = Color(0xFFDDE3E9),
    onBackground        = Color(0xFFDDE3E9),
    error               = Color(0xFFFFB4AB),
    onError             = Color(0xFF690005),
    surfaceVariant      = Color(0xFF1F2E38),
    outline             = Color(0xFF3A4D58),
)

// ── Light scheme ────────────────────────────────────────────────────────────
private val LightColors = lightColorScheme(
    primary             = Color(0xFF0066D6),
    onPrimary           = Color(0xFFFFFFFF),
    primaryContainer    = Color(0xFFD2E5FF),
    onPrimaryContainer  = Color(0xFF001F40),
    secondary           = Color(0xFF4A6072),
    onSecondary         = Color(0xFFFFFFFF),
    background          = Color(0xFFF3F5FA),
    surface             = Color(0xFFFFFFFF),
    onSurface           = Color(0xFF1F2030),
    onBackground        = Color(0xFF1F2030),
    error               = Color(0xFFB02020),
    onError             = Color(0xFFFFFFFF),
    surfaceVariant      = Color(0xFFE8ECF2),
    outline             = Color(0xFFB0B6C0),
)

@Composable
fun AnyMicTheme(
    darkTheme: Boolean = isSystemInDarkTheme(),
    content: @Composable () -> Unit,
) {
    MaterialTheme(
        colorScheme = if (darkTheme) DarkColors else LightColors,
        content     = content,
    )
}
