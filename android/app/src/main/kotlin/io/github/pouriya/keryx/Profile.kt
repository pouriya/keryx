// The profile, as android/AGENTS.md chose: your own page with an editor per line, the photo sheet
// and the crop screen, someone else's page, and the round photo every screen draws a person with.
// Opened photos come from the engine's cache folder, decoded off the main thread. Nothing here logs.

package io.github.pouriya.keryx

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.ImageDecoder
import android.graphics.Paint
import android.net.Uri
import android.os.Build
import android.util.LruCache
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.PickVisualMediaRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.Image
import androidx.compose.foundation.ScrollState
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectTransformGestures
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.asPaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.ime
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.layout.union
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.RadioButton
import androidx.compose.material3.RadioButtonDefaults
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.graphics.PathFillType
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.scale
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.lerp
import androidx.compose.ui.graphics.vector.PathParser
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.platform.LocalLayoutDirection
import androidx.compose.ui.platform.LocalUriHandler
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextDirection
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.unit.LayoutDirection
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.ByteArrayOutputStream
import java.nio.ByteBuffer
import java.time.Instant
import java.time.LocalDate
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.time.format.FormatStyle
import kotlin.math.max
import kotlin.math.min
import kotlin.math.roundToInt
import android.graphics.Canvas as Board
import android.graphics.Rect as Area

private val tile = Color(0xFF2A436B)
private val accent = Color(0xFF4576C4)
private val red = Color(0xFFC93C3C)
private val toastInk = Color(0xFF2B2F36)

/** Root AGENTS.md's limits, which core checks again before anything goes. */
private const val NAME_CHARS = 64
private const val BIO_CHARS = 140
private const val LINKS = 3
private const val LINK_NAME_CHARS = 32
private const val LINK_URL_CHARS = 512

/** One square JPEG of 640x640, under the server's 1 MiB once sealed. */
private const val PHOTO_SIDE = 640
private const val PHOTO_BYTES = (1 shl 20) - 64

/** A picked picture is decoded at most this large for the crop screen. */
private const val CROP_SOURCE = 2048
private const val MAX_ZOOM = 3f

/**
 * Kept photos decoded at half size, 320 pixels, are plenty for 128 dp, about 352 pixels on most
 * phones; 16 MiB holds about 40. The full view decodes its one photo whole.
 */
private const val DECODED_BYTES = 16 shl 20

/** Profiles as core shows them, their photos, and the save an editor waits on. */
internal class Profiles(
    private val engine: Engine,
    private val people: Map<String, AccountBrief>,
) {
    val shown = mutableStateMapOf<String, ShownProfile>()

    /** Bumped when a photo's file is kept, so whatever draws it reads it again. */
    val kept = mutableStateMapOf<String, Int>()
    val saving = mutableStateOf(false)
    val saves = mutableStateOf(0)
    val refusal = mutableStateOf<Event.Error?>(null)

    /** Where each person is, from the server; dropped while this device is offline, as it cannot know. */
    val presence = mutableStateMapOf<String, PresenceItem>()
    private val presenceText = mutableStateMapOf<String, String>()

    /** A new photo while it goes up, drawn on your page with a turning ring. */
    val uploading = mutableStateOf<ImageBitmap?>(null)
    private var me: String? = null
    private val images =
        object : LruCache<String, ImageBitmap>(DECODED_BYTES) {
            override fun sizeOf(
                key: String,
                value: ImageBitmap,
            ) = value.width * value.height * 4
        }

    fun apply(event: Event) {
        when (event) {
            is Event.SignedIn -> me = event.account
            is Event.Profile -> shown[event.profile.account] = event.profile
            is Event.Account ->
                shown[event.account.id]?.let {
                    shown[it.account] = it.copy(username = event.account.username, display_name = event.account.display_name)
                }
            is Event.Photo -> bump(event.photo)
            is Event.Presence -> {
                val account = event.presence.account
                presence[account] = event.presence
                val text = event.text
                if (text != null) presenceText[account] = text else presenceText.remove(account)
            }
            is Event.Offline -> {
                presence.clear()
                presenceText.clear()
            }
            is Event.ProfileSaved -> {
                saving.value = false
                uploading.value = null
                saves.value += 1
                me?.let { shown[it]?.photo?.id }?.let(::bump)
            }
            is Event.Error ->
                if (saving.value || uploading.value != null) {
                    refusal.value = event
                    saving.value = false
                    uploading.value = null
                }
            is Event.LoggedOut -> {
                shown.clear()
                presence.clear()
                presenceText.clear()
                kept.clear()
                me = null
                saving.value = false
                uploading.value = null
                refusal.value = null
            }
            else -> Unit
        }
    }

    fun isAgent(
        account: String,
        username: String? = null,
    ): Boolean = shown[account]?.agent == true || (username ?: shown[account]?.username ?: people[account]?.username).isAgentUsername()

    fun troubleDetail(account: String): String? = presenceText[account]?.takeIf { me != null && shown[account]?.owner == me }

    private fun bump(id: String) {
        kept[id] = (kept[id] ?: 0) + 1
    }

    fun cached(id: String): ImageBitmap? = images.get(id)

    /** Reads a kept photo off the main thread, or null when it is not kept yet. */
    suspend fun load(id: String): ImageBitmap? =
        withContext(Dispatchers.IO) {
            val file = engine.photoFile(id)
            if (!file.exists()) return@withContext null
            val options = BitmapFactory.Options().apply { inSampleSize = 2 }
            BitmapFactory.decodeFile(file.path, options)?.asImageBitmap()?.also { images.put(id, it) }
        }

    /** A kept photo at its full 640 pixels, for the full view only, so it is never cached. */
    suspend fun loadFull(id: String): ImageBitmap? =
        withContext(Dispatchers.IO) {
            val file = engine.photoFile(id)
            if (file.exists()) BitmapFactory.decodeFile(file.path)?.asImageBitmap() else null
        }

    fun ask(
        account: String,
        id: String,
    ) = engine.photo(account, id)

    fun open(
        account: String,
        refreshPresence: Boolean = false,
    ) {
        engine.send(Command.Profile(account))
        // Someone sharing no chat is asked about once, as the root AGENTS.md says.
        if (account != me && (refreshPresence || account !in presence)) engine.send(Command.PresenceOf(account))
    }

    /** Sends an editor's change, which closes it once saved or shows the refusal. */
    fun save(command: Command) {
        refusal.value = null
        saving.value = true
        engine.send(command)
    }

    fun setPhoto(
        cropped: Bitmap,
        jpeg: ByteArray,
        thumbnail: Thumbnail,
    ) {
        refusal.value = null
        uploading.value = cropped.asImageBitmap()
        engine.setPhoto(jpeg, thumbnail)
    }

    fun removePhoto() = engine.send(Command.RemovePhoto)
}

internal val LocalProfiles = staticCompositionLocalOf<Profiles?> { null }

internal const val AGENT_SUFFIX = "_agent"

private fun String?.isAgentUsername() = this?.endsWith(AGENT_SUFFIX) == true

/**
 * A person's round photo: the opened photo once kept, its BlurHash while it is fetched, or the
 * first letter of their name on blue when they have none.
 */
@Composable
internal fun Avatar(
    account: String?,
    name: String,
    size: Dp,
    modifier: Modifier = Modifier,
    username: String? = null,
    badgeRing: Color = ink().page,
    badgeSize: Dp = 14.dp,
) {
    val profiles = LocalProfiles.current
    val photo = account?.let { profiles?.shown?.get(it)?.photo }
    Box(modifier.size(size), contentAlignment = Alignment.Center) {
        Box(Modifier.fillMaxSize().clip(CircleShape).background(tile), contentAlignment = Alignment.Center) {
            val image = if (profiles != null && account != null && photo != null) photoOf(profiles, account, photo) else null
            val blur = photo?.let { blurOf(it.blurhash) }
            when {
                image != null -> Image(image, null, Modifier.fillMaxSize(), contentScale = ContentScale.Crop)
                blur != null -> Image(blur, null, Modifier.fillMaxSize(), contentScale = ContentScale.Crop)
                else ->
                    Text(
                        name.firstOrNull()?.uppercaseChar()?.toString() ?: "",
                        color = Color.White,
                        fontSize = (size.value * 0.38f).sp,
                        fontWeight = FontWeight.SemiBold,
                    )
            }
        }
        if (account != null && (profiles?.isAgent(account, username) == true || username.isAgentUsername())) {
            AgentBadge(Modifier.align(Alignment.BottomStart), badgeSize, badgeRing)
        }
    }
}

