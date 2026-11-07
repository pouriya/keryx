// The account setup screen: the server address, username and password, and a pasted setup link.
// Leaving the address checks the server. Sign up is always there. The captcha shows only when that
// server asks for one, with a refresh icon on the right of the picture.

package io.github.pouriya.keryx

import android.graphics.BitmapFactory
import android.util.Base64
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsFocusedAsState
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
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.ime
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawing
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.relocation.BringIntoViewRequester
import androidx.compose.foundation.relocation.bringIntoViewRequester
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ColorFilter
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalLayoutDirection
import androidx.compose.ui.platform.LocalSoftwareKeyboardController
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.unit.LayoutDirection
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

private val tile = Color(0xFF2A436B)
private val accent = Color(0xFF4576C4)
private val onAccent = Color(0xFFFFFFFF)

private data class Ink(
    val page: Color,
    val text: Color,
    val muted: Color,
    val line: Color,
    val field: Color,
    val danger: Color,
    val reached: Color,
)

@Composable
fun Setup(
    link: Boolean,
    signingUp: Boolean,
    address: String,
    onAddress: (String) -> Unit,
    onAddressBlur: () -> Unit,
    username: String,
    onUsername: (String) -> Unit,
    password: String,
    onPassword: (String) -> Unit,
    setupLink: String,
    onSetupLink: (String) -> Unit,
    onSetupLinkBlur: () -> Unit,
    checking: Boolean,
    reached: Boolean,
    addressError: String?,
    linkError: String?,
    formError: String?,
    showCaptcha: Boolean,
    captchaImage: String?,
    captchaAnswer: String,
    onCaptcha: (String) -> Unit,
    onRefreshCaptcha: () -> Unit,
    gap: String,
    focusNonce: Int,
    busy: Boolean,
    onPrimary: () -> Unit,
    onQuiet: () -> Unit,
    onLink: () -> Unit,
) {
    val dark = isSystemInDarkTheme()
    val ink =
        if (dark) {
            Ink(
                Color(0xFF141B24),
                Color(0xFFF4F6F8),
                Color(0xFF93A0B0),
                Color(0xFF243041),
                Color(0xFF1E2833),
                Color(0xFFFF6B6B),
                Color(0xFF6FCF97),
            )
        } else {
            Ink(
                Color(0xFFFFFFFF),
                Color(0xFF121212),
                Color(0xFF5E6A78),
                Color(0xFFE4E8F0),
                Color(0xFFFFFFFF),
                Color(0xFFC0392B),
                Color(0xFF1B7F4A),
            )
        }
    val addressFocus = remember { FocusRequester() }
    val usernameFocus = remember { FocusRequester() }
    val passwordFocus = remember { FocusRequester() }
    val captchaFocus = remember { FocusRequester() }
    val linkFocus = remember { FocusRequester() }
    val keyboard = LocalSoftwareKeyboardController.current
    LaunchedEffect(focusNonce) {
        val requester =
            when (gap) {
                "address" -> addressFocus
                "username" -> usernameFocus
                "password" -> passwordFocus
                "captcha" -> if (showCaptcha) captchaFocus else null
                "link" -> if (link) linkFocus else null
                else -> null
            }
        if (requester != null) {
            runCatching { requester.requestFocus() }
            keyboard?.show()
        }
    }
    Bars(ink.page, dark)
    val title =
        when {
            link -> stringResource(R.string.setup_title)
            signingUp -> stringResource(R.string.sign_up)
            else -> stringResource(R.string.log_in)
        }
    val primary =
        when {
            link -> stringResource(R.string.set_password)
            signingUp -> stringResource(R.string.sign_up)
            else -> stringResource(R.string.log_in)
        }
    val quiet = if (link || signingUp) stringResource(R.string.log_in) else stringResource(R.string.sign_up)
    Column(Modifier.fillMaxSize().background(ink.page).imePadding()) {
        Column(
            Modifier
                .weight(1f)
                .verticalScroll(rememberScrollState())
                .windowInsetsPadding(WindowInsets.safeDrawing.exclude(WindowInsets.navigationBars).exclude(WindowInsets.ime))
                .padding(horizontal = 24.dp),
        ) {
            Box(
                Modifier.padding(
                    top = 8.dp,
                ).size(56.dp).align(Alignment.CenterHorizontally).clip(RoundedCornerShape(14.dp)).background(tile),
                contentAlignment = Alignment.Center,
            ) {
                Image(painterResource(R.drawable.logo_foreground), null, Modifier.fillMaxSize())
            }
            Text(
                title,
                Modifier.padding(top = 8.dp, bottom = 14.dp).fillMaxWidth(),
                color = ink.text,
                fontSize = 22.sp,
                fontWeight = FontWeight.SemiBold,
                textAlign = androidx.compose.ui.text.style.TextAlign.Center,
            )
            if (link) {
                Field(
                    stringResource(R.string.setup_link_field),
                    setupLink,
                    onSetupLink,
                    ink,
                    if (gap == "link") stringResource(R.string.setup_need_link) else linkError ?: addressError,
                    onSetupLinkBlur,
                    ltr = true,
                    keyboard = KeyboardType.Uri,
                    focus = linkFocus,
                )
            } else {
                Field(
                    stringResource(R.string.server_address),
                    address,
                    onAddress,
                    ink,
                    addressError,
                    onAddressBlur,
                    ltr = true,
                    keyboard = KeyboardType.Uri,
                    checking = checking,
                    reached = reached,
                    focus = addressFocus,
                )
                Field(
                    stringResource(R.string.username),
                    username,
                    onUsername,
                    ink,
                    if (gap == "username") stringResource(R.string.setup_need_username) else null,
                    {},
                    ltr = true,
                    keyboard = KeyboardType.Ascii,
                    focus = usernameFocus,
                )
            }
            Field(
                stringResource(R.string.password),
                password,
                onPassword,
                ink,
                if (gap == "password") stringResource(R.string.setup_need_password) else null,
                {},
                password = true,
                keyboard = KeyboardType.Password,
                done = !showCaptcha,
                onDone = onPrimary,
                focus = passwordFocus,
            )
            if (showCaptcha) {
                Captcha(ink, captchaImage, onRefreshCaptcha)
                Field(
                    stringResource(R.string.captcha),
                    captchaAnswer,
                    onCaptcha,
                    ink,
                    if (gap == "captcha") stringResource(R.string.setup_need_code) else null,
                    {},
                    ltr = true,
                    keyboard = KeyboardType.Ascii,
                    done = true,
                    onDone = onPrimary,
                    focus = captchaFocus,
                )
            }
            if (!link) {
                Text(
                    stringResource(R.string.setup_link),
                    Modifier.clickable(role = Role.Button, onClick = onLink).padding(bottom = 12.dp),
                    color = accent,
                    fontSize = 14.sp,
                    fontWeight = FontWeight.SemiBold,
                )
            }
        }
        Column(
            Modifier.windowInsetsPadding(WindowInsets.navigationBars).padding(start = 24.dp, end = 24.dp, bottom = 16.dp),
        ) {
            if (formError != null) {
                Text(formError, Modifier.padding(bottom = 12.dp), color = ink.danger, fontSize = 14.sp)
            }
            Box(
                Modifier
                    .fillMaxWidth()
                    .height(52.dp)
                    .clip(RoundedCornerShape(14.dp))
                    .background(accent)
                    .clickable(enabled = !busy, role = Role.Button, onClick = onPrimary),
                contentAlignment = Alignment.Center,
            ) {
                Text(primary, color = onAccent, fontSize = 16.sp, fontWeight = FontWeight.SemiBold)
            }
            Text(
                quiet,
                Modifier.fillMaxWidth().clickable(enabled = !busy, role = Role.Button, onClick = onQuiet).padding(top = 12.dp),
                color = accent,
                fontSize = 16.sp,
                fontWeight = FontWeight.SemiBold,
                textAlign = androidx.compose.ui.text.style.TextAlign.Center,
            )
        }
    }
}

