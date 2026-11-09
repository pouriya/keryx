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
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectTransformGestures
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
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
import androidx.compose.material3.Switch
import androidx.compose.material3.SwitchDefaults
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Rect
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
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.input.VisualTransformation
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
) {
    val shown = mutableStateMapOf<String, ShownProfile>()

    /** Bumped when a photo's file is kept, so whatever draws it reads it again. */
    val kept = mutableStateMapOf<String, Int>()
    val saving = mutableStateOf(false)
    val saves = mutableStateOf(0)
    val refusal = mutableStateOf<Event.Error?>(null)

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
                kept.clear()
                me = null
                saving.value = false
                uploading.value = null
                refusal.value = null
            }
            else -> Unit
        }
    }

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

    fun open(account: String) = engine.send(Command.Profile(account))

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
) {
    val profiles = LocalProfiles.current
    val photo = account?.let { profiles?.shown?.get(it)?.photo }
    Box(modifier.size(size).clip(CircleShape).background(tile), contentAlignment = Alignment.Center) {
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
    onLogOut: () -> Unit,
    onTheme: (Boolean) -> Unit,
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
                Bars(ink.page, dark)
                Page(
                    profiles,
                    account,
                    mine,
                    profile,
                    name,
                    username,
                    ink,
                    onBack,
                    { editing = it },
                    { sheet = true },
                    { viewing = it },
                    onDevices,
                    onLogOut,
                    onTheme,
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

@Composable
private fun Page(
    profiles: Profiles,
    account: String,
    mine: Boolean,
    profile: ShownProfile?,
    name: String,
    username: String,
    ink: Ink,
    onBack: () -> Unit,
    onEdit: (Editing) -> Unit,
    onPhoto: () -> Unit,
    onFull: (String) -> Unit,
    onDevices: () -> Unit,
    onLogOut: () -> Unit,
    onTheme: (Boolean) -> Unit,
) {
    val clipboard = LocalClipboardManager.current
    val uri = LocalUriHandler.current
    val copied = stringResource(R.string.chat_copied)
    var toast by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(toast) {
        if (toast != null) {
            delay(1500)
            toast = null
        }
    }
    val band = if (isDark()) Color(0xFF0F151D) else Color(0xFFF1F3F6)
    Box(Modifier.fillMaxSize().windowInsetsPadding(WindowInsets.statusBars)) {
        Column(Modifier.fillMaxSize()) {
            Bar(if (mine) stringResource(R.string.profile_title) else "", ink, onBack)
            // Someone else's page sits in the middle of the screen while it fits, and scrolls from the top
            // once it does not, chosen by the author. The bottom padding matches the bar, so it centres on
            // the whole screen. Your own page always starts at the top.
            BoxWithConstraints(Modifier.weight(1f).windowInsetsPadding(WindowInsets.navigationBars)) {
                val room = maxHeight
                Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState())) {
                    Column(
                        if (mine) Modifier.fillMaxWidth() else Modifier.fillMaxWidth().heightIn(min = room).padding(bottom = 56.dp),
                        verticalArrangement = if (mine) Arrangement.Top else Arrangement.Center,
                    ) {
                        Column(
                            Modifier.fillMaxWidth().padding(start = 16.dp, end = 16.dp, top = 8.dp, bottom = 14.dp),
                            horizontalAlignment = Alignment.CenterHorizontally,
                        ) {
                            Head(profiles, account, mine, name, ink, onPhoto, onFull)
                            // Your own page shows the name and @username as lines below, so only others' show here.
                            if (!mine) {
                                Text(
                                    name,
                                    Modifier.padding(top = 12.dp),
                                    color = ink.text,
                                    fontSize = 22.sp,
                                    fontWeight = FontWeight.Bold,
                                )
                                if (username.isNotEmpty()) {
                                    Text(
                                        "@$username",
                                        Modifier
                                            .padding(top = 2.dp)
                                            .clip(RoundedCornerShape(6.dp))
                                            .clickable(role = Role.Button) {
                                                clipboard.setText(AnnotatedString(username))
                                                // Android 13 and later says it was copied itself.
                                                if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) toast = copied
                                            }.padding(horizontal = 4.dp),
                                        color = ink.muted,
                                        fontSize = 15.sp,
                                        style = TextStyle(textDirection = TextDirection.Ltr),
                                    )
                                }
                            }
                        }
                        val links = profile?.link_list.orEmpty()
                        if (mine) {
                            Line(Glyphs.person, name, stringResource(R.string.profile_name), null, ink) { onEdit(Editing.Name) }
                            Line(
                                Glyphs.at,
                                "@$username",
                                stringResource(R.string.username),
                                null,
                                ink,
                                ltr = true,
                            ) { onEdit(Editing.Username) }
                            Line(
                                Glyphs.info,
                                profile?.bio,
                                stringResource(R.string.profile_bio),
                                stringResource(R.string.profile_bio_empty),
                                ink,
                            ) { onEdit(Editing.Bio) }
                            if (links.isEmpty()) {
                                Line(Glyphs.link, null, null, stringResource(R.string.profile_link_add), ink) { onEdit(Editing.Links) }
                            }
                            for (link in links) {
                                Line(
                                    Glyphs.link,
                                    shortUrl(link.url),
                                    link.name,
                                    null,
                                    ink,
                                    ltr = true,
                                    accentValue = true,
                                ) { onEdit(Editing.Links) }
                            }
                            Spacer(Modifier.fillMaxWidth().height(8.dp).background(band))
                            val dark = isDark()
                            Line(Glyphs.moon, stringResource(R.string.profile_dark), null, null, ink, trailing = {
                                Switch(dark, onTheme, colors = SwitchDefaults.colors(checkedTrackColor = accent))
                            }) { onTheme(!dark) }
                            Line(Glyphs.devices, stringResource(R.string.devices), null, null, ink, onClick = onDevices)
                            Line(Glyphs.out, stringResource(R.string.log_out), null, null, ink, danger = true, onClick = onLogOut)
                        } else {
                            profile?.bio?.let { bio ->
                                Line(Glyphs.info, bio, stringResource(R.string.profile_bio), null, ink, onClick = null)
                            }
                            for (link in links) {
                                Line(Glyphs.link, shortUrl(link.url), link.name, null, ink, ltr = true, accentValue = true) {
                                    runCatching { uri.openUri(link.url) }
                                }
                            }
                        }
                    }
                }
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

/** The page's photo, 128 dp, chosen by the author. */
private val HEAD = 128.dp

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
            IconButton(stringResource(R.string.back), onBack) { BackMark(Color.White, rtl) }
        }
    }
}

/** The big photo, with the camera badge and the upload's ring on your own. */
@Composable
private fun Head(
    profiles: Profiles,
    account: String,
    mine: Boolean,
    name: String,
    ink: Ink,
    onPhoto: () -> Unit,
    onFull: (String) -> Unit,
) {
    val going = profiles.uploading.value
    // Someone else's photo opens whole; yours opens the photo sheet.
    val theirs = profiles.shown[account]?.photo?.id?.takeIf { !mine }
    val tap =
        when {
            mine -> Modifier.clickable(role = Role.Button, onClick = onPhoto)
            theirs != null -> Modifier.clickable(role = Role.Button) { onFull(theirs) }
            else -> Modifier
        }
    Box(Modifier.size(HEAD)) {
        if (mine && going != null) {
            Box(Modifier.size(HEAD).clip(CircleShape), contentAlignment = Alignment.Center) {
                Image(going, null, Modifier.fillMaxSize(), contentScale = ContentScale.Crop)
                CircularProgressIndicator(Modifier.size(HEAD - 8.dp), color = Color.White, strokeWidth = 3.dp, strokeCap = StrokeCap.Round)
            }
        } else {
            Avatar(account, name, HEAD, Modifier.clip(CircleShape).then(tap))
        }
        if (mine) {
            val label = stringResource(R.string.profile_change_photo)
            Box(
                Modifier
                    .align(Alignment.BottomEnd)
                    .offset((-2).dp, (-2).dp)
                    .size(34.dp)
                    .clip(CircleShape)
                    .background(ink.page)
                    .padding(3.dp)
                    .clip(CircleShape)
                    .background(accent)
                    .clickable(role = Role.Button, onClickLabel = label, onClick = onPhoto),
                contentAlignment = Alignment.Center,
            ) { Glyph(Glyphs.cam, Color.White, 17.dp) }
        }
    }
}

/** One line of the page: an icon, its value or what to add, and a small label under it. */
@Composable
private fun Line(
    icon: List<String>,
    value: String?,
    label: String?,
    empty: String?,
    ink: Ink,
    ltr: Boolean = false,
    accentValue: Boolean = false,
    danger: Boolean = false,
    trailing: (@Composable () -> Unit)? = null,
    onClick: (() -> Unit)?,
) {
    val tap = if (onClick != null) Modifier.clickable(role = Role.Button, onClick = onClick) else Modifier
    Row(
        Modifier.fillMaxWidth().then(tap).padding(horizontal = 16.dp, vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        Box(Modifier.width(24.dp), contentAlignment = Alignment.Center) {
            Glyph(icon, if (danger) red else ink.muted)
        }
        Column(Modifier.weight(1f)) {
            val shown = value?.takeIf { it.isNotBlank() }
            Text(
                shown ?: empty.orEmpty(),
                color =
                    when {
                        danger -> red
                        shown == null -> ink.muted
                        accentValue -> accent
                        else -> ink.text
                    },
                fontSize = 16.sp,
                style = if (ltr && shown != null) TextStyle(textDirection = TextDirection.Ltr) else TextStyle.Default,
            )
            if (label != null) Text(label, Modifier.padding(top = 1.dp), color = ink.muted, fontSize = 13.sp)
        }
        trailing?.invoke()
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
        IconButton(stringResource(R.string.back), onBack) { BackMark(ink.text, rtl) }
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
private fun forCrop(
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
private fun encode(square: Bitmap): Pair<ByteArray?, Thumbnail> {
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
private fun Crop(
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
            IconButton(stringResource(R.string.back), onCancel) { BackMark(Color.White, rtl) }
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
}

@Composable
internal fun Glyph(
    paths: List<String>,
    color: Color,
    size: Dp = 22.dp,
    width: Float = 2f,
) {
    val parsed = remember(paths) { paths.map { PathParser().parsePathString(it).toPath() } }
    Canvas(Modifier.size(size)) {
        scale(this.size.width / 24f, pivot = Offset.Zero) {
            for (path in parsed) {
                drawPath(path, color, style = Stroke(width, cap = StrokeCap.Round, join = StrokeJoin.Round))
            }
        }
    }
}