@Composable
private fun AgentBadge(
    modifier: Modifier,
    size: Dp,
    ring: Color,
) {
    val label = stringResource(R.string.agent_badge)
    Box(
        modifier.size(size).clip(CircleShape).background(ring).padding(2.dp).clip(CircleShape).background(tile)
            .semantics { contentDescription = label },
        contentAlignment = Alignment.Center,
    ) {
        Glyph(Glyphs.cpu, Color.White, (size - 4.dp) * 0.9f, width = 1.8f)
    }
}

/** The kept photo, read again whenever a fetch keeps it, and fetched if it is not kept. */
@Composable
private fun photoOf(
    profiles: Profiles,
    account: String,
    photo: ShownPhoto,
): ImageBitmap? {
    val version = profiles.kept[photo.id] ?: 0
    var image by remember(photo.id) { mutableStateOf(profiles.cached(photo.id)) }
    LaunchedEffect(photo.id, version) {
        val loaded = profiles.cached(photo.id) ?: profiles.load(photo.id)
        image = loaded
        if (loaded == null) profiles.ask(account, photo.id)
    }
    return image
}

@Composable
private fun blurOf(hash: String): ImageBitmap? =
    remember(hash) {
        Core.blurhash(hash, 32, 32)?.let { rgba ->
            Bitmap
                .createBitmap(32, 32, Bitmap.Config.ARGB_8888)
                .apply { copyPixelsFromBuffer(ByteBuffer.wrap(rgba)) }
                .asImageBitmap()
        }
    }

private enum class Editing { Name, Username, Bio, Links }

/**
 * Someone's profile, or your own with its editors. Drawn over the screen it was opened from, which
 * keeps its state underneath, so this page takes every touch and the back button.
 */
@Composable
internal fun ProfileScreen(
    account: String,
    mine: Boolean,
    brief: AccountBrief?,
    onBack: () -> Unit,
    onDevices: () -> Unit,
    onAgents: () -> Unit,
    onOwner: (String) -> Unit,
    onLogOut: () -> Unit,
    theme: String,
    onTheme: (String) -> Unit,
    language: String,
    onLanguage: (String) -> Unit,
    server: String,
    actions: ProfileActions,
) {
    val profiles = LocalProfiles.current ?: return
    val dark = isDark()
    val ink = ink()
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val focusManager = LocalFocusManager.current
    var editing by remember { mutableStateOf<Editing?>(null) }
    var sheet by remember { mutableStateOf(false) }
    var cropping by remember { mutableStateOf<Bitmap?>(null) }
    var viewing by remember { mutableStateOf<String?>(null) }
    val picker =
        rememberLauncherForActivityResult(ActivityResultContracts.PickVisualMedia()) { uri ->
            if (uri != null) {
                scope.launch {
                    cropping = withContext(Dispatchers.IO) { forCrop(context, uri) }
                }
            }
        }
    LaunchedEffect(account) {
        focusManager.clearFocus()
        profiles.open(account)
    }
    BackHandler {
        when {
            sheet -> sheet = false
            viewing != null -> viewing = null
            cropping != null -> cropping = null
            editing != null -> editing = null
            else -> onBack()
        }
    }
    val profile = profiles.shown[account]
    val username = profile?.username ?: brief?.username ?: ""
    val name = profile?.display_name?.takeIf { it.isNotBlank() } ?: brief?.display_name?.takeIf { it.isNotBlank() } ?: username
    Box(
        Modifier
            .fillMaxSize()
            .background(ink.page)
            .pointerInput(Unit) { awaitPointerEventScope { while (true) awaitPointerEvent() } },
    ) {
        val source = cropping
        val opened = editing
        val full = viewing
        when {
            full != null -> FullPhoto(profiles, full) { viewing = null }
            source != null -> {
                Crop(source, { cropping = null }) { square ->
                    cropping = null
                    scope.launch {
                        val (jpeg, thumbnail) = withContext(Dispatchers.Default) { encode(square) }
                        if (jpeg != null) profiles.setPhoto(square, jpeg, thumbnail)
                    }
                }
            }
            opened != null -> {
                Bars(ink.page, dark)
                val close = { editing = null }
                when (opened) {
                    Editing.Name -> NameEditor(profiles, name, ink, close)
                    Editing.Username -> UsernameEditor(profiles, username, ink, close)
                    Editing.Bio -> BioEditor(profiles, profile, ink, close)
                    Editing.Links -> LinksEditor(profiles, profile, ink, close)
                }
            }
            else -> {
                Page(
                    profiles,
                    account,
                    mine,
                    profile,
                    name,
                    username,
                    ink,
                    server,
                    actions,
                    onBack,
                    { editing = it },
                    { sheet = true },
                    { viewing = it },
                    onDevices,
                    onAgents,
                    onOwner,
                    onLogOut,
                    Looks(theme, onTheme, language, onLanguage),
                )
            }
        }
        if (sheet) {
            PhotoSheet(profile?.photo != null, ink, { sheet = false }, {
                sheet = false
                picker.launch(PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageOnly))
            }) {
                sheet = false
                profiles.removePhoto()
            }
        }
    }
}

/** What someone else's page can start from it: their chat, and a call in it. Null hides the button. */
internal class ProfileActions(
    val onMessage: (() -> Unit)?,
    val onCall: ((Boolean) -> Unit)?,
)

/** The big photo's height under the status bar, from `tmp/design/profile-redesign.html`'s B, chosen by the author. */
private val HERO = 300.dp

/**
 * The page, as B in `tmp/design/profile-redesign.html`, chosen by the author: the photo across the
 * top with the name over its lower edge, then rounded cards. Once the photo has scrolled away, a solid
 * bar with the name fades in, chosen by the author.
 */