@Composable
private fun Captcha(
    ink: Ink,
    image: String?,
    onRefresh: () -> Unit,
) {
    val bitmap =
        remember(image) {
            val encoded = image?.substringAfter("base64,", "").orEmpty()
            if (encoded.isEmpty()) {
                null
            } else {
                val bytes = Base64.decode(encoded, Base64.DEFAULT)
                BitmapFactory.decodeByteArray(bytes, 0, bytes.size)?.asImageBitmap()
            }
        }
    Row(Modifier.fillMaxWidth().padding(bottom = 12.dp).height(72.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        Box(
            Modifier.weight(
                1f,
            ).fillMaxSize().clip(RoundedCornerShape(12.dp)).border(1.dp, ink.line, RoundedCornerShape(12.dp)).background(ink.field),
            contentAlignment = Alignment.Center,
        ) {
            if (bitmap != null) {
                Image(bitmap, null, Modifier.fillMaxSize(), contentScale = ContentScale.FillBounds)
            }
        }
        Box(
            Modifier
                .size(width = 52.dp, height = 72.dp)
                .clip(RoundedCornerShape(12.dp))
                .border(1.dp, ink.line, RoundedCornerShape(12.dp))
                .clickable(role = Role.Button, onClick = onRefresh),
            contentAlignment = Alignment.Center,
        ) {
            Image(
                painterResource(R.drawable.ic_refresh),
                stringResource(R.string.new_captcha),
                Modifier.size(24.dp),
                colorFilter = ColorFilter.tint(ink.text),
            )
        }
    }
}

@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun Field(
    label: String,
    value: String,
    onValue: (String) -> Unit,
    ink: Ink,
    error: String?,
    onBlur: () -> Unit,
    password: Boolean = false,
    ltr: Boolean = false,
    keyboard: KeyboardType = KeyboardType.Text,
    done: Boolean = false,
    onDone: () -> Unit = {},
    checking: Boolean = false,
    reached: Boolean = false,
    focus: FocusRequester? = null,
) {
    var seen by remember { mutableStateOf(false) }
    var shown by remember { mutableStateOf(false) }
    val hidden = password && !shown
    val intoView = remember { BringIntoViewRequester() }
    val scope = rememberCoroutineScope()
    val interaction = remember { MutableInteractionSource() }
    val focused by interaction.collectIsFocusedAsState()
    // The focused box is what the form scrolls to, so the sentence under it would sit behind the
    // buttons. Scroll the whole field, and again once the keyboard has finished moving.
    LaunchedEffect(focused, error, shown) {
        if (!focused && error == null) return@LaunchedEffect
        intoView.bringIntoView()
        delay(250)
        intoView.bringIntoView()
    }
    val rtl = LocalLayoutDirection.current == LayoutDirection.Rtl
    val border =
        when {
            error != null && !checking -> ink.danger
            reached && !checking -> ink.reached
            focused -> accent
            else -> ink.line
        }
    Column(Modifier.fillMaxWidth().bringIntoViewRequester(intoView).padding(bottom = 12.dp)) {
        Text(label, color = ink.muted, fontSize = 13.sp)
        Box(
            Modifier
                .padding(top = 6.dp)
                .fillMaxWidth()
                .heightIn(min = 48.dp)
                .border(1.dp, border, RoundedCornerShape(12.dp))
                .background(ink.field, RoundedCornerShape(12.dp))
                .padding(start = 14.dp, top = 12.dp, end = if (password) 4.dp else 14.dp, bottom = 12.dp),
        ) {
            val text = @Composable {
                BasicTextField(
                    value = value,
                    onValueChange = onValue,
                    modifier =
                        Modifier
                            .fillMaxWidth()
                            .then(if (focus != null) Modifier.focusRequester(focus) else Modifier)
                            .padding(
                                start = if (checking && rtl) 26.dp else 0.dp,
                                end = (if (checking && !rtl) 26.dp else 0.dp) + (if (password) 36.dp else 0.dp),
                            ).onFocusChanged {
                                if (it.isFocused) {
                                    seen = true
                                } else if (seen) {
                                    onBlur()
                                }
                            },
                    textStyle = TextStyle(color = ink.text, fontSize = 16.sp),
                    singleLine = true,
                    visualTransformation = if (hidden) PasswordVisualTransformation() else VisualTransformation.None,
                    keyboardOptions =
                        KeyboardOptions(
                            capitalization = KeyboardCapitalization.None,
                            autoCorrectEnabled = false,
                            keyboardType = if (password) KeyboardType.Password else keyboard,
                            imeAction = if (done) ImeAction.Done else ImeAction.Next,
                        ),
                    keyboardActions = KeyboardActions(onDone = { onDone() }),
                    cursorBrush = SolidColor(accent),
                    interactionSource = interaction,
                    decorationBox = { inner ->
                        Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.CenterStart) { inner() }
                    },
                )
            }
            Box(Modifier.align(Alignment.CenterStart).fillMaxWidth()) {
                if (ltr) {
                    CompositionLocalProvider(LocalLayoutDirection provides LayoutDirection.Ltr) { text() }
                } else {
                    text()
                }
            }
            if (password) {
                Image(
                    painterResource(if (shown) R.drawable.ic_eye_off else R.drawable.ic_eye),
                    stringResource(if (shown) R.string.hide_password else R.string.show_password),
                    Modifier
                        .align(Alignment.CenterEnd)
                        .size(40.dp)
                        .clickable(role = Role.Button) {
                            shown = !shown
                            scope.launch {
                                intoView.bringIntoView()
                                delay(250)
                                intoView.bringIntoView()
                            }
                        }
                        .padding(8.dp),
                    colorFilter = ColorFilter.tint(ink.text),
                )
            }
            if (checking) {
                CircularProgressIndicator(
                    Modifier.align(Alignment.CenterEnd).size(18.dp),
                    color = accent,
                    strokeWidth = 2.dp,
                    strokeCap = StrokeCap.Round,
                )
            }
        }
        if (error != null && !checking) {
            Text(error, Modifier.padding(top = 6.dp), color = ink.danger, fontSize = 14.sp)
        }
    }
}
