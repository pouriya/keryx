// The screen when the app opens and nothing is set up: the mark, the name, one sentence, and the
// button. Nothing is submitted, so there is no error. English is selected until FA is chosen.

package io.github.pouriya.keryx

import android.app.Activity
import android.content.Context
import android.content.ContextWrapper
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.exclude
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawing
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.view.ViewCompat
import androidx.core.view.WindowCompat

private val tile = Color(0xFF2A436B)
private val accent = Color(0xFF4576C4)
private val onAccent = Color(0xFFFFFFFF)

private data class Page(
    val page: Color,
    val text: Color,
    val muted: Color,
)

@Composable
fun Welcome(
    language: String,
    onLanguage: (String) -> Unit,
    onConnect: () -> Unit,
) {
    val dark = isSystemInDarkTheme()
    val ink =
        if (dark) {
            Page(Color(0xFF141B24), Color(0xFFF4F6F8), Color(0xFF93A0B0))
        } else {
            Page(Color(0xFFFFFFFF), Color(0xFF121212), Color(0xFF5E6A78))
        }
    Bars(ink.page, dark)
    Column(Modifier.fillMaxSize().background(ink.page)) {
        Row(
            Modifier
                .fillMaxWidth()
                .windowInsetsPadding(WindowInsets.safeDrawing.exclude(WindowInsets.navigationBars))
                .padding(start = 24.dp, end = 24.dp, top = 8.dp),
            horizontalArrangement = Arrangement.End,
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Language("en", "EN", language == "en", ink.text, onLanguage)
            Language("fa", "FA", language == "fa", ink.text, onLanguage)
        }
        Column(
            Modifier.weight(1f).fillMaxWidth().padding(horizontal = 24.dp),
            verticalArrangement = Arrangement.Center,
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Box(
                Modifier.size(88.dp).clip(RoundedCornerShape(22.dp)).background(tile),
                contentAlignment = Alignment.Center,
            ) {
                Image(painterResource(R.drawable.logo_foreground), null, Modifier.fillMaxSize())
            }
            Text(
                stringResource(R.string.welcome_name),
                Modifier.padding(top = 16.dp),
                color = ink.text,
                fontSize = 34.sp,
                fontWeight = FontWeight.SemiBold,
                letterSpacing = if (language == "fa") 0.sp else (-0.5).sp,
            )
            Text(
                stringResource(R.string.welcome_line),
                Modifier.padding(top = 18.dp).fillMaxWidth(),
                color = ink.muted,
                fontSize = 16.sp,
                lineHeight = 22.sp,
                textAlign = TextAlign.Center,
            )
        }
        Box(
            Modifier
                .fillMaxWidth()
                .windowInsetsPadding(WindowInsets.navigationBars)
                .padding(start = 24.dp, end = 24.dp, bottom = 16.dp)
                .height(52.dp)
                .clip(RoundedCornerShape(14.dp))
                .background(accent)
                .clickable(role = Role.Button, onClick = onConnect),
            contentAlignment = Alignment.Center,
        ) {
            Text(
                stringResource(R.string.welcome_connect),
                color = onAccent,
                fontSize = 16.sp,
                fontWeight = FontWeight.SemiBold,
            )
        }
    }
}

@Composable
private fun Language(
    code: String,
    label: String,
    selected: Boolean,
    text: Color,
    onLanguage: (String) -> Unit,
) {
    Box(
        Modifier
            .padding(horizontal = 2.dp)
            .height(32.dp)
            .widthIn(min = 44.dp)
            .clip(RoundedCornerShape(8.dp))
            .background(if (selected) accent else Color.Transparent)
            .clickable(role = Role.Button) { onLanguage(code) },
        contentAlignment = Alignment.Center,
    ) {
        Text(
            label,
            Modifier.padding(horizontal = 10.dp),
            color = if (selected) onAccent else text,
            fontSize = 13.sp,
            fontWeight = FontWeight.SemiBold,
        )
    }
}

private fun activity(context: Context): Activity {
    var current = context
    while (current is ContextWrapper) {
        if (current is Activity) {
            return current
        }
        current = current.baseContext
    }
    error("Welcome has no activity")
}

/** The page colour behind the status and navigation bars, put back when this screen goes. */
@Composable
@Suppress("DEPRECATION")
private fun Bars(
    page: Color,
    dark: Boolean,
) {
    val view = LocalView.current
    DisposableEffect(page, dark) {
        val window = activity(view.context).window
        val status = window.statusBarColor
        val navigation = window.navigationBarColor
        val bars = WindowCompat.getInsetsController(window, view)
        val lightStatus = bars.isAppearanceLightStatusBars
        val lightNavigation = bars.isAppearanceLightNavigationBars
        val colour = page.toArgb()
        window.statusBarColor = colour
        window.navigationBarColor = colour
        // The page draws behind the three buttons, so their inset can lift the button clear of them.
        WindowCompat.setDecorFitsSystemWindows(window, false)
        bars.isAppearanceLightStatusBars = !dark
        bars.isAppearanceLightNavigationBars = !dark
        ViewCompat.requestApplyInsets(view)
        onDispose {
            WindowCompat.setDecorFitsSystemWindows(window, true)
            ViewCompat.requestApplyInsets(view)
            window.statusBarColor = status
            window.navigationBarColor = navigation
            bars.isAppearanceLightStatusBars = lightStatus
            bars.isAppearanceLightNavigationBars = lightNavigation
        }
    }
}