@Composable
private fun Page(
    profiles: Profiles,
    account: String,
    mine: Boolean,
    profile: ShownProfile?,
    name: String,
    username: String,
    ink: Ink,
    server: String,
    actions: ProfileActions,
    onBack: () -> Unit,
    onEdit: (Editing) -> Unit,
    onPhoto: () -> Unit,
    onFull: (String) -> Unit,
    onDevices: () -> Unit,
    onAgents: () -> Unit,
    onOwner: (String) -> Unit,
    onLogOut: () -> Unit,
    looks: Looks,
) {
    val clipboard = LocalClipboardManager.current
    var choosing by remember { mutableStateOf<String?>(null) }
    val uri = LocalUriHandler.current
    val copied = stringResource(R.string.chat_copied)
    var toast by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(toast) {
        if (toast != null) {
            delay(1500)
            toast = null
        }
    }
    val dark = isDark()
    val ground = if (dark) Color(0xFF0F151D) else Color(0xFFF2F4F8)
    val card = if (dark) Color(0xFF1A2330) else Color.White
    val scroll = rememberScrollState()
    val top = WindowInsets.statusBars.asPaddingValues().calculateTopPadding()
    val links = profile?.link_list.orEmpty()
    Box(Modifier.fillMaxSize().background(ground)) {
        Column(Modifier.fillMaxSize().verticalScroll(scroll)) {
            Hero(profiles, account, mine, name, username, top, onPhoto, onFull)
            if (mine) {
                Section(stringResource(R.string.profile_account), ink)
                Card(
                    card,
                    ink,
                    buildList<@Composable () -> Unit> {
                        add {
                            Line(
                                Glyphs.person,
                                name,
                                stringResource(R.string.profile_name),
                                ink,
                                chevron = true,
                            ) { onEdit(Editing.Name) }
                        }
                        add {
                            Line(Glyphs.at, "@$username", stringResource(R.string.username), ink, ltr = true, chevron = true) {
                                onEdit(Editing.Username)
                            }
                        }
                        val bio = profile?.bio?.takeIf { it.isNotBlank() }
                        add {
                            if (bio != null) {
                                Line(Glyphs.info, bio, stringResource(R.string.profile_bio), ink, chevron = true) { onEdit(Editing.Bio) }
                            } else {
                                Line(
                                    Glyphs.info,
                                    stringResource(R.string.profile_bio_add),
                                    stringResource(R.string.profile_bio_hint),
                                    ink,
                                    tone = Tone.Add,
                                ) { onEdit(Editing.Bio) }
                            }
                        }
                        for (link in links) {
                            add {
                                Line(Glyphs.link, shortUrl(link.url), link.name, ink, ltr = true, tone = Tone.Link, chevron = true) {
                                    onEdit(Editing.Links)
                                }
                            }
                        }
                        if (links.size < LINKS) {
                            add {
                                Line(
                                    Glyphs.plus,
                                    stringResource(R.string.profile_link_add),
                                    if (links.isEmpty()) stringResource(R.string.profile_links_up_to) else null,
                                    ink,
                                    tone = Tone.Add,
                                ) { onEdit(Editing.Links) }
                            }
                        }
                    },
                )
                Section(stringResource(R.string.profile_settings), ink)
                Card(
                    card,
                    ink,
                    listOf<@Composable () -> Unit>(
                        {
                            Line(
                                Glyphs.globe,
                                stringResource(R.string.profile_language),
                                null,
                                ink,
                                end = languageName(looks.language),
                                chevron = true,
                            ) {
                                choosing = "lang"
                            }
                        },
                        {
                            Line(
                                Glyphs.moon,
                                stringResource(R.string.profile_theme),
                                null,
                                ink,
                                end = themeName(looks.theme),
                                chevron = true,
                            ) {
                                choosing = "theme"
                            }
                        },
                        { Line(Glyphs.devices, stringResource(R.string.devices), null, ink, chevron = true, onClick = onDevices) },
                        { Line(Glyphs.cpu, stringResource(R.string.agents_title), null, ink, chevron = true, onClick = onAgents) },
                    ),
                )
                Spacer(Modifier.height(12.dp))
                Card(
                    card,
                    ink,
                    listOf<@Composable () -> Unit>({
                        Line(Glyphs.out, stringResource(R.string.log_out), null, ink, tone = Tone.Danger, onClick = onLogOut)
                    }),
                )
                Text(
                    server,
                    Modifier.fillMaxWidth().padding(top = 18.dp, bottom = 8.dp),
                    color = ink.muted,
                    fontSize = 12.5.sp,
                    textAlign = TextAlign.Center,
                )
            } else {
                Spacer(Modifier.height(14.dp))
                Actions(actions, card)
                Section(stringResource(R.string.profile_info), ink)
                val times = offlineTimes(profiles.presence[account])
                Card(
                    card,
                    ink,
                    buildList<@Composable () -> Unit> {
                        profile?.owner?.let { owner ->
                            add {
                                val ownerProfile = profiles.shown[owner]
                                LaunchedEffect(owner) { profiles.open(owner) }
                                val label = stringResource(R.string.agent_owner)
                                val name = ownerProfile?.display_name ?: ownerProfile?.username
                                Line(Glyphs.person, name ?: label, if (name != null) label else null, ink, chevron = true) {
                                    onOwner(owner)
                                }
                            }
                        }
                        profile?.bio?.takeIf { it.isNotBlank() }?.let { bio ->
                            add { Line(Glyphs.info, bio, stringResource(R.string.profile_bio), ink, onClick = null) }
                        }
                        for (link in links) {
                            add {
                                Line(Glyphs.link, shortUrl(link.url), link.name, ink, ltr = true, tone = Tone.Link) {
                                    runCatching { uri.openUri(link.url) }
                                }
                            }
                        }
                        if (username.isNotEmpty()) {
                            add {
                                Line(
                                    Glyphs.at,
                                    "@$username",
                                    stringResource(R.string.profile_username_copy),
                                    ink,
                                    ltr = true,
                                    trailing = { Glyph(Glyphs.copy, ink.muted, 18.dp) },
                                ) {
                                    clipboard.setText(AnnotatedString(username))
                                    // Android 13 and later says it was copied itself.
                                    if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) toast = copied
                                }
                            }
                        }
                        times?.first?.let { at ->
                            add { Line(Glyphs.eye, presenceWhen(at), stringResource(R.string.presence_active), ink, onClick = null) }
                        }
                        times?.second?.let { at ->
                            add { Line(Glyphs.plug, presenceWhen(at), stringResource(R.string.presence_connected), ink, onClick = null) }
                        }
                    },
                )
            }
            Spacer(Modifier.windowInsetsPadding(WindowInsets.navigationBars).padding(bottom = 12.dp))
        }
        ProfileBar(name, ink, scroll, onBack, onPhoto.takeIf { mine })
        when (choosing) {
            "lang" ->
                Choices(
                    stringResource(R.string.profile_language),
                    listOf("en" to languageName("en"), "fa" to languageName("fa")),
                    looks.language,
                    ink,
                    { choosing = null },
                ) {
                    choosing = null
                    looks.onLanguage(it)
                }
            "theme" ->
                Choices(
                    stringResource(R.string.profile_theme),
                    listOf("system", "light", "dark").map { it to themeName(it) },
                    looks.theme,
                    ink,
                    { choosing = null },
                ) {
                    choosing = null
                    looks.onTheme(it)
                }
        }
        toast?.let {
            Text(
                it,
                Modifier
                    .align(Alignment.BottomCenter)
                    .windowInsetsPadding(WindowInsets.navigationBars)
                    .padding(bottom = 40.dp)
                    .clip(RoundedCornerShape(18.dp))
                    .background(toastInk)
                    .padding(horizontal = 14.dp, vertical = 8.dp),
                color = Color.White,
                fontSize = 14.sp,
            )
        }
    }
}

/** Stays over the profile photo and takes the page's colours once it scrolls away. */
@Composable
internal fun ProfileBar(
    name: String,
    ink: Ink,
    scroll: ScrollState,
    onBack: () -> Unit,
    onPhoto: (() -> Unit)?,
) {
    val dark = isDark()
    val rtl = LocalLayoutDirection.current == LayoutDirection.Rtl
    val solid = with(LocalDensity.current) { scroll.value > (HERO - 56.dp).toPx() }
    val fade by animateFloatAsState(if (solid) 1f else 0f, label = "bar")
    Bars(if (solid) ink.page else Color.Transparent, if (solid) dark else true)
    val mark = lerp(Color.White, ink.text, fade)
    Row(
        Modifier.fillMaxWidth().background(ink.page.copy(alpha = fade))
            .windowInsetsPadding(WindowInsets.statusBars).height(56.dp).padding(start = 4.dp, end = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        IconButton(stringResource(R.string.back), onBack, 48.dp) { BackMark(mark, rtl) }
        Text(
            name,
            Modifier.weight(1f).padding(horizontal = 4.dp).alpha(fade),
            color = ink.text,
            fontSize = 18.sp,
            fontWeight = FontWeight.SemiBold,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
        if (onPhoto != null) {
            IconButton(stringResource(R.string.profile_change_photo), onPhoto, 48.dp) { Glyph(Glyphs.cam, mark, 24.dp) }
        }
    }
}

/** Your own page's Language and Theme lines: kv's `lang` and `theme`, and what changes them. */
internal class Looks(
    val theme: String,
    val onTheme: (String) -> Unit,
    val language: String,
    val onLanguage: (String) -> Unit,
)

/** Each language in its own words, so it can be found whichever is on. */
private fun languageName(code: String) = if (code == "fa") "فارسی" else "English"

@Composable
private fun themeName(theme: String) =
    stringResource(
        when (theme) {
            "light" -> R.string.theme_light
            "dark" -> R.string.theme_dark
            else -> R.string.theme_system
        },
    )

/**
 * Someone's photo whole, as large as fits on black, with a back arrow, chosen by the author. No
 * zoom. Back or the arrow returns to their profile.
 */
@Composable
private fun FullPhoto(
    profiles: Profiles,
    id: String,
    onBack: () -> Unit,
) {
    val rtl = LocalLayoutDirection.current == LayoutDirection.Rtl
    var image by remember(id) { mutableStateOf(profiles.cached(id)) }
    // Read again once a fetch still under way keeps it.
    val version = profiles.kept[id] ?: 0
    LaunchedEffect(id, version) { profiles.loadFull(id)?.let { image = it } }
    Bars(Color.Black, true)
    Box(Modifier.fillMaxSize().background(Color.Black)) {
        image?.let { Image(it, null, Modifier.fillMaxSize(), contentScale = ContentScale.Fit) }
        Row(
            Modifier.windowInsetsPadding(WindowInsets.statusBars).height(56.dp).padding(start = 4.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            IconButton(stringResource(R.string.back), onBack, 48.dp) { BackMark(Color.White, rtl) }
        }
    }
}

/** The state's word over the photo, in a lighter shade of the dot's colour so it reads on the dark edge. */
private fun heroTone(state: String?): Color? =
    when (state) {
        "online" -> Color(0xFF7EE2A0)
        "call" -> Color(0xFFFF8A8A)
        "away" -> Color(0xFFFFC27A)
        "trouble" -> troubleTone(dark = true)
        else -> null
    }

/**
 * The photo across the whole width, reaching under the status bar, with the name and a line over its
 * lower edge: your @username, or their state. No photo is the letter on a blue gradient. Tapping opens
 * the photo sheet on yours, and the whole photo on theirs, as before, chosen by the author.
 */
@Composable
internal fun Hero(
    profiles: Profiles,
    account: String,
    mine: Boolean,
    name: String,
    username: String,
    top: Dp,
    onPhoto: () -> Unit,
    onFull: (String) -> Unit,
    agent: Boolean = profiles.isAgent(account, username),
    editable: Boolean = mine,
) {
    val photo = profiles.shown[account]?.photo
    val going = profiles.uploading.value.takeIf { mine }
    val tap =
        when {
            editable ->
                Modifier.clickable(
                    role = Role.Button,
                    onClickLabel = stringResource(R.string.profile_change_photo),
                    onClick = onPhoto,
                )
            photo != null -> Modifier.clickable(role = Role.Button) { onFull(photo.id) }
            else -> Modifier
        }
    Box(Modifier.fillMaxWidth().height(HERO + top).clipToBounds().then(tap)) {
        val small = photo?.let { photoOf(profiles, account, it) }
        // The kept photo at its full 640 pixels, since it is drawn across the screen.
        var big by remember(photo?.id) { mutableStateOf<ImageBitmap?>(null) }
        val version = photo?.let { profiles.kept[it.id] } ?: 0
        LaunchedEffect(photo?.id, version) { big = photo?.let { profiles.loadFull(it.id) } }
        val blur = photo?.let { blurOf(it.blurhash) }
        val image = going ?: big ?: small ?: blur
        if (image != null) {
            Image(image, null, Modifier.fillMaxSize(), contentScale = ContentScale.Crop)
        } else {
            Box(
                Modifier.fillMaxSize().background(Brush.linearGradient(listOf(accent, tile))),
                contentAlignment = Alignment.Center,
            ) {
                Text(
                    name.firstOrNull()?.uppercaseChar()?.toString() ?: "",
                    Modifier.padding(top = top),
                    color = Color.White,
                    fontSize = 110.sp,
                    fontWeight = FontWeight.SemiBold,
                )
            }
        }
        if (going != null) {
            CircularProgressIndicator(
                Modifier.align(Alignment.Center).padding(top = top).size(56.dp),
                color = Color.White,
                strokeWidth = 3.dp,
                strokeCap = StrokeCap.Round,
            )
        }
        // Shades at the top and the bottom, so the white arrow and the name read on a light photo.
        Box(
            Modifier
                .fillMaxWidth()
                .height(top + 72.dp)
                .background(Brush.verticalGradient(listOf(Color.Black.copy(alpha = 0.32f), Color.Transparent))),
        )
        Box(
            Modifier
                .align(Alignment.BottomCenter)
                .fillMaxWidth()
                .height(140.dp)
                .background(Brush.verticalGradient(listOf(Color.Transparent, Color.Black.copy(alpha = 0.55f)))),
        )
        Column(
            Modifier.align(Alignment.BottomStart).fillMaxWidth()
                .padding(start = 18.dp, end = if (agent) 58.dp else 18.dp, bottom = 14.dp),
        ) {
            Text(
                name,
                color = Color.White,
                fontSize = 24.sp,
                fontWeight = FontWeight.Bold,
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
            )
            val item = profiles.presence[account]
            val line = if (mine) null else presenceLine(item)
            val ltr = TextStyle(textDirection = TextDirection.Ltr)
            Text(
                line ?: "@$username",
                Modifier.padding(top = 2.dp),
                color = heroTone(item?.state).takeIf { line != null } ?: Color.White.copy(alpha = 0.88f),
                fontSize = 15.sp,
                style = if (line == null) ltr else TextStyle.Default,
            )
        }
        if (agent) {
            AgentBadge(
                Modifier.align(Alignment.BottomEnd).padding(end = 18.dp, bottom = 14.dp),
                28.dp,
                Color.White.copy(alpha = 0.7f),
            )
        }
    }
}

/** Message, Voice call and Video call on someone else's page, each in its own rounded tile. */
@Composable
private fun Actions(
    actions: ProfileActions,
    card: Color,
) {
    val tone = accentInk()
    val shown =
        buildList<Triple<List<String>, Int, () -> Unit>> {
            actions.onMessage?.let { add(Triple(Glyphs.chat, R.string.profile_message, it)) }
            actions.onCall?.let { call ->
                add(Triple(Glyphs.phone, R.string.profile_voice_call, { call(false) }))
                add(Triple(Glyphs.video, R.string.profile_video_call, { call(true) }))
            }
        }
    if (shown.isEmpty()) return
    Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        for ((glyph, label, onClick) in shown) {
            Column(
                Modifier
                    .weight(1f)
                    .clip(RoundedCornerShape(14.dp))
                    .background(card)
                    .clickable(role = Role.Button, onClick = onClick)
                    .padding(top = 10.dp, bottom = 8.dp),
                horizontalAlignment = Alignment.CenterHorizontally,
                verticalArrangement = Arrangement.spacedBy(4.dp),
            ) {
                Glyph(glyph, tone, 22.dp)
                Text(stringResource(label), color = tone, fontSize = 13.sp, fontWeight = FontWeight.SemiBold, maxLines = 1)
            }
        }
    }
}

/** The accent for text and marks: the accent by day, its light shade at night. */
@Composable
private fun accentInk() = if (isDark()) Color(0xFFC5D4E8) else accent

/** A card's small title. */
@Composable
private fun Section(
    title: String,
    ink: Ink,
) {
    Text(
        title.uppercase(),
        Modifier.padding(start = 28.dp, end = 28.dp, top = 18.dp, bottom = 6.dp),
        color = ink.muted,
        fontSize = 12.5.sp,
        fontWeight = FontWeight.SemiBold,
        letterSpacing = 0.5.sp,
    )
}

/** Rounded lines on the page, with a thin rule between them that starts after the icons. */
@Composable
private fun Card(
    card: Color,
    ink: Ink,
    lines: List<@Composable () -> Unit>,
) {
    Column(Modifier.padding(horizontal = 12.dp).fillMaxWidth().clip(RoundedCornerShape(16.dp)).background(card)) {
        lines.forEachIndexed { at, line ->
            if (at > 0) Spacer(Modifier.padding(start = 54.dp).fillMaxWidth().height(1.dp).background(ink.line))
            line()
        }
    }
}

private enum class Tone { Plain, Add, Link, Danger }

/**
 * One line of a card: an icon, its value and a small label under it, and at the end a value, a mark
 * such as copy, or a chevron when it opens something.
 */
@Composable
private fun Line(
    icon: List<String>,
    value: String,
    label: String?,
    ink: Ink,
    ltr: Boolean = false,
    tone: Tone = Tone.Plain,
    end: String? = null,
    chevron: Boolean = false,
    trailing: (@Composable () -> Unit)? = null,
    onClick: (() -> Unit)?,
) {
    val tap = if (onClick != null) Modifier.clickable(role = Role.Button, onClick = onClick) else Modifier
    val rtl = LocalLayoutDirection.current == LayoutDirection.Rtl
    val strong =
        when (tone) {
            Tone.Plain -> ink.text
            Tone.Add, Tone.Link -> accentInk()
            Tone.Danger -> ink.danger
        }
    Row(
        Modifier.fillMaxWidth().heightIn(min = 54.dp).then(tap).padding(horizontal = 16.dp, vertical = 11.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        Box(Modifier.width(24.dp), contentAlignment = Alignment.Center) {
            Glyph(icon, if (tone == Tone.Plain || tone == Tone.Link) ink.muted else strong)
        }
        Column(Modifier.weight(1f)) {
            Text(
                value,
                color = strong,
                fontSize = 16.sp,
                style = if (ltr) TextStyle(textDirection = TextDirection.Ltr) else TextStyle.Default,
            )
            if (label != null) Text(label, Modifier.padding(top = 1.dp), color = ink.muted, fontSize = 13.sp)
        }
        if (end != null) Text(end, color = ink.muted, fontSize = 15.sp)
        trailing?.invoke()
        if (chevron) {
            Box(Modifier.graphicsLayer { scaleX = if (rtl) -1f else 1f }) { Glyph(Glyphs.chevron, ink.muted, 18.dp) }
        }
    }
}

private fun shortUrl(url: String) = url.removePrefix("https://").removePrefix("http://")

@Composable
private fun Bar(
    title: String,
    ink: Ink,
    onBack: () -> Unit,
    end: @Composable () -> Unit = {},
) {
    val rtl = LocalLayoutDirection.current == LayoutDirection.Rtl
    Row(Modifier.fillMaxWidth().height(56.dp).padding(start = 4.dp, end = 8.dp), verticalAlignment = Alignment.CenterVertically) {
        IconButton(stringResource(R.string.back), onBack, 48.dp) { BackMark(ink.text, rtl) }
        Text(
            title,
            Modifier.weight(1f).padding(horizontal = 4.dp),
            color = ink.text,
            fontSize = 18.sp,
            fontWeight = FontWeight.SemiBold,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
        end()
    }
}

/** An editor's bar: back leaves without saving, and the check saves, turning while it does. */
@Composable
private fun EditorBar(
    title: String,
    ink: Ink,
    saving: Boolean,
    onBack: () -> Unit,
    onSave: () -> Unit,
) = Bar(title, ink, onBack) {
    if (saving) {
        Box(Modifier.size(40.dp), contentAlignment = Alignment.Center) {
            CircularProgressIndicator(Modifier.size(20.dp), color = accent, strokeWidth = 2.dp, strokeCap = StrokeCap.Round)
        }
    } else {
        IconButton(stringResource(R.string.profile_save), onSave) { Glyph(Glyphs.check, accent, width = 2.4f) }
    }
}

/** Closes an editor once the save it sent lands. */
@Composable
private fun ClosesWhenSaved(
    profiles: Profiles,
    waitFor: Int?,
    close: () -> Unit,
) {
    val saves = profiles.saves.value
    LaunchedEffect(saves, waitFor) {
        if (waitFor != null && saves >= waitFor) close()
    }
}

@Composable
private fun EditorPage(content: @Composable ColumnScope.() -> Unit) {
    Column(
        Modifier
            .fillMaxSize()
            .windowInsetsPadding(WindowInsets.statusBars)
            .windowInsetsPadding(WindowInsets.ime.union(WindowInsets.navigationBars)),
    ) { content() }
}

@Composable
private fun Label(
    text: String,
    ink: Ink,
    top: Dp = 10.dp,
) = Text(text, Modifier.padding(top = top, bottom = 6.dp), color = ink.muted, fontSize = 13.sp)

@Composable
private fun Hint(
    text: String,
    ink: Ink,
    top: Dp = 6.dp,
) = Text(text, Modifier.padding(top = top), color = ink.muted, fontSize = 13.sp)

/** What is wrong, in red at the start, and a count at the end. */
@Composable
private fun Under(
    error: String?,
    count: String?,
    ink: Ink,
) {
    if (error == null && count == null) return
    Row(Modifier.fillMaxWidth().padding(top = 6.dp)) {
        Text(error.orEmpty(), Modifier.weight(1f), color = red, fontSize = 13.sp)
        if (count != null) Text(count, color = ink.muted, fontSize = 13.sp)
    }
}

private fun chars(text: String) = text.codePointCount(0, text.length)

@Composable
private fun NameEditor(
    profiles: Profiles,
    current: String,
    ink: Ink,
    close: () -> Unit,
) {
    var text by remember { mutableStateOf(current) }
    var empty by remember { mutableStateOf(false) }
    var waitFor by remember { mutableStateOf<Int?>(null) }
    val focus = remember { FocusRequester() }
    ClosesWhenSaved(profiles, waitFor, close)
    LaunchedEffect(Unit) { runCatching { focus.requestFocus() } }
    val saving = profiles.saving.value
    val refused = profiles.refusal.value?.takeIf { waitFor != null && !saving }?.message
    EditorPage {
        EditorBar(stringResource(R.string.profile_name), ink, saving, close) {
            val trimmed = text.trim()
            when {
                trimmed.isEmpty() -> empty = true
                trimmed == current -> close()
                else -> {
                    waitFor = profiles.saves.value + 1
                    profiles.save(Command.SetNames(display_name = trimmed))
                }
            }
        }
        Column(Modifier.padding(horizontal = 16.dp, vertical = 8.dp)) {
            Label(stringResource(R.string.profile_display_name), ink)
            Field(
                text,
                {
                    if (chars(it.trim()) <= NAME_CHARS) {
                        text = it
                        empty = false
                    }
                },
                ink,
                bad = empty || refused != null,
                focus = focus,
                capitalize = true,
            )
            Under(
                if (empty) stringResource(R.string.profile_name_empty) else refused,
                "${chars(text.trim())}/$NAME_CHARS",
                ink,
            )
            Hint(stringResource(R.string.profile_name_note), ink, 14.dp)
        }
    }
}

@Composable
private fun UsernameEditor(
    profiles: Profiles,
    current: String,
    ink: Ink,
    close: () -> Unit,
) {
    var name by remember { mutableStateOf(current) }
    var password by remember { mutableStateOf("") }
    var noPassword by remember { mutableStateOf(false) }
    var waitFor by remember { mutableStateOf<Int?>(null) }
    val focus = remember { FocusRequester() }
    ClosesWhenSaved(profiles, waitFor, close)
    LaunchedEffect(Unit) { runCatching { focus.requestFocus() } }
    val saving = profiles.saving.value
    val refusal = profiles.refusal.value?.takeIf { waitFor != null && !saving }
    val wrongPassword = refusal?.code == "bad_password"
    val nameError =
        when {
            refusal == null || wrongPassword -> null
            refusal.code == "username_taken" -> stringResource(R.string.profile_username_taken)
            else -> refusal.message
        }
    val passwordError =
        when {
            noPassword -> stringResource(R.string.setup_need_password)
            wrongPassword -> stringResource(R.string.profile_password_wrong)
            else -> null
        }
    EditorPage {
        EditorBar(stringResource(R.string.username), ink, saving, close) {
            val wanted = name.trim()
            when {
                wanted == current -> close()
                password.isEmpty() -> noPassword = true
                else -> {
                    waitFor = profiles.saves.value + 1
                    profiles.save(Command.SetNames(username = wanted, password = password))
                }
            }
        }
        Column(Modifier.padding(horizontal = 16.dp, vertical = 8.dp)) {
            Label(stringResource(R.string.username), ink)
            Field(
                name,
                {
                    name = it.lowercase()
                    waitFor = null
                },
                ink,
                bad = nameError != null,
                prefix = "@",
                focus = focus,
                ltr = true,
                keyboard = KeyboardType.Ascii,
            )
            if (nameError != null) Under(nameError, null, ink) else Hint(stringResource(R.string.profile_username_rules), ink)
            Label(stringResource(R.string.password), ink, 18.dp)
            Field(
                password,
                {
                    password = it
                    noPassword = false
                    if (wrongPassword) waitFor = null
                },
                ink,
                bad = passwordError != null,
                ltr = true,
                keyboard = KeyboardType.Password,
                hidden = true,
            )
            Under(passwordError, null, ink)
            Hint(stringResource(R.string.profile_username_note), ink, 14.dp)
        }
    }
}

@Composable
private fun BioEditor(
    profiles: Profiles,
    profile: ShownProfile?,
    ink: Ink,
    close: () -> Unit,
) {
    val current = profile?.bio.orEmpty()
    var text by remember { mutableStateOf(current) }
    var waitFor by remember { mutableStateOf<Int?>(null) }
    val focus = remember { FocusRequester() }
    ClosesWhenSaved(profiles, waitFor, close)
    LaunchedEffect(Unit) { runCatching { focus.requestFocus() } }
    val saving = profiles.saving.value
    val refused = profiles.refusal.value?.takeIf { waitFor != null && !saving }?.message
    EditorPage {
        EditorBar(stringResource(R.string.profile_bio), ink, saving, close) {
            if (text.trim() == current.trim()) {
                close()
            } else {
                waitFor = profiles.saves.value + 1
                profiles.save(Command.SaveProfile(text.trim().ifEmpty { null }, profile?.link_list.orEmpty()))
            }
        }
        Column(Modifier.padding(horizontal = 16.dp, vertical = 8.dp)) {
            Label(stringResource(R.string.profile_bio), ink)
            Field(
                text,
                { if (chars(it) <= BIO_CHARS) text = it },
                ink,
                bad = refused != null,
                focus = focus,
                tall = true,
                capitalize = true,
            )
            Under(refused, "${chars(text)}/$BIO_CHARS", ink)
            Hint(stringResource(R.string.profile_bio_note), ink, 14.dp)
        }
    }
}

@Composable
private fun LinksEditor(
    profiles: Profiles,
    profile: ShownProfile?,
    ink: Ink,
    close: () -> Unit,
) {
    val current = profile?.link_list.orEmpty()
    val rows = remember { mutableStateListOf<ProfileLink>().apply { addAll(current.ifEmpty { listOf(ProfileLink("", "")) }) } }
    val wrong = remember { mutableStateMapOf<Int, Int>() }
    var waitFor by remember { mutableStateOf<Int?>(null) }
    val focus = remember { FocusRequester() }
    ClosesWhenSaved(profiles, waitFor, close)
    LaunchedEffect(Unit) { if (current.isEmpty()) runCatching { focus.requestFocus() } }
    val saving = profiles.saving.value
    val refused = profiles.refusal.value?.takeIf { waitFor != null && !saving }?.message
    EditorPage {
        EditorBar(stringResource(R.string.profile_links), ink, saving, close) {
            wrong.clear()
            val kept = rows.map { ProfileLink(it.name.trim(), it.url.trim()) }
            kept.forEachIndexed { at, link ->
                val lower = link.url.lowercase()
                val rest = lower.removePrefix("https://").takeIf { it != lower } ?: lower.removePrefix("http://").takeIf { it != lower }
                when {
                    link.name.isEmpty() && link.url.isEmpty() -> Unit
                    link.name.isEmpty() -> wrong[at] = R.string.profile_link_no_name
                    rest.isNullOrEmpty() -> wrong[at] = R.string.profile_link_bad_address
                }
            }
            val links = kept.filter { it.name.isNotEmpty() || it.url.isNotEmpty() }
            when {
                wrong.isNotEmpty() -> Unit
                links == current -> close()
                else -> {
                    waitFor = profiles.saves.value + 1
                    profiles.save(Command.SaveProfile(profile?.bio, links))
                }
            }
        }
        Column(Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(horizontal = 16.dp, vertical = 8.dp)) {
            Label(stringResource(R.string.profile_links_label), ink)
            rows.forEachIndexed { at, link ->
                Row(Modifier.fillMaxWidth().padding(bottom = 12.dp), verticalAlignment = Alignment.Top) {
                    Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                        Field(
                            link.name,
                            {
                                if (chars(it) <= LINK_NAME_CHARS) rows[at] = link.copy(name = it)
                                wrong.remove(at)
                            },
                            ink,
                            bad = wrong[at] == R.string.profile_link_no_name,
                            placeholder = stringResource(R.string.profile_link_name),
                            focus = if (at == 0) focus else null,
                            capitalize = true,
                            short = true,
                        )
                        Field(
                            link.url,
                            {
                                if (chars(it) <= LINK_URL_CHARS) rows[at] = link.copy(url = it.trim())
                                wrong.remove(at)
                            },
                            ink,
                            bad = wrong[at] == R.string.profile_link_bad_address,
                            placeholder = stringResource(R.string.profile_link_address),
                            ltr = true,
                            keyboard = KeyboardType.Uri,
                            short = true,
                        )
                        wrong[at]?.let { Under(stringResource(it), null, ink) }
                    }
                    Box(Modifier.padding(start = 8.dp, top = 2.dp)) {
                        IconButton(stringResource(R.string.profile_remove_link), {
                            rows.removeAt(at)
                            wrong.clear()
                        }) { Cross(ink.muted) }
                    }
                }
            }
            if (rows.size < LINKS) {
                Row(
                    Modifier
                        .clip(RoundedCornerShape(8.dp))
                        .clickable(role = Role.Button) { rows.add(ProfileLink("", "")) }
                        .padding(vertical = 6.dp, horizontal = 2.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    Glyph(Glyphs.plus, accent, 20.dp)
                    Text(stringResource(R.string.profile_link_add), color = accent, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
                }
            } else {
                Hint(stringResource(R.string.profile_links_full), ink, 0.dp)
            }
            Under(refused, null, ink)
        }
    }
}

/** A box as the design draws it: a rounded border, blue while typing in it and red when wrong. */
@Composable
private fun Field(
    value: String,
    onValue: (String) -> Unit,
    ink: Ink,
    bad: Boolean = false,
    prefix: String? = null,
    placeholder: String? = null,
    focus: FocusRequester? = null,
    tall: Boolean = false,
    short: Boolean = false,
    ltr: Boolean = false,
    hidden: Boolean = false,
    capitalize: Boolean = false,
    keyboard: KeyboardType = KeyboardType.Text,
) {
    var focused by remember { mutableStateOf(false) }
    val edge =
        when {
            bad -> red
            focused -> accent
            else -> ink.line
        }
    val shape = RoundedCornerShape(12.dp)
    val style =
        TextStyle(
            color = ink.text,
            fontSize = if (short) 15.sp else 16.sp,
            textDirection = if (ltr) TextDirection.Ltr else TextDirection.Content,
        )
    BasicTextField(
        value = value,
        onValueChange = onValue,
        modifier =
            Modifier
                .fillMaxWidth()
                .then(if (focus != null) Modifier.focusRequester(focus) else Modifier)
                .onFocusChanged { focused = it.isFocused },
        textStyle = style,
        singleLine = !tall,
        visualTransformation = if (hidden) PasswordVisualTransformation() else VisualTransformation.None,
        keyboardOptions =
            KeyboardOptions(
                capitalization = if (capitalize) KeyboardCapitalization.Sentences else KeyboardCapitalization.None,
                autoCorrectEnabled = capitalize,
                keyboardType = keyboard,
            ),
        decorationBox = { inner ->
            Row(
                Modifier
                    .fillMaxWidth()
                    .heightIn(
                        min =
                            if (tall) {
                                120.dp
                            } else if (short) {
                                42.dp
                            } else {
                                48.dp
                            },
                    )
                    .border(if (bad || focused) 2.dp else 1.dp, edge, shape)
                    .background(ink.field, shape)
                    .padding(horizontal = if (short) 12.dp else 14.dp, vertical = if (short) 9.dp else 12.dp),
                verticalAlignment = if (tall) Alignment.Top else Alignment.CenterVertically,
            ) {
                if (prefix != null) Text(prefix, color = ink.muted, style = style)
                Box(Modifier.weight(1f)) {
                    if (value.isEmpty() && placeholder != null) Text(placeholder, color = ink.muted, style = style)
                    inner()
                }
            }
        },
    )
}

@Composable
private fun PhotoSheet(
    hasPhoto: Boolean,
    ink: Ink,
    onClose: () -> Unit,
    onChoose: () -> Unit,
    onRemove: () -> Unit,
) {
    val dark = isDark()
    val scrim = remember { MutableInteractionSource() }
    Box(Modifier.fillMaxSize().background(Color(0x6B000000)).clickable(scrim, null, onClick = onClose))
    Box(Modifier.fillMaxSize(), contentAlignment = Alignment.BottomCenter) {
        Column(
            Modifier
                .fillMaxWidth()
                .clip(RoundedCornerShape(topStart = 20.dp, topEnd = 20.dp))
                .background(if (dark) Color(0xFF1E2833) else Color.White)
                .windowInsetsPadding(WindowInsets.navigationBars)
                .padding(top = 8.dp, bottom = 22.dp),
        ) {
            Box(
                Modifier
                    .padding(top = 4.dp, bottom = 12.dp)
                    .size(36.dp, 4.dp)
                    .clip(RoundedCornerShape(2.dp))
                    .background(if (dark) Color(0xFF3A4656) else Color(0xFFD3D8E0))
                    .align(Alignment.CenterHorizontally),
            )
            SheetOption(Glyphs.image, stringResource(R.string.profile_choose_photo), ink.text, onChoose)
            if (hasPhoto) SheetOption(Glyphs.trash, stringResource(R.string.profile_remove_photo), red, onRemove)
        }
    }
}

@Composable
private fun SheetOption(
    icon: List<String>,
    label: String,
    color: Color,
    onClick: () -> Unit,
) {
    Row(
        Modifier.fillMaxWidth().clickable(role = Role.Button, onClick = onClick).padding(horizontal = 22.dp, vertical = 13.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        Glyph(icon, color)
        Text(label, color = color, fontSize = 16.sp)
    }
}

/** A picked picture, turned upright by its EXIF, at most 2048 pixels on its longer side. */
internal fun forCrop(
    context: Context,
    uri: Uri,
): Bitmap? =
    try {
        ImageDecoder.decodeBitmap(ImageDecoder.createSource(context.contentResolver, uri)) { decoder, info, _ ->
            decoder.allocator = ImageDecoder.ALLOCATOR_SOFTWARE
            val longer = max(info.size.width, info.size.height)
            if (longer > CROP_SOURCE) {
                decoder.setTargetSize(
                    max(1, info.size.width * CROP_SOURCE / longer),
                    max(1, info.size.height * CROP_SOURCE / longer),
                )
            }
        }
    } catch (_: Exception) {
        null
    }

/** The square as a JPEG under the limit, and its pixels small for the BlurHash. */
internal fun encode(square: Bitmap): Pair<ByteArray?, Thumbnail> {
    val thumbnail = thumbnailOf(Bitmap.createScaledBitmap(square, 32, 32, true))
    for (quality in listOf(90, 80, 70, 60)) {
        val out = ByteArrayOutputStream()
        square.compress(Bitmap.CompressFormat.JPEG, quality, out)
        if (out.size() <= PHOTO_BYTES) return out.toByteArray() to thumbnail
    }
    return null to thumbnail
}

/**
 * Picks the square: drag to move, pinch to zoom. The picture always covers the square, so it
 * cannot be moved or zoomed out past the square's edges. Done hands back the square at 640x640.
 */
@Composable
internal fun Crop(
    source: Bitmap,
    onCancel: () -> Unit,
    onDone: (Bitmap) -> Unit,
) {
    Bars(Color.Black, true)
    val image = remember(source) { source.asImageBitmap() }
    val margin = with(LocalDensity.current) { 20.dp.toPx() }
    var stage by remember { mutableStateOf(IntSize.Zero) }
    var zoom by remember { mutableFloatStateOf(1f) }
    var at by remember { mutableStateOf(Offset.Unspecified) }
    val side = (min(stage.width, stage.height) - 2 * margin).coerceAtLeast(1f)
    val square = Rect(Offset((stage.width - side) / 2f, (stage.height - side) / 2f), androidx.compose.ui.geometry.Size(side, side))
    val base = max(side / source.width, side / source.height)

    fun held(
        offset: Offset,
        scale: Float,
    ): Offset {
        val width = source.width * base * scale
        val height = source.height * base * scale
        return Offset(
            offset.x.coerceIn(square.right - width, square.left),
            offset.y.coerceIn(square.bottom - height, square.top),
        )
    }
    if (stage != IntSize.Zero && at == Offset.Unspecified) {
        at = Offset(square.center.x - source.width * base / 2f, square.center.y - source.height * base / 2f)
    }
    val rtl = LocalLayoutDirection.current == LayoutDirection.Rtl
    Column(
        Modifier
            .fillMaxSize()
            .background(Color.Black)
            .windowInsetsPadding(WindowInsets.statusBars)
            .windowInsetsPadding(WindowInsets.navigationBars),
    ) {
        Row(Modifier.fillMaxWidth().height(56.dp).padding(start = 4.dp, end = 8.dp), verticalAlignment = Alignment.CenterVertically) {
            IconButton(stringResource(R.string.back), onCancel, 48.dp) { BackMark(Color.White, rtl) }
            Text(
                stringResource(R.string.profile_move_zoom),
                Modifier.padding(horizontal = 4.dp),
                color = Color.White,
                fontSize = 18.sp,
                fontWeight = FontWeight.SemiBold,
            )
        }
        Box(
            Modifier
                .weight(1f)
                .fillMaxWidth()
                .clipToBounds()
                .onSizeChanged {
                    stage = it
                    at = Offset.Unspecified
                    zoom = 1f
                }.pointerInput(stage) {
                    detectTransformGestures { centroid, pan, gesture, _ ->
                        val next = (zoom * gesture).coerceIn(1f, MAX_ZOOM)
                        val grown = next / zoom
                        val moved = Offset(centroid.x - (centroid.x - at.x) * grown, centroid.y - (centroid.y - at.y) * grown) + pan
                        zoom = next
                        at = held(moved, next)
                    }
                },
        ) {
            Canvas(Modifier.fillMaxSize()) {
                if (at != Offset.Unspecified) {
                    drawImage(
                        image,
                        dstOffset = IntOffset(at.x.roundToInt(), at.y.roundToInt()),
                        dstSize = IntSize((source.width * base * zoom).roundToInt(), (source.height * base * zoom).roundToInt()),
                    )
                }
                val shade =
                    Path().apply {
                        fillType = PathFillType.EvenOdd
                        addRect(Rect(Offset.Zero, size))
                        addRect(square)
                    }
                drawPath(shade, Color(0x8C000000))
                drawRect(Color.White, square.topLeft, square.size, style = Stroke(1.5.dp.toPx()))
                drawCircle(
                    Color(0x80FFFFFF),
                    side / 2f,
                    square.center,
                    style = Stroke(1.dp.toPx(), pathEffect = PathEffect.dashPathEffect(floatArrayOf(4.dp.toPx(), 5.dp.toPx()))),
                )
            }
        }
        Row(Modifier.fillMaxWidth().padding(start = 10.dp, end = 10.dp, top = 6.dp, bottom = 14.dp)) {
            Text(
                stringResource(R.string.chat_cancel),
                Modifier.clip(RoundedCornerShape(8.dp)).clickable(role = Role.Button, onClick = onCancel).padding(12.dp),
                color = Color.White,
                fontSize = 16.sp,
                fontWeight = FontWeight.SemiBold,
            )
            Spacer(Modifier.weight(1f))
            Text(
                stringResource(R.string.profile_done),
                Modifier
                    .clip(RoundedCornerShape(8.dp))
                    .clickable(role = Role.Button) {
                        if (at != Offset.Unspecified) onDone(cut(source, square, at, base * zoom))
                    }.padding(12.dp),
                color = Color(0xFF8FB2E8),
                fontSize = 16.sp,
                fontWeight = FontWeight.SemiBold,
            )
        }
    }
}

/** The part of the picture under the square, drawn into a new 640x640 bitmap. */
private fun cut(
    source: Bitmap,
    square: Rect,
    at: Offset,
    scale: Float,
): Bitmap {
    val left = ((square.left - at.x) / scale).roundToInt().coerceIn(0, source.width - 1)
    val top = ((square.top - at.y) / scale).roundToInt().coerceIn(0, source.height - 1)
    val side = (square.width / scale).roundToInt().coerceIn(1, min(source.width - left, source.height - top))
    val out = Bitmap.createBitmap(PHOTO_SIDE, PHOTO_SIDE, Bitmap.Config.ARGB_8888)
    Board(
        out,
    ).drawBitmap(source, Area(left, top, left + side, top + side), Area(0, 0, PHOTO_SIDE, PHOTO_SIDE), Paint(Paint.FILTER_BITMAP_FLAG))
    return out
}

/** Line icons from the design, as SVG path data on a 24-unit grid. */
internal object Glyphs {
    private fun circle(
        x: Float,
        y: Float,
        r: Float,
    ) = "M${x - r} ${y}a$r $r 0 1 0 ${2 * r} 0a$r $r 0 1 0 ${-2 * r} 0"

    private fun box(
        x: Float,
        y: Float,
        w: Float,
        h: Float,
        r: Float,
    ) = "M${x + r} ${y}h${w - 2 * r}a$r $r 0 0 1 $r ${r}v${h - 2 * r}a$r $r 0 0 1 ${-r} ${r}h${-(w - 2 * r)}" +
        "a$r $r 0 0 1 ${-r} ${-r}v${-(h - 2 * r)}a$r $r 0 0 1 $r ${-r}z"

    val check = listOf("M5 12.5l4.5 4.5L19 7")
    val cpu = listOf("M6 6h12v12H6zM9 9h6v6H9z", "M9 3v3m6-3v3m6 3h-3m3 6h-3m-3 6v-3m-6 3v-3m-6-3h3m-3-6h3")
    val bolt = listOf("M13 2L4 14h7l-1 8 10-12h-7z")
    val cam = listOf("M4 8h3l2-3h6l2 3h3v11H4z", circle(12f, 13f, 3.5f))
    val person = listOf(circle(12f, 8f, 4f), "M4 20c1.5-4 4.5-6 8-6s6.5 2 8 6")
    val at = listOf(circle(12f, 12f, 4f), "M16 12v1.5a2.5 2.5 0 0 0 5 0V12a9 9 0 1 0-4 7.5")
    val info = listOf(circle(12f, 12f, 9f), "M12 11v6M12 7.5v.5")
    val link = listOf("M10 14a4 4 0 0 0 5.7 0l3-3a4 4 0 0 0-5.7-5.7l-1 1", "M14 10a4 4 0 0 0-5.7 0l-3 3a4 4 0 0 0 5.7 5.7l1-1")
    val devices = listOf(box(3f, 5f, 13f, 10f, 1.5f), "M7 19h5", box(17f, 9f, 5f, 10f, 1f))
    val out = listOf("M14 5h4a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2h-4", "M10 8l-4 4 4 4M6 12h10")
    val image = listOf(box(3f, 4f, 18f, 16f, 2f), circle(9f, 10f, 2f), "M21 16l-5-5-9 9")
    val trash = listOf("M4 7h16M9 7V4h6v3M6 7l1 13h10l1-13")
    val plus = listOf("M12 5v14M5 12h14")
    val moon = listOf("M20 14.5A8 8 0 1 1 9.5 4a6.5 6.5 0 0 0 10.5 10.5z")
    val pin = listOf("M9 4h6l-1 6 3 3H7l3-3z", "M12 13v7")
    val unpin = listOf("M9 4h6l-1 6 3 3H7l3-3z", "M12 13v7", "M4 4l16 16")
    val handle = listOf("M5 9h14M5 12h14M5 15h14")
    private const val FOLDER = "M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"
    val folder = listOf(FOLDER)
    val folderPlus = listOf(FOLDER, "M12 10.5v5M9.5 13h5")
    val move = listOf(FOLDER, "M9 13h6M13 11l2 2-2 2")
    val folderOut = listOf(FOLDER, "M9.5 13h5")
    val copy = listOf(box(8f, 8f, 12f, 12f, 2f), "M16 8V6a2 2 0 0 0-2-2H6a2 2 0 0 0-2 2v8a2 2 0 0 0 2 2h2")
    val star = listOf("M12 4l2.4 5 5.6.8-4 3.9.9 5.5L12 16.6 7.1 19.2 8 13.7 4 9.8l5.6-.8z")
    val edit = listOf("M4 20h4L19 9l-4-4L4 16z")
    val clock = listOf(circle(12f, 12f, 9f), "M12 7v5l3 2")
    val cross = listOf("M7 7l10 10M17 7L7 17")
    val alert = listOf(circle(12f, 12f, 9f), "M12 7.5v5", "M12 16.2v.3")
    val eye = listOf("M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7S2 12 2 12z", circle(12f, 12f, 3f))
    val plug = listOf("M5 9a10 10 0 0 1 14 0", "M8 12.5a5.5 5.5 0 0 1 8 0", circle(12f, 16.5f, 1.3f))
    val globe = listOf(circle(12f, 12f, 9f), "M3 12h18", "M12 3a14 14 0 0 1 0 18a14 14 0 0 1 0-18")
    val chevron = listOf("M9 6l6 6-6 6")
    val chat = listOf("M4 5h16v11H9l-5 4z")
    val phone = listOf("M5 4h4l2 5-2.5 1.5a11 11 0 0 0 5 5L15 13l5 2v4a2 2 0 0 1-2 2A16 16 0 0 1 3 6a2 2 0 0 1 2-2z")
    val video = listOf(box(3f, 6f, 12f, 12f, 2f), "M15 10l6-3v10l-6-3z")
}

@Composable
internal fun Glyph(
    paths: List<String>,
    color: Color,
    size: Dp = 22.dp,
    width: Float = 2f,
    filled: Boolean = false,
) {
    val parsed = remember(paths) { paths.map { PathParser().parsePathString(it).toPath() } }
    Canvas(Modifier.size(size)) {
        scale(this.size.width / 24f, pivot = Offset.Zero) {
            for (path in parsed) {
                if (filled) drawPath(path, color)
                drawPath(path, color, style = Stroke(width, cap = StrokeCap.Round, join = StrokeJoin.Round))
            }
        }
    }
}

/** Each state's dot, from `tmp/design/presence.html`: green online, red in a call, orange away. */
internal fun presenceTone(state: String?): Color? =
    when (state) {
        "online" -> Color(0xFF2EAD5B)
        "call" -> Color(0xFFD94040)
        "away" -> Color(0xFFE8912D)
        "trouble" -> Color(0xFFF1BF40)
        else -> null
    }

internal fun troubleTone(dark: Boolean): Color = if (dark) Color(0xFFFFDF81) else Color(0xFF8C6115)

/**
 * The dot on a person's photo, with a thin ring of the background's colour: 9 dp on the list's 48 dp
 * photo, 8 dp on the header's 40 dp and 14 dp on the profile's, chosen by the author. Nothing while
 * offline or hidden.
 */
@Composable
internal fun PresenceDot(
    state: String?,
    dot: Dp,
    ring: Dp,
    page: Color,
    modifier: Modifier = Modifier,
) {
    val tone = presenceTone(state) ?: return
    val trouble = state == "trouble"
    val label = if (trouble) stringResource(R.string.agent_trouble) else null
    Box(
        modifier
            .size(dot + ring * 2)
            .clip(CircleShape)
            .background(page)
            .padding(ring)
            .clip(CircleShape)
            .background(tone)
            .then(if (label != null) Modifier.semantics { contentDescription = label } else Modifier),
        contentAlignment = Alignment.Center,
    ) {
        if (trouble) Glyph(listOf("M12 5v8M12 18v.5"), Color(0xFF362500), dot, width = 3f)
    }
}

/** The clock, read again every 30 seconds, so "4m ago" moves on. */
@Composable
internal fun rememberNow(): Long {
    val now by produceState(System.currentTimeMillis()) {
        while (true) {
            delay(30_000)
            value = System.currentTimeMillis()
        }
    }
    return now
}

private fun timeText(at: Long): String =
    DateTimeFormatter.ofLocalizedTime(FormatStyle.SHORT).format(Instant.ofEpochMilli(at).atZone(ZoneId.systemDefault()))

private fun dayOf(at: Long): LocalDate = Instant.ofEpochMilli(at).atZone(ZoneId.systemDefault()).toLocalDate()

/** "3 Nov", with the year only when it is not this one. */
private fun dateText(at: Long): String {
    val day = dayOf(at)
    val pattern = if (day.year == LocalDate.now().year) "d MMM" else "d MMM yyyy"
    return DateTimeFormatter.ofPattern(pattern).format(day)
}

/**
 * A direct chat's header line: the state's word, or offline "active" and when they were last in the
 * app, chosen by the author: minutes for the first hour, then the time, yesterday and the time, or
 * the date and time. Null when nothing may be shown, and the @username stays.
 */
@Composable
internal fun presenceLine(item: PresenceItem?): String? {
    item ?: return null
    when (item.state) {
        "online" -> return stringResource(R.string.presence_online)
        "call" -> return stringResource(R.string.presence_call)
        "away" -> return stringResource(R.string.presence_away)
        "trouble" -> return stringResource(R.string.agent_trouble)
    }
    val at = item.in_app_at ?: return null
    val now = rememberNow()
    val minutes = (now - at) / 60_000
    val today = LocalDate.now()
    return when {
        minutes < 60 -> stringResource(R.string.presence_active_ago, minutes.coerceAtLeast(1).toInt())
        dayOf(at) == today -> stringResource(R.string.presence_active_at, timeText(at))
        dayOf(at) == today.minusDays(1) -> stringResource(R.string.presence_active_yesterday, timeText(at))
        else -> stringResource(R.string.presence_active_at, "${dateText(at)} ${timeText(at)}")
    }
}

/** The profile's state under the @username, in the dot's colour. */
@Composable
internal fun presenceState(state: String?): String? =
    when (state) {
        "online" -> stringResource(R.string.presence_state_online)
        "call" -> stringResource(R.string.presence_state_call)
        "away" -> stringResource(R.string.presence_state_away)
        "trouble" -> stringResource(R.string.agent_trouble)
        else -> null
    }

/** A profile line's full day and time, with how long ago inside the first hour. */
@Composable
internal fun presenceWhen(at: Long): String {
    val now = rememberNow()
    val today = LocalDate.now()
    val day =
        when (dayOf(at)) {
            today -> stringResource(R.string.presence_today, timeText(at))
            today.minusDays(1) -> stringResource(R.string.presence_yesterday, timeText(at))
            else -> stringResource(R.string.presence_on, dateText(at), timeText(at))
        }
    val minutes = (now - at) / 60_000
    return if (minutes < 60) stringResource(R.string.presence_ago, day, minutes.coerceAtLeast(1).toInt()) else day
}

/**
 * Offline, the times the profile's Active and Connected lines show, or null for none. Connected is left
 * out when Active shows and is the same or later, chosen by the author, since it then says nothing more.
 */
private fun offlineTimes(item: PresenceItem?): Pair<Long?, Long?>? {
    item ?: return null
    if (presenceTone(item.state) != null) return null
    val active = item.in_app_at
    val connected = item.connected_at?.takeIf { active == null || it > active }
    return if (active == null && connected == null) null else active to connected
}

/** A small list of choices in the middle of the screen; a tap chooses and closes it. */
@Composable
private fun Choices(
    title: String,
    options: List<Pair<String, String>>,
    chosen: String,
    ink: Ink,
    onCancel: () -> Unit,
    onChoose: (String) -> Unit,
) {
    Dialog(isDark(), onCancel) {
        Text(title, Modifier.padding(bottom = 8.dp), color = ink.text, fontSize = 17.sp, fontWeight = FontWeight.Bold)
        for ((value, label) in options) {
            Row(
                Modifier
                    .fillMaxWidth()
                    .clip(RoundedCornerShape(10.dp))
                    .clickable(role = Role.RadioButton) { onChoose(value) }
                    .padding(vertical = 4.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                RadioButton(value == chosen, { onChoose(value) }, colors = RadioButtonDefaults.colors(selectedColor = accent))
                Text(label, color = ink.text, fontSize = 16.sp)
            }
        }
    }
}
