package io.github.pouriya.keryx

import android.graphics.ImageDecoder
import android.graphics.drawable.AnimatedImageDrawable
import android.graphics.drawable.Drawable
import android.os.Build
import android.widget.ImageView
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectHorizontalDragGestures
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxHeight
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
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.GridItemSpan
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.rememberLazyGridState
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.Layout
import androidx.compose.ui.layout.MeasurePolicy
import androidx.compose.ui.layout.Placeable
import androidx.compose.ui.layout.layoutId
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.platform.LocalLayoutDirection
import androidx.compose.ui.platform.LocalSoftwareKeyboardController
import androidx.compose.ui.platform.LocalUriHandler
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.PlatformTextStyle
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextLayoutResult
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.drawText
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.text.style.LineHeightStyle
import androidx.compose.ui.text.style.ResolvedTextDirection
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.text.style.TextDirection
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.Constraints
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.IntRect
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.unit.LayoutDirection
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.compose.ui.window.Popup
import androidx.compose.ui.window.PopupPositionProvider
import androidx.compose.ui.window.PopupProperties
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.time.Instant
import java.time.LocalDate
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.time.format.FormatStyle
import kotlin.math.ceil
import kotlin.math.roundToInt

private val tile = Color(0xFF2A436B)
private val accent = Color(0xFF4576C4)
private val onAccent = Color(0xFFFFFFFF)
private val SEEN = Color(0xFF6CB0FF)
private val RED = Color(0xFFC93C3C)
private const val SWIPE_AT = 72f

/** How long typing must stop before the draft goes to the server. */
private const val DRAFT_AFTER_MS = 5000L

/** What the screen is showing in front of the thread. */
private sealed interface Modal {
    data class Options(
        val bubble: Bubble,
    ) : Modal

    data object NoKey : Modal

    /** Every emoji, in a sheet over the chat, to react to `bubble` with. */
    data class Picker(
        val bubble: Bubble,
    ) : Modal

    data class Confirm(
        val list: List<Bubble>,
    ) : Modal

    data class Times(
        val bubble: Bubble,
    ) : Modal

    /** Accepting a move would take the chat out of other folders: asked first, chosen by the author. */
    data class MoveAsk(
        val bubble: Bubble,
        val folder: String,
    ) : Modal
}

/** The thread's items, oldest first: a day's separator, then its messages. */
private sealed interface Line {
    data class Day(
        val at: Long,
    ) : Line

    data class One(
        val bubble: Bubble,
        val joined: Boolean,
        val activities: MutableList<Bubble>? = null,
        val expanded: Boolean = false,
        val firstActivityAt: Long = bubble.at,
    ) : Line {
        val shownBubble: Bubble get() = if (expanded) bubble else activities?.lastOrNull() ?: bubble

        fun holds(key: String) = bubble.key == key || activities?.any { it.key == key } == true
    }

    data class Note(
        val bubble: Bubble,
    ) : Line

    /** A membership line, a small centred pill that acts as a message. */
    data class Pill(
        val bubble: Bubble,
    ) : Line
}

/**
 * What a group's or channel's chat adds to the screen: which chat, the thread it shows (none for the
 * main list), and what its header, tabs and threads open.
 */
internal class GroupView(
    val chat: String,
    val type: Int,
    val thread: Long?,
    val onPage: () -> Unit,
    val onRequests: () -> Unit,
    val onThread: (Long) -> Unit,
    val onTyping: (Boolean) -> Unit,
    val onTopics: () -> Unit,
    val onTopicPage: () -> Unit,
)

/** Who this account is and the names it knows, for the words of membership lines. */
private val LocalWho = staticCompositionLocalOf<Pair<String?, Map<String, AccountBrief>>> { null to emptyMap() }

/** The names a message's mentions are found by, and what tapping one does: open that person's profile. */
private val LocalMention =
    staticCompositionLocalOf<
        Pair<
            Map<String, AccountBrief>,
            (
                String,
            ) -> Unit,
            >,
        > { emptyMap<String, AccountBrief>() to {} }

/** The result of asking for one message's times: `null` while it is on its way. `more` says another page waits,
 * and `asking` that it was asked for. */
internal data class TimesAnswer(
    val seq: Long,
    val member_list: List<TimesItem>?,
    val more: Boolean = false,
    val asking: Boolean = false,
)

@Composable
internal fun ChatScreen(
    talk: Talk,
    title: String,
    link: Link,
    type: Int,
    me: String?,
    people: Map<String, AccountBrief>,
    marks: List<MarkItem>,
    times: TimesAnswer?,
    onBack: () -> Unit,
    onSend: (text: String, reply: Bubble?) -> Unit,
    onEdit: (Bubble, String) -> Unit,
    onDelete: (List<Bubble>) -> Unit,
    onTimes: (Long) -> Unit,
    onMoreTimes: () -> Unit,
    onOlder: () -> Unit,
    onSeen: (Long) -> Unit,
    onDraft: (DraftText?) -> Unit,
    onFile: (FileInfo) -> Unit,
    stickers: Stickers,
    onStickers: () -> Unit,
    onSticker: (id: String, reply: Bubble?) -> Unit,
    reactions: Reactions,
    onEmoji: () -> Unit,
    onReact: (Bubble, String) -> Unit,
    onUnreact: (Reaction) -> Unit,
    onAnswer: (question: Bubble, answer: Answer, editing: Bubble?) -> Unit,
    wouldLeave: (Bubble) -> Boolean,
    onAccept: (Bubble) -> Unit,
    onDecline: (Bubble) -> Unit,
    onFolder: (String) -> Unit,
    onProfile: (String) -> Unit,
    group: GroupView? = null,
) {
    val dark = isDark()
    val ink = ink()
    val clipboard = LocalClipboardManager.current
    var draft by remember { mutableStateOf("") }
    var replying by remember { mutableStateOf<Bubble?>(null) }
    var editing by remember { mutableStateOf<Bubble?>(null) }
    var modal by remember { mutableStateOf<Modal?>(null) }
    var toast by remember { mutableStateOf<String?>(null) }
    var flash by remember { mutableStateOf<String?>(null) }
    val state = rememberLazyListState()
    val scope = rememberCoroutineScope()
    val focus = remember { FocusRequester() }
    val keyboard = LocalSoftwareKeyboardController.current
    val focusManager = LocalFocusManager.current
    val density = LocalDensity.current
    var panel by remember { mutableStateOf(false) }
    var form by remember { mutableStateOf<Form?>(null) }
    val host = LocalGroups.current
    val groups = host?.state
    val info = group?.let { groups?.info(it.chat) }
    val thread = group?.thread
    val main = thread == null
    val channel = group?.type == TYPE_CHANNEL
    val topicsOn = info?.topicsOn == true && main
    val topic = if (group != null && topicsOn) groups?.topicOpen?.get(group.chat).orEmpty() else ""
    val managing = group != null && groups?.managing(group.chat) == true
    // The topic whose settings apply here: the open one, or a thread's message's; General's while topics are off.
    val here = if (thread != null) talk.bySeq(thread)?.topic.orEmpty() else topic
    val setting = if (group != null) groups?.setting(group.chat, here) else null
    val moderating = group != null && groups?.moderating(group.chat, here, me) == true
    // A closed topic is frozen for all but its moderators; a moderators-only one keeps its main list theirs.
    val closedHere = setting?.closed == true && !moderating
    val modsOnlyHere = main && setting?.write == "moderators" && !moderating
    val mayReact =
        !closedHere &&
            (group == null || moderating || setting == null || topicMay(group.type, info?.options, setting, "message.send.reaction"))
    // A channel's main list is written by its owners and moderators; everyone writes in a thread.
    val mayWrite = (group == null || !main || groups?.mayPost(group.chat) != false) && !closedHere && !modsOnlyHere
    // A group's chat is a view on the archive: the main list or a topic, or one thread under its first message.
    val all = talk.bubbles.toList()
    val list =
        when {
            group == null -> all
            thread != null -> listOfNotNull(talk.bySeq(thread)) + all.filter { it.thread == thread }
            topicsOn -> all.filter { it.thread == null && (it.topic ?: "") == topic }
            else -> all.filter { it.thread == null }
        }

    // The sticker panel takes the keyboard's height, once the keyboard has opened.
    val imeNow = WindowInsets.ime.getBottom(density) - WindowInsets.navigationBars.getBottom(density)
    var keyboardPx by remember { mutableIntStateOf(0) }
    LaunchedEffect(imeNow) {
        if (imeNow > keyboardPx) keyboardPx = imeNow
    }
    val panelHeight = if (keyboardPx > 0) with(density) { keyboardPx.toDp() } else PANEL_HEIGHT.dp

    fun openPanel() {
        keyboard?.hide()
        focusManager.clearFocus()
        panel = true
        onStickers()
    }

    fun openForm(next: Form) {
        keyboard?.hide()
        focusManager.clearFocus()
        panel = false
        modal = null
        form = next
    }
    val copied = stringResource(R.string.chat_copied)
    val selecting = talk.selected.isNotEmpty()
    val picked = talk.picked()
    val allMine = picked.isNotEmpty() && picked.all { it.mine && it.line == null }

    fun copy(text: String) {
        clipboard.setText(AnnotatedString(text))
        // Android 13 and later says it was copied itself.
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) toast = copied
    }

    val expandedActivities = remember(talk.chat, here, thread) { mutableStateListOf<String>() }
    val expandedKeys = expandedActivities.toList()
    val rows = remember(list, expandedKeys) { rowsOf(list, expandedKeys.toSet()).asReversed() }
    val ends =
        remember(rows) {
            rows.filterIsInstance<Line.One>()
                .mapNotNull { row ->
                    val shown = row.shownBubble
                    shown.seq?.takeIf { shown.key != row.bubble.key }?.let { row.bubble.key to it }
                }
                .toMap()
        }
    val latestInRow by rememberUpdatedState(ends)
    BackHandler(enabled = true) {
        when {
            form != null -> form = null
            modal != null -> modal = null
            panel -> panel = false
            selecting -> talk.selected.clear()
            replying != null -> replying = null
            editing != null -> {
                editing = null
                draft = ""
            }
            else -> onBack()
        }
    }
    LaunchedEffect(replying, editing) {
        if (replying != null || editing != null) {
            panel = false
            runCatching { focus.requestFocus() }
        }
    }

    // A draft is kept on the server 5 seconds after typing stops, and cleared when the text goes.
    val latest by rememberUpdatedState(
        if (draft.isBlank()) {
            null
        } else {
            DraftText(draft, replying?.seq, editing?.seq)
        },
    )
    var kept by remember { mutableStateOf<DraftText?>(null) }
    LaunchedEffect(draft, replying, editing) {
        // A thread's box keeps no draft: the chat's draft is its main list's.
        if (!main) return@LaunchedEffect
        val wanted = latest
        if (wanted == kept) return@LaunchedEffect
        if (wanted != null) delay(DRAFT_AFTER_MS)
        kept = wanted
        onDraft(wanted)
    }
    DisposableEffect(Unit) {
        onDispose {
            if (main && latest != kept) onDraft(latest)
        }
    }
    // The draft the server holds for this chat comes back once, if nothing was typed meanwhile.
    LaunchedEffect(talk.restored, talk.bubbles.size) {
        if (!main) return@LaunchedEffect
        val saved = talk.restored ?: return@LaunchedEffect
        if (saved.reply != null && talk.bySeq(saved.reply) == null && talk.hasOlder) return@LaunchedEffect
        if (draft.isEmpty()) {
            draft = saved.text
            replying = saved.reply?.let { talk.bySeq(it) }
            editing = saved.edit?.let { talk.bySeq(it) }
            kept = DraftText(saved.text, saved.reply, saved.edit)
        }
        talk.taken()
    }
    LaunchedEffect(toast) {
        if (toast != null) {
            delay(1400)
            toast = null
        }
    }
    LaunchedEffect(flash) {
        if (flash != null) {
            delay(1200)
            flash = null
        }
    }
    // Typing goes out in groups and channels only, where it is shown, and stops 5 seconds after the last key.
    var typedAt by remember { mutableLongStateOf(0L) }
    LaunchedEffect(draft) {
        val typing = group?.onTyping ?: return@LaunchedEffect
        if (draft.isBlank()) {
            if (typedAt > 0) {
                typedAt = 0
                typing(false)
            }
            return@LaunchedEffect
        }
        val now = System.currentTimeMillis()
        if (now - typedAt > 3000) {
            typedAt = now
            typing(true)
        }
        delay(5000)
        typedAt = 0
        typing(false)
    }
    val newest = list.lastOrNull()
    val atBottom by remember { derivedStateOf { state.firstVisibleItemIndex == 0 && state.firstVisibleItemScrollOffset < 40 } }
    var previous by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(newest?.key) {
        val before = previous
        previous = newest?.key
        if (newest == null || before == null) return@LaunchedEffect
        // The old newest message still on screen means the reader was at the end: follow.
        val previousRow = rows.firstOrNull { it is Line.One && it.holds(before) }?.let(::rowKey) ?: before
        val wasAtEnd = state.layoutInfo.visibleItemsInfo.any { it.key == previousRow }
        if (wasAtEnd || newest.mine) state.scrollToItem(0)
    }
    // The newest message that has been on screen. The down button counts the others' messages after it,
    // so a batch counts each of its messages, and one already seen never counts.
    var lookedAt by remember(talk.chat) { mutableLongStateOf(0L) }
    LaunchedEffect(state, talk.chat) {
        snapshotFlow {
            state.layoutInfo.visibleItemsInfo.maxOfOrNull { item ->
                val key = item.key as? String
                if (key == null) 0L else latestInRow[key] ?: key.takeIf { it.startsWith("s") }?.drop(1)?.toLongOrNull() ?: 0L
            } ?: 0L
        }.collect { if (it > lookedAt) lookedAt = it }
    }
    val below = list.count { !it.mine && it.notice == null && (it.seq ?: 0L) > lookedAt }
    val lastOther = list.lastOrNull { !it.mine && it.seq != null }?.seq
    LaunchedEffect(lastOther, atBottom) {
        if (lastOther != null && atBottom) onSeen(lastOther)
    }
    val atTop by remember {
        derivedStateOf {
            val last = state.layoutInfo.visibleItemsInfo.lastOrNull()?.index ?: -1
            last >= state.layoutInfo.totalItemsCount - 3
        }
    }
    LaunchedEffect(atTop, talk.bubbles.size, talk.hasOlder) {
        if (atTop && talk.hasOlder && talk.bubbles.isNotEmpty()) onOlder()
    }

    fun jump(seq: Long) {
        val at = rows.indexOfFirst { it is Line.One && it.holds("s$seq") }
        if (at >= 0) {
            flash = "s$seq"
            talk.seeking = null
        } else if (talk.hasOlder) {
            talk.seeking = seq
            onOlder()
        }
    }
    LaunchedEffect(flash, rows) {
        val key = flash ?: return@LaunchedEffect
        val at = rows.indexOfFirst { it is Line.One && it.holds(key) }
        val matched = rows.getOrNull(at) as? Line.One ?: return@LaunchedEffect
        if (matched.shownBubble.key != key) {
            matched.activities?.let { expandedActivities.addAll(it.map(Bubble::key)) }
        } else {
            state.animateScrollToItem(at)
        }
    }
    LaunchedEffect(talk.seeking, rows, talk.hasOlder, talk.paged) {
        val seq = talk.seeking ?: return@LaunchedEffect
        // A chat opened at a message, from search, waits for its first page before looking further up.
        if (!talk.paged) return@LaunchedEffect
        if (main && group != null) {
            val activityThread = talk.bySeq(seq)?.takeIf { it.kind == CONTENT_ACTIVITY }?.thread
            if (activityThread != null) {
                group.onThread(activityThread)
                return@LaunchedEffect
            }
        }
        if (rows.any { it is Line.One && it.holds("s$seq") }) {
            flash = "s$seq"
            talk.seeking = null
        } else if (talk.hasOlder) {
            onOlder()
        } else {
            talk.seeking = null
        }
    }

    if (main) Bars(ink.page, dark)
    Box(
        Modifier
            .fillMaxSize()
            .background(ink.page)
            .then(if (main) Modifier.windowInsetsPadding(WindowInsets.statusBars) else Modifier)
            .windowInsetsPadding(WindowInsets.ime.union(WindowInsets.navigationBars)),
    ) {
        Column(Modifier.fillMaxSize()) {
            if (selecting) {
                SelectionBar(talk.selected.size, ink) { talk.selected.clear() }
            } else if (group != null && groups != null && host != null) {
                val people0 = people
                val typists = typingNow(groups, group.chat, thread, topicsOn, topic, channel)
                val typingLine = typingText(typists, people0)
                if (main && topicsOn) {
                    // A topic open: its icon and name, the group under it, and its page on a tap.
                    val name = topicName(groups, group.chat, topic)
                    GroupBar(
                        name,
                        group.chat,
                        if (link != Link.Connected) statusText(link) else typingLine ?: title,
                        typingLine != null && link == Link.Connected,
                        (info?.requestCount ?: 0) > 0 && groups.mayAdd(group.chat),
                        ink,
                        onBack,
                        group.onRequests,
                        group.onTopicPage,
                        icon = { TopicIcon(group.chat, topic, name, 44.dp) },
                    )
                } else if (main) {
                    val count = info?.memberCount
                    GroupBar(
                        title,
                        group.chat,
                        if (link != Link.Connected) statusText(link) else typingLine ?: count?.let { memberCountText(group.type, it) },
                        typingLine != null && link == Link.Connected,
                        (info?.requestCount ?: 0) > 0 && groups.mayAdd(group.chat),
                        ink,
                        onBack,
                        group.onRequests,
                        group.onPage,
                    )
                } else {
                    val total = groups.unread.replies[group.chat to thread] ?: 0
                    ThreadBar(
                        stringResource(if (channel) R.string.channel_comments_title else R.string.group_thread),
                        typingLine,
                        if (channel && total > 0) pluralStringResource(R.plurals.channel_comments, total, exact(total.toLong())) else null,
                        ink,
                        onBack,
                    )
                }
            } else {
                // A direct chat's header shows the other person, and opens their profile; Saved
                // messages shows your own photo and opens your profile, as the author chose.
                val other = if (type == 1) marks.firstOrNull { it.account != me }?.account else null
                val person = if (type == 0) me else other
                val handle = other?.let { people[it]?.username }?.let { "@$it" }
                // A direct chat's header shows the state, or when they were last in the app, chosen by the author.
                val profiles = LocalProfiles.current
                val presence = if (link == Link.Connected) other?.let { profiles?.presence?.get(it) } else null
                val detail = if (presence?.state == "trouble") other?.let { profiles?.troubleDetail(it) } else null
                val line = detail?.let { troubleLine(it) } ?: presenceLine(presence)
                TopBar(
                    title,
                    person,
                    if (link != Link.Connected) statusText(link) else line ?: handle,
                    presence?.state?.let { if (it == "trouble") troubleTone(dark) else presenceTone(it) }?.takeIf { line != null },
                    presence?.state,
                    ink,
                    onBack,
                ) {
                    person?.let(onProfile)
                }
            }
            LazyColumn(
                Modifier.weight(1f).fillMaxWidth(),
                state = state,
                reverseLayout = true,
            ) {
                itemsIndexed(rows, key = { _, row -> rowKey(row) }) { index, row ->
                    CompositionLocalProvider(LocalMention provides (people to onProfile)) {
                        when (row) {
                            is Line.Day -> DayRow(row.at, ink, dark)
                            is Line.Note -> NoteRow(row.bubble, ink, dark)
                            is Line.Pill ->
                                CompositionLocalProvider(LocalWho provides (me to people)) {
                                    PillRow(
                                        bubble = row.bubble,
                                        talk = talk,
                                        selecting = selecting,
                                        flashing = flash == row.bubble.key,
                                        ink = ink,
                                        dark = dark,
                                        replies = threadCount(group, groups, row.bubble, main),
                                        onOpen = { modal = Modal.Options(row.bubble) },
                                        onSelect = { talk.toggle(row.bubble) },
                                        onChip = { chip ->
                                            val yours = chip.mine
                                            if (mayReact) {
                                                if (yours != null) {
                                                    onUnreact(yours)
                                                } else {
                                                    onReact(row.bubble, chip.emoji)
                                                }
                                            }
                                        },
                                        onThread = { row.bubble.seq?.let { group?.onThread?.invoke(it) } },
                                    )
                                }
                            is Line.One -> {
                                val bubble = row.shownBubble
                                val activity = bubble.kind == CONTENT_ACTIVITY
                                val footer =
                                    row.activities?.takeIf { it.size > 1 }?.let { steps ->
                                        ActivityFooter(steps.size, row.expanded) {
                                            val keys = steps.map(Bubble::key)
                                            val firstVisible = state.firstVisibleItemIndex
                                            val visibleKey = state.layoutInfo.visibleItemsInfo.firstOrNull { it.index == firstVisible }?.key
                                            if (row.expanded) {
                                                expandedActivities.removeAll(keys.toSet())
                                                if (flash in keys) flash = null
                                            } else {
                                                expandedActivities.addAll(keys)
                                            }
                                            // Keep the bottom of this group in view when it anchors the chat.
                                            if (visibleKey is String && visibleKey in keys) {
                                                state.requestScrollToItem(if (row.expanded) index - steps.lastIndex else index)
                                            }
                                        }
                                    }
                                val name =
                                    if (group != null && !row.joined && !bubble.mine) {
                                        if (channel && bubble.thread == null) title else personName(bubble.account, people)
                                    } else {
                                        null
                                    }
                                val onName =
                                    bubble.account?.takeIf { !(channel && bubble.thread == null) }?.let { account ->
                                        { onProfile(account) }
                                    }
                                MessageRow(
                                    bubble = bubble,
                                    joined = row.joined,
                                    type = type,
                                    me = me,
                                    people = people,
                                    marks = marks,
                                    talk = talk,
                                    selecting = selecting,
                                    flashing = flash == bubble.key,
                                    ink = ink,
                                    dark = dark,
                                    onOpen = { modal = if (bubble.locked) Modal.NoKey else Modal.Options(bubble) },
                                    onSelect = { talk.toggle(bubble) },
                                    onCopy = ::copy,
                                    onTime = {
                                        if (bubble.seq != null) {
                                            onTimes(bubble.seq)
                                            modal = Modal.Times(bubble)
                                        }
                                    },
                                    onReply = {
                                        editing = null
                                        replying = bubble
                                    },
                                    onJump = ::jump,
                                    onFile = onFile,
                                    stickers = stickers,
                                    chips = talk.chips(bubble),
                                    onChip = { chip ->
                                        val yours = chip.mine
                                        if (mayReact) {
                                            if (yours != null) {
                                                onUnreact(yours)
                                            } else {
                                                onReact(bubble, chip.emoji)
                                            }
                                        }
                                    },
                                    onAnswer = {
                                        val tree = bubble.question
                                        if (tree != null) openForm(Form(bubble, tree, null, null))
                                    },
                                    onAccept = {
                                        val folder = bubble.suggestion?.folder
                                        if (folder != null && folder.move && wouldLeave(bubble)) {
                                            modal = Modal.MoveAsk(bubble, folder.name.trim())
                                        } else {
                                            onAccept(bubble)
                                        }
                                    },
                                    onDecline = { onDecline(bubble) },
                                    onFolder = onFolder,
                                    name = name,
                                    onName = onName,
                                    mentioned = !activity && me != null && me in bubble.mentions,
                                    replies = if (activity) null else threadCount(group, groups, bubble, main),
                                    onThread = { bubble.seq?.let { group?.onThread?.invoke(it) } },
                                    mayReply = mayWrite,
                                    activityFooter = footer,
                                    showDate = activity && dayOf(bubble.at) != dayOf(row.firstActivityAt),
                                )
                            }
                        }
                    }
                }
            }
            val note = replying ?: editing
            if (note != null) {
                NoteBar(
                    title =
                        if (editing != null) {
                            stringResource(R.string.chat_editing)
                        } else {
                            stringResource(R.string.chat_replying_to, nameOf(note, me, people))
                        },
                    line = quoteLine(note),
                    ink = ink,
                ) {
                    replying = null
                    if (editing != null) {
                        editing = null
                        draft = ""
                    }
                }
            }
            if (group != null && host != null) {
                MentionList(group.chat, draft, me, people, ink) { draft = it }
            }
            val slowLeft = slowLeft(group, groups, main, moderating)
            if (!mayWrite) {
                Box(Modifier.fillMaxWidth().height(1.dp).background(ink.line))
                Row(
                    Modifier.fillMaxWidth().padding(start = 18.dp, end = 18.dp, top = 14.dp, bottom = 18.dp),
                    horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.CenterHorizontally),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    if (closedHere) Glyph(topicLockGlyph, ink.muted, 16.dp)
                    Text(
                        stringResource(
                            when {
                                closedHere -> R.string.topic_closed_note
                                modsOnlyHere -> R.string.topic_mods_note
                                else -> R.string.channel_notice
                            },
                        ),
                        color = ink.muted,
                        fontSize = 13.5.sp,
                        textAlign = TextAlign.Center,
                    )
                }
            } else {
                Composer(
                    focus = focus,
                    draft = draft,
                    hint =
                        when {
                            slowLeft > 0 -> stringResource(R.string.group_slow_wait, slowClock(slowLeft))
                            channel && main -> stringResource(R.string.group_post_hint)
                            !main && channel -> stringResource(R.string.channel_comment_hint)
                            !main -> stringResource(R.string.group_reply_hint)
                            topicsOn ->
                                stringResource(
                                    R.string.chat_message_in,
                                    groups?.topics(group.chat)?.firstOrNull { it.id == topic }?.name
                                        ?: stringResource(R.string.topic_general),
                                )
                            else -> stringResource(R.string.chat_message_hint)
                        },
                    blocked = slowLeft > 0,
                    onDraft = { draft = it },
                    removeCount = if (allMine) picked.size else 0,
                    panel = panel,
                    onPanel = {
                        if (panel) {
                            panel = false
                            runCatching { focus.requestFocus() }
                            keyboard?.show()
                        } else {
                            openPanel()
                        }
                    },
                    onFocused = { panel = false },
                    ink = ink,
                    onSend = {
                        val text = draft.trim()
                        if (text.isNotEmpty() && slowLeft <= 0) {
                            val target = editing
                            if (target != null) onEdit(target, text) else onSend(text, replying)
                            // Slow mode counts from this send in the main list or a topic, never in a thread.
                            val slow = setting?.slow_mode ?: info?.options?.slow_mode
                            if (target == null && group != null && main && !moderating && slow != null) {
                                groups?.slowUntil?.set(group.chat, System.currentTimeMillis() + slow * 1000)
                            }
                            draft = ""
                            replying = null
                            editing = null
                        }
                    },
                    onRemove = { modal = Modal.Confirm(picked) },
                )
            }
            if (panel) {
                StickerPanel(stickers, panelHeight, ink) { id ->
                    onSticker(id, replying)
                    replying = null
                }
            }
        }
        if (!atBottom) {
            Box(
                Modifier.align(Alignment.BottomEnd).padding(end = 16.dp, bottom = 76.dp).size(44.dp),
                contentAlignment = Alignment.Center,
            ) {
                Box(
                    Modifier
                        .size(44.dp)
                        .clip(CircleShape)
                        .background(if (dark) Color(0xFF243041) else Color.White)
                        .border(1.dp, ink.line, CircleShape)
                        .clickable(role = Role.Button) { scope.launch { state.animateScrollToItem(0) } },
                    contentAlignment = Alignment.Center,
                ) { DownMark(ink.text) }
                if (below > 0) {
                    Text(
                        below.toString(),
                        Modifier
                            .align(Alignment.TopEnd)
                            .offset(x = 4.dp, y = (-6).dp)
                            .height(20.dp)
                            .widthIn(min = 20.dp)
                            .clip(RoundedCornerShape(10.dp))
                            .background(accent)
                            .padding(horizontal = 6.dp),
                        color = onAccent,
                        fontSize = 12.sp,
                        fontWeight = FontWeight.Bold,
                        textAlign = TextAlign.Center,
                    )
                }
            }
        }
        toast?.let {
            Text(
                it,
                Modifier
                    .align(Alignment.BottomCenter)
                    .padding(bottom = 84.dp)
                    .clip(RoundedCornerShape(18.dp))
                    .background(if (dark) Color(0xFFE4E8F0) else Color(0xFF222B36))
                    .padding(horizontal = 16.dp, vertical = 8.dp),
                color = if (dark) Color(0xFF121212) else Color.White,
                fontSize = 14.sp,
            )
        }
        modal?.let { shown ->
            if (shown is Modal.Picker) {
                EmojiSheet(reactions, ink, dark, onClose = { modal = null }) { emoji ->
                    modal = null
                    onReact(shown.bubble, emoji)
                }
                return@let
            }
            // The emoji row sits under a message's options, for any message that can still be sent.
            val reactable = (shown as? Modal.Options)?.bubble?.takeIf { !it.failed && mayReact }
            Overlay(
                ink,
                dark,
                onDismiss = { modal = null },
                below =
                    reactable?.let { bubble ->
                        {
                            ReactRow(
                                recent = reactions.recent,
                                ink = ink,
                                onPick = { emoji ->
                                    modal = null
                                    onReact(bubble, emoji)
                                },
                                onMore = {
                                    onEmoji()
                                    modal = Modal.Picker(bubble)
                                },
                            )
                        }
                    },
            ) {
                when (shown) {
                    is Modal.Options ->
                        CompositionLocalProvider(LocalWho provides (me to people)) {
                            OptionsModal(
                                bubble = shown.bubble,
                                name = nameOf(shown.bubble, me, people),
                                ink = ink,
                                mayReply = mayWrite,
                                onThread =
                                    shown.bubble.seq?.takeIf { group != null && main && !shown.bubble.failed }?.let { seq ->
                                        {
                                            modal = null
                                            group?.onThread?.invoke(seq)
                                        }
                                    },
                                onRemoveOthers =
                                    if (managing && shown.bubble.seq != null && (!shown.bubble.mine || shown.bubble.line != null)) {
                                        { modal = Modal.Confirm(listOf(shown.bubble)) }
                                    } else {
                                        null
                                    },
                                onReply = {
                                    modal = null
                                    editing = null
                                    replying = shown.bubble
                                },
                                onCopy = {
                                    modal = null
                                    copy(shown.bubble.text.orEmpty())
                                },
                                onEdit = {
                                    modal = null
                                    replying = null
                                    editing = shown.bubble
                                    draft = shown.bubble.text.orEmpty()
                                },
                                onDelete = { modal = Modal.Confirm(listOf(shown.bubble)) },
                                onShowAnswer =
                                    shown.bubble.seq
                                        ?.takeIf { shown.bubble.question != null }
                                        ?.let { talk.answerTo(it) }
                                        ?.seq
                                        ?.let { answerSeq ->
                                            {
                                                modal = null
                                                jump(answerSeq)
                                            }
                                        },
                                onEditAnswer =
                                    shown.bubble.answer
                                        ?.takeIf { shown.bubble.mine && shown.bubble.seq != null }
                                        ?.let { answer ->
                                            val question = talk.bySeq(answer.question)
                                            val tree = question?.question
                                            if (question != null && tree != null) {
                                                { openForm(Form(question, tree, shown.bubble, answer)) }
                                            } else {
                                                null
                                            }
                                        },
                            )
                        }
                    Modal.NoKey -> NoKeyModal(ink) { modal = null }
                    is Modal.Picker -> {}
                    is Modal.Confirm ->
                        ConfirmModal(
                            count = shown.list.size,
                            ink = ink,
                            onCancel = { modal = null },
                            onDelete = {
                                modal = null
                                talk.selected.clear()
                                onDelete(shown.list)
                            },
                        )
                    is Modal.Times ->
                        TimesModal(
                            bubble = shown.bubble,
                            answer = times?.takeIf { it.seq == shown.bubble.seq },
                            me = me,
                            people = people.takeIf { group != null },
                            ink = ink,
                            onMore = onMoreTimes,
                        ) { modal = null }
                    is Modal.MoveAsk ->
                        MoveAskModal(
                            folder = shown.folder,
                            ink = ink,
                            onCancel = { modal = null },
                            onMove = {
                                modal = null
                                onAccept(shown.bubble)
                            },
                        )
                }
            }
        }
        form?.let { open ->
            AnswerSheet(open, ink, dark, onClose = { form = null }) { answer ->
                form = null
                onAnswer(open.question, answer, open.editing)
            }
        }
    }
}

private fun rowKey(row: Line): String =
    when (row) {
        is Line.Day -> "day-${row.at}"
        is Line.One -> row.bubble.key
        is Line.Note -> row.bubble.key
        is Line.Pill -> row.bubble.key
    }

private fun dayOf(at: Long): LocalDate = Instant.ofEpochMilli(at).atZone(ZoneId.systemDefault()).toLocalDate()

private fun rowsOf(
    list: List<Bubble>,
    expanded: Set<String>,
): List<Line> {
    val out = ArrayList<Line>()
    var day: LocalDate? = null
    var before: Bubble? = null
    for (bubble in list) {
        val activity = bubble.kind == CONTENT_ACTIVITY && !bubble.locked && bubble.seq != null
        val previous = (out.lastOrNull() as? Line.One)?.activities
        if (activity && previous != null && before != null && bubble.account != null &&
            before.account == bubble.account && before.topic.orEmpty() == bubble.topic.orEmpty() && before.thread == bubble.thread
        ) {
            previous.add(bubble)
            before = bubble
            continue
        }
        val today = dayOf(bubble.at)
        if (today != day) {
            out.add(Line.Day(bubble.at))
            day = today
            before = null
        }
        if (bubble.notice != null) {
            out.add(Line.Note(bubble))
            before = null
            continue
        }
        if (bubble.line != null) {
            out.add(Line.Pill(bubble))
            before = null
            continue
        }
        // A run is one sender's messages in a row: its first shows their name in a group.
        val joined = before != null && before.mine == bubble.mine && (bubble.mine || before.account == bubble.account)
        out.add(Line.One(bubble, joined, if (activity) mutableListOf(bubble) else null))
        before = bubble
    }
    if (expanded.isEmpty()) return out
    return buildList {
        for (row in out) {
            val steps = (row as? Line.One)?.activities
            if (row is Line.One && steps != null && steps.size > 1 && steps.any { it.key in expanded }) {
                add(row.copy(expanded = true))
                for (step in steps.drop(1)) {
                    add(Line.One(step, joined = true, firstActivityAt = row.bubble.at))
                }
            } else {
                add(row)
            }
        }
    }
}

internal fun firstLine(text: String): String {
    val lines = text.lines()
    val first = lines.firstOrNull().orEmpty()
    return if (lines.size > 1) "$first…" else first
}

@Composable
private fun nameOf(
    bubble: Bubble,
    me: String?,
    people: Map<String, AccountBrief>,
): String {
    if (bubble.mine || (bubble.account != null && bubble.account == me)) return stringResource(R.string.chat_you)
    val account = bubble.account?.let { people[it] } ?: return ""
    return account.display_name?.takeIf { it.isNotBlank() } ?: account.username
}

@Composable
private fun TopBar(
    title: String,
    person: String?,
    status: String?,
    statusTone: Color?,
    state: String?,
    ink: Ink,
    onBack: () -> Unit,
    onPerson: () -> Unit,
) {
    val rtl = LocalLayoutDirection.current == LayoutDirection.Rtl
    Row(Modifier.fillMaxWidth().height(64.dp).padding(start = 4.dp, end = 8.dp), verticalAlignment = Alignment.CenterVertically) {
        IconButton(stringResource(R.string.back), onBack, 48.dp) { BackMark(ink.text, rtl) }
        Row(
            Modifier
                .weight(1f)
                .fillMaxHeight()
                .then(if (person != null) Modifier.clickable(role = Role.Button, onClick = onPerson) else Modifier),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Box(Modifier.padding(start = 2.dp, end = 10.dp)) {
                Avatar(person, title, 44.dp, badgeSize = 13.dp)
                PresenceDot(state, 9.dp, 2.dp, ink.page, Modifier.align(Alignment.BottomEnd))
            }
            Column(Modifier.weight(1f)) {
                Text(
                    title,
                    color = ink.text,
                    fontSize = 17.sp,
                    fontWeight = FontWeight.Bold,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                if (status != null) {
                    Text(
                        status,
                        color = statusTone ?: ink.muted,
                        fontSize = 13.sp,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        style = TextStyle(textDirection = TextDirection.Content),
                    )
                }
            }
        }
    }
}

@Composable
private fun troubleLine(text: String): String {
    val reason = text.substringBefore(':')
    val label =
        stringResource(
            when (reason) {
                "llm_failing" -> R.string.agent_trouble_llm
                "stuck" -> R.string.agent_trouble_stuck
                "storage_warn" -> R.string.agent_trouble_storage_warn
                "storage_full" -> R.string.agent_trouble_storage_full
                else -> R.string.agent_trouble
            },
        )
    val detail = text.substringAfter(':', "").trim()
    return if (detail.isEmpty()) label else stringResource(R.string.agent_trouble_detail, label, detail)
}

@Composable
private fun SelectionBar(
    count: Int,
    ink: Ink,
    onClose: () -> Unit,
) {
    Row(Modifier.fillMaxWidth().height(56.dp).padding(start = 4.dp, end = 8.dp), verticalAlignment = Alignment.CenterVertically) {
        IconButton(stringResource(R.string.chat_cancel), onClose) { Cross(ink.text) }
        Text(
            pluralStringResource(R.plurals.chat_selected, count, count),
            Modifier.padding(start = 6.dp),
            color = ink.text,
            fontSize = 18.sp,
            fontWeight = FontWeight.Bold,
        )
    }
}

@Composable
private fun NoteRow(
    bubble: Bubble,
    ink: Ink,
    dark: Boolean,
) {
    val time = DateTimeFormatter.ofLocalizedTime(FormatStyle.SHORT).format(Instant.ofEpochMilli(bubble.at).atZone(ZoneId.systemDefault()))
    Box(Modifier.fillMaxWidth().padding(horizontal = 24.dp, vertical = 6.dp), contentAlignment = Alignment.Center) {
        Text(
            "${bubble.notice} · $time",
            Modifier
                .clip(RoundedCornerShape(10.dp))
                .background(if (dark) Color(0xFF1E2833) else Color(0xFFF1F3F6))
                .padding(horizontal = 12.dp, vertical = 5.dp),
            color = ink.muted,
            fontSize = 13.sp,
            textAlign = TextAlign.Center,
        )
    }
}

@Composable
private fun DayRow(
    at: Long,
    ink: Ink,
    dark: Boolean,
) {
    val today = LocalDate.now()
    val day = dayOf(at)
    val label =
        when {
            day == today -> stringResource(R.string.chat_today)
            day == today.minusDays(1) -> stringResource(R.string.yesterday)
            else -> DateTimeFormatter.ofLocalizedDate(FormatStyle.MEDIUM).format(day)
        }
    Box(Modifier.fillMaxWidth().padding(vertical = 8.dp), contentAlignment = Alignment.Center) {
        Text(
            label,
            Modifier
                .clip(RoundedCornerShape(10.dp))
                .background(if (dark) Color(0xFF1E2833) else Color(0xFFF1F3F6))
                .padding(horizontal = 10.dp, vertical = 3.dp),
            color = ink.muted,
            fontSize = 12.sp,
            fontWeight = FontWeight.SemiBold,
        )
    }
}

@Composable
private fun MessageRow(
    bubble: Bubble,
    joined: Boolean,
    type: Int,
    me: String?,
    people: Map<String, AccountBrief>,
    marks: List<MarkItem>,
    talk: Talk,
    selecting: Boolean,
    flashing: Boolean,
    ink: Ink,
    dark: Boolean,
    onOpen: () -> Unit,
    onSelect: () -> Unit,
    onCopy: (String) -> Unit,
    onTime: () -> Unit,
    onReply: () -> Unit,
    onJump: (Long) -> Unit,
    onFile: (FileInfo) -> Unit,
    stickers: Stickers,
    chips: List<Chip>,
    onChip: (Chip) -> Unit,
    onAnswer: () -> Unit,
    onAccept: () -> Unit,
    onDecline: () -> Unit,
    onFolder: (String) -> Unit,
    name: String? = null,
    onName: (() -> Unit)? = null,
    mentioned: Boolean = false,
    replies: ThreadCount? = null,
    onThread: () -> Unit = {},
    mayReply: Boolean = true,
    activityFooter: ActivityFooter? = null,
    showDate: Boolean = false,
) {
    val rtl = LocalLayoutDirection.current == LayoutDirection.Rtl
    val density = LocalDensity.current
    val selected = bubble.key in talk.selected
    var drag by remember { mutableFloatStateOf(0f) }
    val limit = with(density) { SWIPE_AT.dp.toPx() }
    val tint =
        when {
            selected -> accent.copy(alpha = 0.20f)
            flashing -> accent.copy(alpha = 0.28f)
            else -> Color.Transparent
        }
    val maxWidth = (LocalConfiguration.current.screenWidthDp * 0.78f).dp
    val fill =
        if (bubble.mine) {
            tile
        } else if (mentioned) {
            // A message mentioning you is a tinted bubble, chosen by the author.
            if (dark) Color(0xFF243B5E) else Color(0xFFDCE7F7)
        } else if (dark) {
            Color(0xFF1E2833)
        } else {
            Color(0xFFF1F3F6)
        }
    val onFill = if (bubble.mine) Color.White else ink.text
    val quiet = if (bubble.mine) Color.White.copy(alpha = 0.66f) else ink.muted
    Box(
        Modifier
            .fillMaxWidth()
            .background(tint)
            .padding(top = if (joined) 3.dp else 5.dp)
            .pointerInput(bubble.key, selecting) {
                detectTapGestures(
                    onTap = { if (selecting) onSelect() else onOpen() },
                    onLongPress = { if (!selecting) onSelect() },
                )
            }.pointerInput(bubble.key, mayReply) {
                if (!mayReply) return@pointerInput
                detectHorizontalDragGestures(
                    onDragEnd = {
                        if (kotlin.math.abs(drag) >= limit) onReply()
                        drag = 0f
                    },
                    onDragCancel = { drag = 0f },
                ) { _, amount ->
                    val next = drag + amount
                    drag = if (rtl) next.coerceIn(0f, limit * 1.2f) else next.coerceIn(-limit * 1.2f, 0f)
                }
            },
    ) {
        if (drag != 0f) {
            Box(
                Modifier
                    .align(Alignment.CenterEnd)
                    .padding(end = 14.dp)
                    .size(34.dp)
                    .clip(CircleShape)
                    .background(accent.copy(alpha = 0.22f)),
                contentAlignment = Alignment.Center,
            ) { ReplyMark(accent) }
        }
        Row(
            Modifier.fillMaxWidth().padding(horizontal = 10.dp).offset { IntOffset(drag.roundToInt(), 0) },
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = if (bubble.mine) Arrangement.End else Arrangement.Start,
        ) {
            if (selecting) {
                Box(
                    Modifier
                        .padding(end = 8.dp)
                        .size(22.dp)
                        .clip(CircleShape)
                        .background(if (selected) accent else Color.Transparent)
                        .border(2.dp, if (selected) accent else ink.muted, CircleShape),
                    contentAlignment = Alignment.Center,
                ) {
                    if (selected) Checks(Color.White, two = false)
                }
                if (bubble.mine) Spacer(Modifier.weight(1f))
            }
            val sticker = bubble.sticker
            if (sticker != null && !bubble.locked) {
                StickerBubble(bubble, sticker, type, me, people, marks, talk, selecting, stickers, ink, {}, onSelect, onJump) {
                    Chips(chips, onBubble = false, mine = false, dark = dark, ink = ink, enabled = !selecting, onChip = onChip)
                }
                return@Row
            }
            val suggestion = bubble.suggestion
            if (suggestion != null && !bubble.locked) {
                SuggestionBubble(
                    bubble, suggestion, type, me, people, marks, talk, selecting, dark, ink, fill, onFill, quiet,
                    {}, onSelect, onCopy, onTime, onJump, onAccept, onDecline, onFolder,
                ) {
                    Chips(chips, onBubble = true, mine = bubble.mine, dark = dark, ink = ink, enabled = !selecting, onChip = onChip)
                }
                return@Row
            }
            val last = remember(bubble.key) { LastLine() }
            Layout(
                modifier =
                    Modifier
                        .widthIn(max = maxWidth)
                        .clip(RoundedCornerShape(16.dp))
                        .background(fill)
                        .pointerInput(bubble.key, selecting) {
                            // Only the row's empty part opens the options, chosen by the author; the bubble's
                            // own parts keep their taps, and one this device cannot open still says why.
                            detectTapGestures(
                                onTap = {
                                    if (selecting) {
                                        onSelect()
                                    } else if (bubble.locked || bubble.kind == CONTENT_ACTIVITY) {
                                        onOpen()
                                    }
                                },
                                onLongPress = { if (!selecting) onSelect() },
                            )
                        }.padding(start = 10.dp, end = 10.dp, top = 7.dp, bottom = 6.dp),
                measurePolicy = floatTime(last),
                content = {
                    if (name != null) {
                        Text(
                            name,
                            Modifier
                                .padding(bottom = 2.dp)
                                .then(
                                    if (onName != null && !selecting) {
                                        Modifier.clickable(
                                            role = Role.Button,
                                            onClick = onName,
                                        )
                                    } else {
                                        Modifier
                                    },
                                ),
                            color = nameColor(bubble.account, dark),
                            fontSize = 13.sp,
                            fontWeight = FontWeight.Bold,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                    }
                    if (bubble.reply != null || bubble.replyLocal != null) {
                        Quote(bubble, talk, me, people, bubble.mine, ink, onJump)
                    }
                    val answerable = !bubble.mine && bubble.seq != null && talk.answerTo(bubble.seq, mine = true) == null
                    val open = if (bubble.locked) onOpen else ({})
                    Column(Modifier.layoutId(TEXT_ID)) {
                        CompositionLocalProvider(LocalLastLine provides last) {
                            Content(bubble, quiet, onFill, selecting, open, onSelect, onCopy, onFile, talk, answerable, onAnswer, onJump)
                        }
                    }
                    Row(
                        Modifier.layoutId(TIME_ID),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        if (bubble.failed) {
                            Text(
                                stringResource(R.string.chat_not_sent),
                                color = Color(0xFFFF8A8A),
                                fontSize = 11.sp,
                                fontWeight = FontWeight.SemiBold,
                            )
                        } else {
                            val time = timeOf(bubble.at)
                            val stamp =
                                if (showDate) {
                                    "${DateTimeFormatter.ofLocalizedDate(FormatStyle.MEDIUM).format(dayOf(bubble.at))} $time"
                                } else {
                                    time
                                }
                            if (bubble.edited) {
                                Text(stringResource(R.string.chat_edited), Modifier.padding(end = 4.dp), color = quiet, fontSize = 11.sp)
                            }
                            Text(
                                stamp,
                                Modifier
                                    .clip(RoundedCornerShape(8.dp))
                                    .clickable(enabled = !selecting && bubble.seq != null, role = Role.Button, onClick = onTime)
                                    .padding(horizontal = 4.dp, vertical = 1.dp),
                                color = quiet,
                                fontSize = 11.sp,
                                fontWeight = FontWeight.Medium,
                            )
                            if (bubble.mine) Ticks(bubble, type, me, marks, quiet)
                        }
                    }
                    Chips(chips, onBubble = true, mine = bubble.mine, dark = dark, ink = ink, enabled = !selecting, onChip = onChip)
                    if (activityFooter != null) {
                        ActivityFoot(activityFooter, bubble.mine, onFill, !selecting)
                    } else if (replies != null) {
                        ThreadFoot(replies, bubble.mine, onFill, !selecting, onThread)
                    }
                },
            )
        }
    }
}

private const val TEXT_ID = "text"
private const val TIME_ID = "time"
private const val FOOT_ID = "foot"

private data class ActivityFooter(
    val count: Int,
    val expanded: Boolean,
    val toggle: () -> Unit,
)

@Composable
private fun ActivityFoot(
    footer: ActivityFooter,
    mine: Boolean,
    onFill: Color,
    enabled: Boolean,
) {
    val tone = if (mine) onFill else kit().accent
    val divider = if (isDark() || mine) Color.White.copy(alpha = 0.08f) else Color.Black.copy(alpha = 0.08f)
    val label = stringResource(if (footer.expanded) R.string.activity_collapse else R.string.activity_expand)
    Row(
        Modifier
            .layoutId(FOOT_ID)
            .drawBehind { drawLine(divider, Offset.Zero, Offset(size.width, 0f), 1.dp.toPx()) }
            .clickable(enabled = enabled, onClickLabel = label, role = Role.Button, onClick = footer.toggle)
            .padding(horizontal = 10.dp, vertical = 7.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Text(
            stringResource(R.string.activity_count, exact(footer.count.toLong())),
            Modifier.weight(1f),
            color = tone,
            fontSize = 13.sp,
            fontWeight = FontWeight.Bold,
            maxLines = 1,
        )
        Glyph(if (footer.expanded) GroupGlyphs.up else GroupGlyphs.down, tone, 16.dp)
    }
}

/** The width of a bubble's last text line, set as its text is laid out, or null when the time cannot share it. */
private class LastLine {
    var width: Int? = null
}

private val LocalLastLine = staticCompositionLocalOf<LastLine?> { null }

/**
 * A bubble's parts stacked as a column, except its time, which sits at the end of the last text line
 * when it fits there and on a line of its own when it does not, as `tmp/design/chat-screen.html` and
 * `tmp/design/groups.html` draw it. The time always sits at the bubble's end, however wide a name over
 * the text makes the bubble.
 */
private fun floatTime(last: LastLine) =
    MeasurePolicy { measurables, constraints ->
        val loose = constraints.copy(minWidth = 0, minHeight = 0)
        val footAt = measurables.indexOfFirst { it.layoutId == FOOT_ID }
        // The thread's foot runs past the bubble's padding to its edges and foot, as drawn.
        val side = 10.dp.roundToPx()
        val below = 6.dp.roundToPx()
        val placeables = arrayOfNulls<Placeable>(measurables.size)
        for ((index, measurable) in measurables.withIndex()) {
            if (index != footAt) placeables[index] = measurable.measure(loose)
        }
        val timeAt = measurables.indexOfFirst { it.layoutId == TIME_ID }
        val textAt = measurables.indexOfFirst { it.layoutId == TEXT_ID }
        val time = placeables.getOrNull(timeAt)
        val gap = 8.dp.roundToPx()
        val line = if (textAt >= 0) last.width else null
        val shares = time != null && line != null && line + gap + time.width <= constraints.maxWidth
        var width = placeables.filterIndexed { index, _ -> index != timeAt }.maxOfOrNull { it?.width ?: 0 } ?: 0
        if (time != null) width = maxOf(width, if (shares) line!! + gap + time.width else time.width)
        if (footAt >= 0) width = maxOf(width, measurables[footAt].maxIntrinsicWidth(Constraints.Infinity) - 2 * side)
        width = width.coerceIn(constraints.minWidth, constraints.maxWidth)
        if (footAt >= 0) placeables[footAt] = measurables[footAt].measure(Constraints.fixedWidth(width + 2 * side))
        val tops = IntArray(placeables.size)
        var y = 0
        for ((index, placeable) in placeables.withIndex()) {
            placeable ?: continue
            when (index) {
                timeAt ->
                    if (shares) {
                        // Level with the last line's foot, a little lower as drawn.
                        val low = 3.dp.roundToPx()
                        tops[index] = y - placeable.height + low
                        y += low
                    } else {
                        tops[index] = y + 2.dp.roundToPx()
                        y = tops[index] + placeable.height
                    }
                footAt -> {
                    tops[index] = y + 7.dp.roundToPx()
                    y = tops[index] + placeable.height - below
                }
                else -> {
                    tops[index] = y
                    y += placeable.height
                }
            }
        }
        layout(width, y.coerceIn(constraints.minHeight, constraints.maxHeight)) {
            for ((index, placeable) in placeables.withIndex()) {
                placeable ?: continue
                val x =
                    when (index) {
                        timeAt -> width - placeable.width
                        footAt -> -side
                        else -> 0
                    }
                placeable.placeRelative(x, tops[index])
            }
        }
    }

@Composable
private fun Ticks(
    bubble: Bubble,
    type: Int,
    me: String?,
    marks: List<MarkItem>,
    quiet: Color,
) {
    val seq = bubble.seq
    if (seq == null) {
        Clock(quiet)
        return
    }
    // A direct chat and a group alike, chosen by the author: two at the first member's received, the
    // seen colour at the first seen. A channel's one mark says the server has it.
    if (type != 1 && type != 2) {
        Checks(quiet, two = false)
        return
    }
    val them = marks.filter { it.account != me }
    when {
        them.any { (it.seen_seq ?: 0) >= seq } -> Checks(SEEN, two = true)
        them.any { it.received_seq >= seq } -> Checks(quiet, two = true)
        else -> Checks(quiet, two = false)
    }
}

@Composable
private fun Quote(
    bubble: Bubble,
    talk: Talk,
    me: String?,
    people: Map<String, AccountBrief>,
    mine: Boolean,
    ink: Ink,
    onJump: (Long) -> Unit,
) {
    val seq = bubble.reply
    val original = seq?.let { talk.bySeq(it) }
    val removed = seq != null && talk.gone[seq] == true
    val waiting = bubble.replyLocal?.let { local -> talk.bubbles.firstOrNull { it.local == local } }
    val target = original ?: waiting
    val fill = if (mine) Color.White.copy(alpha = 0.18f) else Color.Black.copy(alpha = 0.08f)
    val onFill = if (mine) Color.White else ink.text
    val quiet = if (mine) Color.White.copy(alpha = 0.72f) else ink.muted
    val gone = removed
    val unreadable = target != null && target.locked
    Column(
        Modifier
            .padding(bottom = 5.dp)
            .fillMaxWidth()
            .clip(RoundedCornerShape(8.dp))
            .background(fill)
            .clickable(enabled = seq != null && !removed) { if (seq != null) onJump(seq) }
            .padding(horizontal = 8.dp, vertical = 4.dp),
    ) {
        when {
            gone ->
                Text(stringResource(R.string.chat_deleted_message), color = quiet, fontSize = 13.sp, fontStyle = FontStyle.Italic)
            unreadable ->
                Text(stringResource(R.string.chat_unavailable), color = quiet, fontSize = 13.sp, fontStyle = FontStyle.Italic)
            target == null ->
                Text(stringResource(R.string.chat_earlier), color = quiet, fontSize = 13.sp, fontStyle = FontStyle.Italic)
            else -> {
                if (target.line == null) {
                    Text(
                        nameOf(target, me, people),
                        color = onFill,
                        fontSize = 12.sp,
                        fontWeight = FontWeight.Bold,
                        maxLines = 1,
                    )
                }
                Text(
                    quoteLine(target),
                    color = quiet,
                    fontSize = 13.sp,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
        }
    }
}

@Composable
private fun quoteLine(bubble: Bubble): String {
    bubble.line?.let { line ->
        val (me, people) = LocalWho.current
        return lineText(line.action, bubble.account.orEmpty(), line.accountList, me, people)
    }
    val text = bubble.text?.takeIf { it.isNotBlank() }
    if (text != null) return firstLine(text)
    if (bubble.sticker != null) return stringResource(R.string.kind_sticker)
    if (bubble.answer != null) return stringResource(R.string.kind_answered)
    bubble.question?.let { return firstQuestion(it) }
    bubble.suggestion?.let { suggestion ->
        val folder = suggestion.folder ?: return stringResource(R.string.suggestion_unknown)
        return stringResource(R.string.suggestion_preview, folder.name.trim())
    }
    if (bubble.call != null || bubble.kind == CONTENT_CALL) {
        return stringResource(if (bubble.call?.video == true) R.string.call_video else R.string.call_voice)
    }
    return kindLabel(bubble.kind)
}

@Composable
private fun kindLabel(kind: Int?): String =
    stringResource(
        when (kind) {
            1 -> R.string.kind_voice
            3 -> R.string.kind_photo
            4 -> R.string.kind_music
            5 -> R.string.kind_video
            6 -> R.string.kind_doc
            9 -> R.string.kind_bundle
            CONTENT_ACTIVITY -> R.string.activity_label
            else -> R.string.kind_file
        },
    )

@Composable
private fun Content(
    bubble: Bubble,
    quiet: Color,
    onFill: Color,
    selecting: Boolean,
    onOpen: () -> Unit,
    onSelect: () -> Unit,
    onCopy: (String) -> Unit,
    onFile: (FileInfo) -> Unit,
    talk: Talk,
    answerable: Boolean,
    onAnswer: () -> Unit,
    onJump: (Long) -> Unit,
) {
    val question = bubble.question
    val answer = bubble.answer
    when {
        bubble.locked -> {
            Row(Modifier.padding(vertical = 2.dp), verticalAlignment = Alignment.CenterVertically) {
                Box(
                    Modifier.size(32.dp).clip(CircleShape).background(Color.Black.copy(alpha = 0.12f)),
                    contentAlignment = Alignment.Center,
                ) { LockMark(quiet) }
                Column(Modifier.padding(start = 10.dp)) {
                    Text(stringResource(R.string.chat_cant_open), color = onFill, fontSize = 14.sp, fontWeight = FontWeight.Bold)
                    Text(stringResource(R.string.chat_tap_why), color = quiet, fontSize = 12.sp)
                }
            }
        }
        bubble.call != null || bubble.kind == CONTENT_CALL -> {
            val video = bubble.call?.video == true
            val missed = bubble.callOutcome == 0
            Text(
                stringResource(
                    when {
                        missed && video -> R.string.call_missed_video
                        missed -> R.string.call_missed_voice
                        video -> R.string.call_video
                        else -> R.string.call_voice
                    },
                ),
                color = onFill,
                fontSize = 15.sp,
                fontWeight = FontWeight.Bold,
            )
        }
        question != null -> {
            val first = firstQuestion(question)
            val intro = bubble.text?.takeIf { it.isNotBlank() }
            Text(intro ?: first, color = onFill, fontSize = 15.sp, fontWeight = FontWeight.Bold, lineHeight = 20.sp)
            if (intro != null) Text(first, color = quiet, fontSize = 13.sp, lineHeight = 18.sp)
            if (answerable) {
                Box(
                    Modifier
                        .padding(top = 8.dp, bottom = 2.dp)
                        .fillMaxWidth()
                        .clip(RoundedCornerShape(12.dp))
                        .background(accent)
                        .clickable(enabled = !selecting, role = Role.Button, onClick = onAnswer)
                        .padding(vertical = 8.dp),
                    contentAlignment = Alignment.Center,
                ) {
                    Text(stringResource(R.string.question_answer), color = onAccent, fontSize = 14.sp, fontWeight = FontWeight.Bold)
                }
            }
        }
        answer != null -> AnswerLines(answer, talk, bubble.mine, onFill, quiet, selecting, onJump)
        bubble.kind == CONTENT_ACTIVITY -> {
            Row(horizontalArrangement = Arrangement.spacedBy(7.dp), verticalAlignment = Alignment.Top) {
                Box(Modifier.padding(top = 2.dp)) { Glyph(Glyphs.bolt, if (bubble.mine) onFill else kit().accent, 16.dp) }
                Text(
                    bubble.text.orEmpty(),
                    Modifier.weight(1f, fill = false),
                    color = onFill,
                    fontSize = 15.sp,
                    style = TextStyle(textDirection = TextDirection.Content),
                )
            }
        }
        else -> {
            for (file in bubble.files) {
                Text(
                    "${file.name} · ${sizeOf(file.size)}",
                    Modifier.padding(bottom = 3.dp).clickable(enabled = !selecting) { onFile(file) },
                    color = onFill,
                    fontSize = 15.sp,
                    fontWeight = FontWeight.SemiBold,
                    textDecoration = TextDecoration.Underline,
                )
            }
            val text = bubble.text
            if (!text.isNullOrEmpty()) CopyText(text, bubble.mentions, onFill, selecting, onOpen, onSelect, onCopy)
        }
    }
}

@Composable
private fun CopyText(
    text: String,
    mentions: List<String>,
    color: Color,
    selecting: Boolean,
    onOpen: () -> Unit,
    onSelect: () -> Unit,
    onCopy: (String) -> Unit,
) {
    val (people, onProfile) = LocalMention.current
    val uri = LocalUriHandler.current
    val last = LocalLastLine.current
    val layoutDirection = LocalLayoutDirection.current
    val spans =
        remember(text, mentions, people) {
            copyables(text, mentions.mapNotNull { account -> people[account]?.let { it.username to account } }.toMap())
        }
    var layout by remember { mutableStateOf<TextLayoutResult?>(null) }
    val styled =
        remember(text, spans) {
            buildAnnotatedString {
                var at = 0
                for (span in spans) {
                    append(text.substring(at, span.start))
                    // Links and mentions are the seen ticks' blue, chosen by the author; a number is only underlined.
                    val blue = span.link || span.account != null
                    withStyle(SpanStyle(color = if (blue) SEEN else Color.Unspecified, textDecoration = TextDecoration.Underline)) {
                        append(text.substring(span.start, span.end))
                    }
                    at = span.end
                }
                append(text.substring(at))
            }
        }
    Text(
        styled,
        Modifier.pointerInput(text, selecting) {
            detectTapGestures(
                onTap = { point ->
                    val hit =
                        layout?.let { result ->
                            val offset = result.getOffsetForPosition(point)
                            spans.firstOrNull { offset >= it.start && offset < it.end }
                        }
                    when {
                        selecting -> onSelect()
                        // A link opens, a mention opens its profile and a number copies, chosen by the author.
                        hit?.link == true -> runCatching { uri.openUri(text.substring(hit.start, hit.end)) }
                        hit?.account != null -> onProfile(hit.account)
                        hit != null -> onCopy(text.substring(hit.start, hit.end))
                        else -> onOpen()
                    }
                },
                onLongPress = { if (!selecting) onSelect() },
            )
        },
        color = color,
        fontSize = 15.sp,
        lineHeight = 20.sp,
        onTextLayout = { result ->
            layout = result
            if (last != null) {
                val end = result.lineCount - 1
                val direction = result.getParagraphDirection(result.getLineStart(end))
                val along = (direction == ResolvedTextDirection.Rtl) == (layoutDirection == LayoutDirection.Rtl)
                // The time can follow the last line only when that line runs the same way as the screen.
                last.width = if (along) ceil(result.getLineRight(end) - result.getLineLeft(end)).toInt() else null
            }
        },
    )
}

@Composable
private fun NoteBar(
    title: String,
    line: String,
    ink: Ink,
    onClose: () -> Unit,
) {
    Row(
        Modifier
            .fillMaxWidth()
            .border(1.dp, ink.line)
            .background(ink.page)
            .padding(start = 14.dp, end = 4.dp, top = 6.dp, bottom = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(Modifier.weight(1f)) {
            Text(title, color = accent, fontSize = 12.sp, fontWeight = FontWeight.Bold, maxLines = 1)
            Text(line, color = ink.muted, fontSize = 13.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
        IconButton(stringResource(R.string.chat_cancel), onClose) { Cross(ink.muted) }
    }
}

@Composable
private fun Composer(
    focus: FocusRequester,
    draft: String,
    hint: String,
    blocked: Boolean,
    onDraft: (String) -> Unit,
    removeCount: Int,
    panel: Boolean,
    onPanel: () -> Unit,
    onFocused: () -> Unit,
    ink: Ink,
    onSend: () -> Unit,
    onRemove: () -> Unit,
) {
    Row(Modifier.fillMaxWidth().padding(start = 8.dp, end = 8.dp, top = 6.dp, bottom = 8.dp), verticalAlignment = Alignment.Bottom) {
        Row(
            Modifier
                .weight(1f)
                .heightIn(min = 48.dp)
                .background(ink.field, RoundedCornerShape(24.dp))
                .border(1.dp, ink.line, RoundedCornerShape(24.dp))
                .padding(start = 4.dp, end = 4.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            IconButton(
                stringResource(if (panel) R.string.chat_keyboard else R.string.chat_stickers),
                onPanel,
            ) { if (panel) KeysMark(ink.muted) else SmileMark(ink.muted) }
            BasicTextField(
                value = draft,
                onValueChange = onDraft,
                modifier =
                    Modifier
                        .weight(1f)
                        .padding(vertical = 10.dp)
                        .focusRequester(focus)
                        .onFocusChanged { if (it.isFocused) onFocused() },
                textStyle = TextStyle(color = ink.text, fontSize = 16.sp),
                cursorBrush = SolidColor(accent),
                maxLines = 5,
                keyboardOptions = KeyboardOptions(autoCorrectEnabled = true),
                decorationBox = { inner ->
                    if (draft.isEmpty()) Text(hint, color = ink.muted, fontSize = 16.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    inner()
                },
            )
            if (removeCount > 0) {
                Box(
                    Modifier.padding(end = 8.dp).size(28.dp).clip(CircleShape).border(1.5.dp, RED, CircleShape),
                    contentAlignment = Alignment.Center,
                ) {
                    Text(removeCount.toString(), color = RED, fontSize = 13.sp, fontWeight = FontWeight.Bold, textAlign = TextAlign.Center)
                }
                Box(
                    Modifier.padding(end = 2.dp).size(40.dp).clip(CircleShape).background(RED)
                        .clickable(role = Role.Button, onClick = onRemove),
                    contentAlignment = Alignment.Center,
                ) { TrashMark(Color.White) }
            } else {
                Box(
                    Modifier
                        .padding(end = 2.dp)
                        .size(40.dp)
                        .clip(CircleShape)
                        .background(accent.copy(alpha = if (draft.isBlank() || blocked) 0.45f else 1f))
                        .clickable(enabled = !blocked, role = Role.Button, onClick = onSend),
                    contentAlignment = Alignment.Center,
                ) { SendMark(Color.White) }
            }
        }
    }
}

@Composable
private fun Overlay(
    ink: Ink,
    dark: Boolean,
    onDismiss: () -> Unit,
    below: (@Composable () -> Unit)? = null,
    content: @Composable () -> Unit,
) {
    val scrim = remember { MutableInteractionSource() }
    Box(
        Modifier
            .fillMaxSize()
            .background(Color.Black.copy(alpha = 0.42f))
            .clickable(scrim, null, onClick = onDismiss)
            .padding(horizontal = 22.dp, vertical = 16.dp),
        contentAlignment = Alignment.Center,
    ) {
        val block = remember { MutableInteractionSource() }
        Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
            Column(
                Modifier
                    .fillMaxWidth()
                    .clip(RoundedCornerShape(22.dp))
                    .background(if (dark) Color(0xFF1E2833) else Color.White)
                    .clickable(block, null) {},
            ) { content() }
            if (below != null) {
                Box(
                    Modifier
                        .fillMaxWidth()
                        .clip(RoundedCornerShape(22.dp))
                        .background(if (dark) Color(0xFF1E2833) else Color.White)
                        .clickable(block, null) {},
                ) { below() }
            }
        }
    }
}

@Composable
private fun QuoteBox(
    line: String,
    ink: Ink,
) {
    Text(
        line,
        Modifier
            .padding(start = 14.dp, end = 14.dp, top = 14.dp, bottom = 4.dp)
            .fillMaxWidth()
            .clip(RoundedCornerShape(10.dp))
            .background(ink.line.copy(alpha = 0.6f))
            .padding(horizontal = 10.dp, vertical = 8.dp),
        color = ink.muted,
        fontSize = 13.sp,
        maxLines = 1,
        overflow = TextOverflow.Ellipsis,
    )
}

@Composable
private fun OptionRow(
    label: String,
    color: Color,
    onClick: () -> Unit,
    mark: @Composable () -> Unit,
) {
    Row(
        Modifier.fillMaxWidth().height(50.dp).clickable(role = Role.Button, onClick = onClick).padding(horizontal = 18.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        mark()
        Text(label, color = color, fontSize = 16.sp)
    }
}

@Composable
private fun OptionsModal(
    bubble: Bubble,
    name: String,
    ink: Ink,
    onReply: () -> Unit,
    onCopy: () -> Unit,
    onEdit: () -> Unit,
    onDelete: () -> Unit,
    onShowAnswer: (() -> Unit)?,
    onEditAnswer: (() -> Unit)?,
    mayReply: Boolean = true,
    onThread: (() -> Unit)? = null,
    onRemoveOthers: (() -> Unit)? = null,
) {
    QuoteBox(quoteLine(bubble), ink)
    Column(Modifier.padding(top = 4.dp, bottom = 6.dp)) {
        if (mayReply) OptionRow(stringResource(R.string.chat_reply), ink.text, onReply) { ReplyMark(ink.text) }
        if (onThread != null) {
            OptionRow(stringResource(R.string.group_reply_thread), ink.text, onThread) { Glyph(threadGlyph, ink.text) }
        }
        if (onShowAnswer != null) {
            OptionRow(stringResource(R.string.question_show_answer), ink.text, onShowAnswer) { DownMark(ink.text) }
        }
        if (onEditAnswer != null) {
            OptionRow(stringResource(R.string.chat_edit), ink.text, onEditAnswer) { PencilMark(ink.text) }
        }
        if ((bubble.kind == CONTENT_TEXT || bubble.kind == CONTENT_ACTIVITY) && !bubble.text.isNullOrEmpty()) {
            OptionRow(stringResource(R.string.chat_copy), ink.text, onCopy) { CopyMark(ink.text) }
        }
        if (bubble.mine && bubble.line == null) {
            if (bubble.kind == CONTENT_TEXT && !bubble.text.isNullOrEmpty()) {
                OptionRow(stringResource(R.string.chat_edit), ink.text, onEdit) { PencilMark(ink.text) }
            }
            OptionRow(stringResource(R.string.chat_delete), RED, onDelete) { TrashMark(RED) }
        } else if (onRemoveOthers != null) {
            OptionRow(stringResource(R.string.chat_delete), RED, onRemoveOthers) { TrashMark(RED) }
        }
    }
}

@Composable
private fun ModalTitle(
    text: String,
    color: Color,
) {
    Text(
        text,
        Modifier.padding(start = 18.dp, end = 18.dp, top = 16.dp, bottom = 4.dp),
        color = color,
        fontSize = 17.sp,
        fontWeight = FontWeight.Bold,
    )
}

@Composable
private fun ModalButtons(vararg buttons: Triple<String, Color, () -> Unit>) {
    Row(Modifier.fillMaxWidth().padding(start = 10.dp, end = 10.dp, top = 4.dp, bottom = 12.dp), horizontalArrangement = Arrangement.End) {
        for ((label, color, click) in buttons) {
            Text(
                label,
                Modifier.clip(
                    RoundedCornerShape(18.dp),
                ).clickable(role = Role.Button, onClick = click).padding(horizontal = 16.dp, vertical = 9.dp),
                color = color,
                fontSize = 15.sp,
                fontWeight = FontWeight.Bold,
            )
        }
    }
}

@Composable
private fun NoKeyModal(
    ink: Ink,
    onClose: () -> Unit,
) {
    Text(
        stringResource(R.string.chat_cant_open),
        Modifier.padding(start = 18.dp, end = 18.dp, top = 16.dp, bottom = 4.dp),
        color = ink.text,
        fontSize = 17.sp,
        fontWeight = FontWeight.Bold,
    )
    Text(
        stringResource(R.string.chat_nokey_body),
        Modifier.padding(start = 18.dp, end = 18.dp, top = 6.dp, bottom = 14.dp),
        color = ink.muted,
        fontSize = 14.sp,
        lineHeight = 20.sp,
    )
    ModalButtons(Triple(stringResource(R.string.chat_ok), accent, onClose))
}

@Composable
private fun ConfirmModal(
    count: Int,
    ink: Ink,
    onCancel: () -> Unit,
    onDelete: () -> Unit,
) {
    Text(
        if (count == 1) stringResource(R.string.chat_delete_one) else pluralStringResource(R.plurals.chat_delete_many, count, count),
        Modifier.padding(start = 18.dp, end = 18.dp, top = 16.dp, bottom = 4.dp),
        color = ink.text,
        fontSize = 17.sp,
        fontWeight = FontWeight.Bold,
    )
    Text(
        stringResource(R.string.chat_delete_body),
        Modifier.padding(start = 18.dp, end = 18.dp, top = 6.dp, bottom = 14.dp),
        color = ink.muted,
        fontSize = 14.sp,
        lineHeight = 20.sp,
    )
    ModalButtons(
        Triple(stringResource(R.string.chat_cancel), accent, onCancel),
        Triple(stringResource(R.string.chat_delete), RED, onDelete),
    )
}

@Composable
private fun MoveAskModal(
    folder: String,
    ink: Ink,
    onCancel: () -> Unit,
    onMove: () -> Unit,
) {
    Text(
        stringResource(R.string.suggestion_ask_title, folder),
        Modifier.padding(start = 18.dp, end = 18.dp, top = 16.dp, bottom = 4.dp),
        color = ink.text,
        fontSize = 17.sp,
        fontWeight = FontWeight.Bold,
    )
    Text(
        stringResource(R.string.suggestion_ask_body),
        Modifier.padding(start = 18.dp, end = 18.dp, top = 6.dp, bottom = 14.dp),
        color = ink.muted,
        fontSize = 14.sp,
        lineHeight = 20.sp,
    )
    ModalButtons(
        Triple(stringResource(R.string.suggestion_cancel), accent, onCancel),
        Triple(stringResource(R.string.suggestion_ask_move), accent, onMove),
    )
}

@Composable
private fun TimesModal(
    bubble: Bubble,
    answer: TimesAnswer?,
    me: String?,
    people: Map<String, AccountBrief>?,
    ink: Ink,
    onMore: () -> Unit,
    onClose: () -> Unit,
) {
    ModalTitle(stringResource(R.string.chat_times), ink.text)
    QuoteBox(quoteLine(bubble), ink)
    if (people != null) {
        GroupTimes(bubble, answer, people, ink, onMore)
        ModalButtons(Triple(stringResource(R.string.chat_close), accent, onClose))
        return
    }
    val member = answer?.member_list?.firstOrNull { if (bubble.mine) it.account != me else it.account == me }
    val loading = answer?.member_list == null
    val notYet = stringResource(R.string.chat_not_yet)
    val wait = stringResource(R.string.chat_loading)
    val rows =
        listOf(
            stringResource(R.string.chat_saved) to dateOf(bubble.at),
            stringResource(R.string.chat_received) to
                if (loading) wait else member?.received_at?.let { dateOf(it) } ?: notYet,
            stringResource(R.string.chat_seen) to
                if (loading) wait else member?.seen_at?.let { dateOf(it) } ?: notYet,
        )
    Column(Modifier.padding(horizontal = 18.dp, vertical = 6.dp)) {
        rows.forEachIndexed { index, (name, value) ->
            if (index > 0) Box(Modifier.fillMaxWidth().height(1.dp).background(ink.line))
            Row(Modifier.fillMaxWidth().padding(vertical = 9.dp), horizontalArrangement = Arrangement.SpaceBetween) {
                Text(name, color = ink.text, fontSize = 14.sp, fontWeight = FontWeight.SemiBold)
                Text(value, color = if (value == notYet || value == wait) ink.muted else ink.text, fontSize = 14.sp)
            }
        }
    }
    ModalButtons(Triple(stringResource(R.string.chat_close), accent, onClose))
}

/** A group's or channel's times, one row per member, chosen by the author: Received and Seen side by side. */
@OptIn(androidx.compose.foundation.layout.ExperimentalLayoutApi::class)
@Composable
private fun GroupTimes(
    bubble: Bubble,
    answer: TimesAnswer?,
    people: Map<String, AccountBrief>,
    ink: Ink,
    onMore: () -> Unit,
) {
    val notYet = stringResource(R.string.chat_not_yet)
    val received = stringResource(R.string.chat_received)
    val seen = stringResource(R.string.chat_seen)
    Column(Modifier.padding(horizontal = 18.dp, vertical = 6.dp)) {
        Row(Modifier.fillMaxWidth().padding(vertical = 9.dp), horizontalArrangement = Arrangement.SpaceBetween) {
            Text(stringResource(R.string.chat_saved), color = ink.text, fontSize = 14.sp, fontWeight = FontWeight.SemiBold)
            Text(dateOf(bubble.at), color = ink.text, fontSize = 14.sp)
        }
        Box(Modifier.fillMaxWidth().height(1.dp).background(ink.line))
        val list = answer?.member_list
        if (list == null) {
            Text(stringResource(R.string.chat_loading), color = ink.muted, fontSize = 14.sp, modifier = Modifier.padding(vertical = 9.dp))
            return@Column
        }
        // The next page of 50 is asked once the list is scrolled near its end, chosen by the author.
        val scroll = rememberScrollState()
        val nearEnd by remember { derivedStateOf { scroll.value >= scroll.maxValue - 200 } }
        LaunchedEffect(nearEnd, answer.more, answer.asking) {
            if (nearEnd && answer.more && !answer.asking) onMore()
        }
        Column(Modifier.heightIn(max = 340.dp).verticalScroll(scroll)) {
            for (member in list) {
                val name = personName(member.account, people)
                Row(Modifier.fillMaxWidth().padding(vertical = 7.dp), verticalAlignment = Alignment.CenterVertically) {
                    Avatar(member.account, name, 36.dp)
                    Column(Modifier.weight(1f).padding(start = 10.dp)) {
                        Text(
                            name,
                            color = ink.text,
                            fontSize = 14.sp,
                            fontWeight = FontWeight.SemiBold,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                        FlowRow(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                            for ((label, at) in listOf(received to member.received_at, seen to member.seen_at)) {
                                Text(
                                    "$label ${at?.let { dateOf(it) } ?: notYet}",
                                    color = if (at == null) ink.muted else ink.text,
                                    fontSize = 13.sp,
                                )
                            }
                        }
                    }
                }
            }
            if (answer.more) {
                Text(
                    stringResource(R.string.chat_loading),
                    color = ink.muted,
                    fontSize = 14.sp,
                    modifier = Modifier.padding(vertical = 9.dp),
                )
            }
        }
    }
}

private fun timeOf(at: Long): String =
    DateTimeFormatter.ofLocalizedTime(FormatStyle.SHORT).format(Instant.ofEpochMilli(at).atZone(ZoneId.systemDefault()))

private fun dateOf(at: Long): String {
    val then = Instant.ofEpochMilli(at).atZone(ZoneId.systemDefault())
    return if (then.toLocalDate() == LocalDate.now()) {
        DateTimeFormatter.ofLocalizedTime(FormatStyle.MEDIUM).format(then)
    } else {
        DateTimeFormatter.ofLocalizedDateTime(FormatStyle.MEDIUM, FormatStyle.MEDIUM).format(then)
    }
}

private fun sizeOf(bytes: Long): String =
    when {
        bytes >= 1 shl 20 -> String.format(java.util.Locale.ROOT, "%.1f MB", bytes / 1048576.0)
        bytes >= 1 shl 10 -> String.format(java.util.Locale.ROOT, "%.1f KB", bytes / 1024.0)
        else -> "$bytes B"
    }

// ---------------------------------------------------------------------------------------------
// Marks, drawn so they need no icon pack
// ---------------------------------------------------------------------------------------------

private fun stroke24(unit: Float) = Stroke(width = 2f * unit, cap = StrokeCap.Round, join = StrokeJoin.Round)

@Composable
private fun ReplyMark(color: Color) {
    Canvas(Modifier.size(20.dp)) {
        val unit = size.width / 24f
        val path =
            Path().apply {
                moveTo(10f * unit, 6f * unit)
                lineTo(4f * unit, 12f * unit)
                lineTo(10f * unit, 18f * unit)
                moveTo(4f * unit, 12f * unit)
                lineTo(15f * unit, 12f * unit)
                cubicTo(18f * unit, 12f * unit, 20f * unit, 14f * unit, 20f * unit, 18f * unit)
            }
        drawPath(path, color, style = stroke24(unit))
    }
}

@Composable
private fun CopyMark(color: Color) {
    Canvas(Modifier.size(20.dp)) {
        val unit = size.width / 24f
        val stroke = stroke24(unit)
        drawRoundRect(
            color,
            topLeft = Offset(8f * unit, 8f * unit),
            size = androidx.compose.ui.geometry.Size(12f * unit, 12f * unit),
            cornerRadius = androidx.compose.ui.geometry.CornerRadius(2f * unit),
            style = stroke,
        )
        drawPath(
            Path().apply {
                moveTo(16f * unit, 8f * unit)
                lineTo(16f * unit, 5f * unit)
                lineTo(5f * unit, 5f * unit)
                lineTo(5f * unit, 16f * unit)
                lineTo(8f * unit, 16f * unit)
            },
            color,
            style = stroke,
        )
    }
}

@Composable
private fun PencilMark(color: Color) {
    Canvas(Modifier.size(20.dp)) {
        val unit = size.width / 24f
        drawPath(
            Path().apply {
                moveTo(4f * unit, 20f * unit)
                lineTo(5f * unit, 15f * unit)
                lineTo(16f * unit, 4f * unit)
                lineTo(20f * unit, 8f * unit)
                lineTo(9f * unit, 19f * unit)
                close()
            },
            color,
            style = stroke24(unit),
        )
    }
}

@Composable
private fun TrashMark(color: Color) {
    Canvas(Modifier.size(20.dp)) {
        val unit = size.width / 24f
        val stroke = stroke24(unit)
        drawLine(color, Offset(4f * unit, 7f * unit), Offset(20f * unit, 7f * unit), stroke.width, StrokeCap.Round)
        drawPath(
            Path().apply {
                moveTo(6f * unit, 7f * unit)
                lineTo(7f * unit, 20f * unit)
                lineTo(17f * unit, 20f * unit)
                lineTo(18f * unit, 7f * unit)
                moveTo(9f * unit, 7f * unit)
                lineTo(9f * unit, 4f * unit)
                lineTo(15f * unit, 4f * unit)
                lineTo(15f * unit, 7f * unit)
            },
            color,
            style = stroke,
        )
    }
}

@Composable
private fun SendMark(color: Color) {
    val rtl = LocalLayoutDirection.current == LayoutDirection.Rtl
    Canvas(Modifier.size(20.dp)) {
        val unit = size.width / 24f
        val x = { value: Float -> (if (rtl) 24f - value else value) * unit }
        drawPath(
            Path().apply {
                moveTo(x(4f), 12f * unit)
                lineTo(x(20f), 4f * unit)
                lineTo(x(15f), 20f * unit)
                lineTo(x(11f), 13f * unit)
                close()
            },
            color,
            style = stroke24(unit),
        )
    }
}

@Composable
private fun LockMark(color: Color) {
    Canvas(Modifier.size(18.dp)) {
        val unit = size.width / 24f
        val stroke = stroke24(unit)
        drawRoundRect(
            color,
            topLeft = Offset(5f * unit, 11f * unit),
            size = androidx.compose.ui.geometry.Size(14f * unit, 10f * unit),
            cornerRadius = androidx.compose.ui.geometry.CornerRadius(2f * unit),
            style = stroke,
        )
        drawPath(
            Path().apply {
                moveTo(8f * unit, 11f * unit)
                lineTo(8f * unit, 8f * unit)
                cubicTo(8f * unit, 3f * unit, 16f * unit, 3f * unit, 16f * unit, 8f * unit)
                lineTo(16f * unit, 11f * unit)
            },
            color,
            style = stroke,
        )
    }
}

@Composable
private fun DownMark(color: Color) {
    Canvas(Modifier.size(20.dp)) {
        val unit = size.width / 24f
        drawPath(
            Path().apply {
                moveTo(12f * unit, 5f * unit)
                lineTo(12f * unit, 19f * unit)
                moveTo(6f * unit, 13f * unit)
                lineTo(12f * unit, 19f * unit)
                lineTo(18f * unit, 13f * unit)
            },
            color,
            style = stroke24(unit),
        )
    }
}

/** The panel's height until the keyboard has opened once and given its own. */
private const val PANEL_HEIGHT = 280

/** A sticker in the chat, and the picker's cells. */
private const val STICKER_SIDE = 160
private const val CELL_SIDE = 72

/** The server's stickers as the screen needs them: the packs once asked for, a sticker's kept file, and whether it is gone. */
internal class Stickers(
    val packs: List<StickerPack>?,
    val file: (String) -> java.io.File?,
    val gone: (String) -> Boolean,
    val recent: List<String> = emptyList(),
)

/** A WebP no larger than `px` on its longest side, or null if it cannot be read. */
private fun decodeSticker(
    file: java.io.File,
    px: Int,
): Drawable? =
    try {
        ImageDecoder.decodeDrawable(ImageDecoder.createSource(file)) { decoder, info, _ ->
            val longest = maxOf(info.size.width, info.size.height)
            if (longest > px) {
                val scale = px.toFloat() / longest
                decoder.setTargetSize(
                    maxOf(1, (info.size.width * scale).roundToInt()),
                    maxOf(1, (info.size.height * scale).roundToInt()),
                )
            }
        }
    } catch (_: Exception) {
        null
    }

/** A sticker's WebP, decoded off the main thread at the size it is shown. An animated one loops. */
@Composable
private fun StickerArt(
    file: java.io.File?,
    side: Dp,
    modifier: Modifier = Modifier,
) {
    val px = with(LocalDensity.current) { side.roundToPx() }
    var drawable by remember(file, px) { mutableStateOf<Drawable?>(null) }
    LaunchedEffect(file, px) {
        drawable = if (file == null) null else withContext(Dispatchers.IO) { decodeSticker(file, px) }
    }
    DisposableEffect(drawable) {
        onDispose { (drawable as? AnimatedImageDrawable)?.stop() }
    }
    Box(modifier.size(side)) {
        val shown = drawable
        if (shown != null) {
            AndroidView(
                factory = { context -> ImageView(context).apply { scaleType = ImageView.ScaleType.FIT_CENTER } },
                modifier = Modifier.fillMaxSize(),
                update = { view ->
                    if (view.drawable !== shown) {
                        view.setImageDrawable(shown)
                        if (shown is AnimatedImageDrawable) {
                            shown.repeatCount = AnimatedImageDrawable.REPEAT_INFINITE
                            shown.start()
                        }
                    }
                },
            )
        }
    }
}

/** A suggestion's pill word, as tall as its letters, so the pill centres them and not the font's spare room. */
private val pillText =
    TextStyle(
        lineHeight = 12.sp,
        platformStyle = PlatformTextStyle(includeFontPadding = false),
        lineHeightStyle = LineHeightStyle(LineHeightStyle.Alignment.Center, LineHeightStyle.Trim.Both),
    )

/**
 * A suggestion, drawn in `tmp/design/suggestions.html` and approved by the author: the label and time on
 * top, its text, the folder with Copy or Move, then Decline and Accept, which become the state at the same
 * height once this account has an ack. Once accepted the folder's pill says Open and the row opens that
 * folder; once declined the row is the name alone. Its own account's suggestion shows the other person's
 * ack instead, and keeps its pill.
 */
@Composable
private fun SuggestionBubble(
    bubble: Bubble,
    suggestion: Suggestion,
    type: Int,
    me: String?,
    people: Map<String, AccountBrief>,
    marks: List<MarkItem>,
    talk: Talk,
    selecting: Boolean,
    dark: Boolean,
    ink: Ink,
    fill: Color,
    onFill: Color,
    quiet: Color,
    onOpen: () -> Unit,
    onSelect: () -> Unit,
    onCopy: (String) -> Unit,
    onTime: () -> Unit,
    onJump: (Long) -> Unit,
    onAccept: () -> Unit,
    onDecline: () -> Unit,
    onFolder: (String) -> Unit,
    chips: @Composable () -> Unit,
) {
    val own = bubble.mine
    val tint =
        when {
            own -> Color.White.copy(alpha = 0.10f)
            dark -> Color.White.copy(alpha = 0.06f)
            else -> tile.copy(alpha = 0.07f)
        }
    val badge =
        if (own) {
            Color.White.copy(alpha = 0.16f)
        } else if (dark) {
            tile
        } else {
            Color(0xFFDCE7F7)
        }
    val onBadge =
        if (own) {
            Color.White
        } else if (dark) {
            Color(0xFFDCE7F7)
        } else {
            tile
        }
    val green = if (own || dark) Color(0xFF6FCF97) else Color(0xFF1B7F4A)
    val red = if (own || dark) Color(0xFFE26767) else RED
    val width = minOf(256f, LocalConfiguration.current.screenWidthDp * 0.84f).dp
    val folder = suggestion.folder
    val seq = bubble.seq
    Column(
        Modifier
            .width(width)
            .clip(RoundedCornerShape(16.dp))
            .background(fill)
            .pointerInput(bubble.key, selecting) {
                detectTapGestures(
                    onTap = { if (selecting) onSelect() else onOpen() },
                    onLongPress = { if (!selecting) onSelect() },
                )
            }.padding(start = 12.dp, end = 12.dp, top = 10.dp, bottom = 12.dp),
    ) {
        Row(Modifier.fillMaxWidth().height(28.dp), verticalAlignment = Alignment.CenterVertically) {
            Box(Modifier.size(28.dp).clip(CircleShape).background(badge), contentAlignment = Alignment.Center) {
                Glyph(Glyphs.folder, onBadge, 16.dp)
            }
            Text(
                stringResource(if (folder != null) R.string.suggestion_label else R.string.suggestion_unknown),
                Modifier.padding(start = 8.dp).weight(1f),
                color = quiet,
                fontSize = 12.5.sp,
                fontWeight = FontWeight.SemiBold,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            Text(
                timeOf(bubble.at),
                Modifier
                    .clip(RoundedCornerShape(8.dp))
                    .clickable(enabled = !selecting && seq != null, role = Role.Button, onClick = onTime)
                    .padding(horizontal = 4.dp, vertical = 1.dp),
                color = quiet,
                fontSize = 11.sp,
                fontWeight = FontWeight.Medium,
            )
            if (own) Ticks(bubble, type, me, marks, quiet)
        }
        if (bubble.reply != null || bubble.replyLocal != null) {
            Box(Modifier.padding(top = 8.dp)) { Quote(bubble, talk, me, people, own, ink, onJump) }
        }
        val text = bubble.text
        if (!text.isNullOrBlank()) {
            Box(Modifier.padding(top = 8.dp)) { CopyText(text, emptyList(), onFill, selecting, onOpen, onSelect, onCopy) }
        }
        if (folder == null) {
            SuggestionState(Glyphs.info, stringResource(R.string.suggestion_newer), quiet, tint)
        } else {
            val name = folder.name.trim()
            val mineAck = seq?.let { talk.ackOf(it) }
            // Once accepted the folder exists, so its row opens it; once declined the row is only its name.
            val opens = !own && mineAck?.accepted == true
            val pill =
                when {
                    opens -> R.string.suggestion_open
                    !own && mineAck != null -> null
                    folder.move -> R.string.suggestion_move
                    else -> R.string.suggestion_copy
                }
            Row(
                Modifier
                    .padding(top = 10.dp)
                    .fillMaxWidth()
                    .height(40.dp)
                    .clip(RoundedCornerShape(10.dp))
                    .background(tint)
                    .clickable(enabled = opens && !selecting, role = Role.Button) { onFolder(name) }
                    .padding(start = 10.dp, end = 8.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Glyph(Glyphs.folder, onFill, 16.dp)
                Text(
                    name,
                    Modifier.padding(horizontal = 8.dp).weight(1f),
                    color = onFill,
                    fontSize = 15.sp,
                    fontWeight = FontWeight.Bold,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                if (pill != null) {
                    Box(
                        Modifier
                            .height(24.dp)
                            .clip(RoundedCornerShape(12.dp))
                            .background(badge)
                            .padding(horizontal = 10.dp),
                        contentAlignment = Alignment.Center,
                    ) {
                        Text(
                            stringResource(pill),
                            color = onBadge,
                            fontSize = 12.sp,
                            fontWeight = FontWeight.SemiBold,
                            style = pillText,
                        )
                    }
                }
            }
            when {
                own -> {
                    val theirs = seq?.let { talk.theirAck(it) }
                    if (theirs != null) {
                        val who =
                            theirs.account?.let { people[it] }?.let { it.display_name?.takeIf { name -> name.isNotBlank() } ?: it.username }
                                .orEmpty()
                        if (theirs.accepted) {
                            SuggestionState(Glyphs.check, stringResource(R.string.suggestion_they_accepted, who), green, tint)
                        } else {
                            SuggestionState(Glyphs.cross, stringResource(R.string.suggestion_they_declined, who), quiet, tint)
                        }
                    }
                }
                mineAck != null -> {
                    val waiting = mineAck.seq == null
                    if (mineAck.accepted) {
                        SuggestionState(
                            if (waiting) Glyphs.clock else Glyphs.check,
                            stringResource(if (folder.move) R.string.suggestion_moved else R.string.suggestion_copied, name),
                            if (waiting) quiet else green,
                            tint,
                        )
                    } else {
                        SuggestionState(
                            if (waiting) Glyphs.clock else Glyphs.cross,
                            stringResource(R.string.suggestion_declined),
                            quiet,
                            tint,
                        )
                    }
                }
                seq != null -> {
                    val saving = talk.accepting == seq
                    Row(Modifier.padding(top = 10.dp).fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        Box(
                            Modifier
                                .weight(1f)
                                .height(38.dp)
                                .alpha(if (saving) 0.5f else 1f)
                                .clip(RoundedCornerShape(10.dp))
                                .border(1.5.dp, if (own) Color.White.copy(alpha = 0.35f) else ink.line, RoundedCornerShape(10.dp))
                                .clickable(enabled = !selecting && !saving, role = Role.Button, onClick = onDecline),
                            contentAlignment = Alignment.Center,
                        ) {
                            Text(
                                stringResource(R.string.suggestion_decline),
                                color = onFill,
                                fontSize = 14.sp,
                                fontWeight = FontWeight.SemiBold,
                            )
                        }
                        Box(
                            Modifier
                                .weight(1f)
                                .height(38.dp)
                                .alpha(if (saving) 0.5f else 1f)
                                .clip(RoundedCornerShape(10.dp))
                                .background(accent)
                                .clickable(enabled = !selecting && !saving, role = Role.Button, onClick = onAccept),
                            contentAlignment = Alignment.Center,
                        ) {
                            if (saving) {
                                CircularProgressIndicator(
                                    Modifier.size(16.dp),
                                    color = onAccent,
                                    strokeWidth = 2.dp,
                                    strokeCap = StrokeCap.Round,
                                )
                            } else {
                                Text(
                                    stringResource(R.string.suggestion_accept),
                                    color = onAccent,
                                    fontSize = 14.sp,
                                    fontWeight = FontWeight.SemiBold,
                                )
                            }
                        }
                    }
                    talk.refusals[seq]?.let { code ->
                        Row(Modifier.padding(top = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                            Glyph(Glyphs.alert, red, 15.dp)
                            Text(
                                stringResource(
                                    when (code) {
                                        "too_many_folders" -> R.string.suggestion_full
                                        "offline" -> R.string.suggestion_offline
                                        "no_permission" -> R.string.folders_refused_permission
                                        "unknown_folder" -> R.string.folders_refused_unknown
                                        else -> R.string.folders_refused
                                    },
                                ),
                                Modifier.padding(start = 6.dp),
                                color = red,
                                fontSize = 13.sp,
                                lineHeight = 18.sp,
                            )
                        }
                    }
                }
            }
        }
        chips()
    }
}

/** A suggestion's state, at the buttons' height: an icon and one line, cut with an ellipsis. */
@Composable
private fun SuggestionState(
    icon: List<String>,
    words: String,
    color: Color,
    tint: Color,
) {
    Row(
        Modifier
            .padding(top = 10.dp)
            .fillMaxWidth()
            .height(38.dp)
            .clip(RoundedCornerShape(10.dp))
            .background(tint)
            .padding(horizontal = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Glyph(icon, color, 16.dp)
        Text(
            words,
            Modifier.padding(start = 8.dp),
            color = color,
            fontSize = 13.5.sp,
            fontWeight = FontWeight.SemiBold,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
    }
}

/** A sticker message: no bubble, its time and ticks in a small pill on its corner. */
@Composable
private fun StickerBubble(
    bubble: Bubble,
    sticker: String,
    type: Int,
    me: String?,
    people: Map<String, AccountBrief>,
    marks: List<MarkItem>,
    talk: Talk,
    selecting: Boolean,
    stickers: Stickers,
    ink: Ink,
    onOpen: () -> Unit,
    onSelect: () -> Unit,
    onJump: (Long) -> Unit,
    chips: @Composable () -> Unit,
) {
    Column(
        Modifier
            .width(STICKER_SIDE.dp)
            .pointerInput(bubble.key, selecting) {
                detectTapGestures(
                    onTap = { if (selecting) onSelect() else onOpen() },
                    onLongPress = { if (!selecting) onSelect() },
                )
            },
    ) {
        if (bubble.reply != null || bubble.replyLocal != null) {
            Quote(bubble, talk, me, people, false, ink, onJump)
        }
        Box(Modifier.size(STICKER_SIDE.dp)) {
            if (stickers.gone(sticker)) {
                val line = ink.muted
                Box(
                    Modifier
                        .fillMaxSize()
                        .drawBehind {
                            drawRoundRect(
                                line,
                                cornerRadius = androidx.compose.ui.geometry.CornerRadius(16.dp.toPx()),
                                style = Stroke(width = 1.5.dp.toPx(), pathEffect = PathEffect.dashPathEffect(floatArrayOf(14f, 10f))),
                            )
                        }.padding(12.dp),
                    contentAlignment = Alignment.Center,
                ) {
                    Text(
                        stringResource(R.string.sticker_unavailable),
                        color = ink.muted,
                        fontSize = 13.sp,
                        fontStyle = FontStyle.Italic,
                        textAlign = TextAlign.Center,
                    )
                }
            } else {
                StickerArt(stickers.file(sticker), STICKER_SIDE.dp)
            }
            Row(
                Modifier
                    .align(Alignment.BottomEnd)
                    .padding(4.dp)
                    .clip(RoundedCornerShape(10.dp))
                    .background(Color.Black.copy(alpha = 0.42f))
                    .padding(horizontal = 7.dp, vertical = 2.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                if (bubble.failed) {
                    Text(
                        stringResource(R.string.chat_not_sent),
                        color = Color(0xFFFF8A8A),
                        fontSize = 11.sp,
                        fontWeight = FontWeight.SemiBold,
                    )
                } else {
                    Text(timeOf(bubble.at), color = Color.White, fontSize = 11.sp, fontWeight = FontWeight.Medium)
                    if (bubble.mine) Ticks(bubble, type, me, marks, Color.White.copy(alpha = 0.85f))
                }
            }
        }
        chips()
    }
}

/**
 * The picker, in the keyboard's place: a strip of pack icons, each its pack's first sticker, over every
 * pack in one grid under its name. An icon jumps to its pack, and the strip follows the scroll. Recent
 * stickers come first, with a clock in the strip, chosen by the author, and hide while there are none.
 */
@Composable
private fun StickerPanel(
    stickers: Stickers,
    height: Dp,
    ink: Ink,
    onPick: (String) -> Unit,
) {
    val known = stickers.packs.orEmpty().flatMap { it.sticker_list }.associateBy { it.id }
    val recent = stickers.recent.mapNotNull { id -> known[id]?.takeIf { !stickers.gone(id) } }
    val packs =
        stickers.packs.orEmpty().mapNotNull { pack ->
            val shown = pack.sticker_list.filter { !stickers.gone(it.id) }
            if (shown.isEmpty()) null else pack to shown
        }
    val recentName = stringResource(R.string.stickers_recent)
    // Each section: its key, its name and its stickers, Recent first while it has any.
    val sections =
        buildList {
            if (recent.isNotEmpty()) add(Triple("recent", recentName, recent))
            for ((pack, shown) in packs) add(Triple("pack-${pack.id}", pack.name, shown))
        }
    val heads = ArrayList<Int>()
    var at = 0
    for ((_, _, shown) in sections) {
        heads.add(at)
        at += 1 + shown.size
    }
    val grid = rememberLazyGridState()
    val scope = rememberCoroutineScope()
    val current = heads.indexOfLast { it <= grid.firstVisibleItemIndex }.coerceAtLeast(0)
    Column(
        Modifier
            .fillMaxWidth()
            .height(height)
            .border(1.dp, ink.line)
            .background(ink.page),
    ) {
        Row(
            Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()).padding(horizontal = 8.dp, vertical = 6.dp),
            horizontalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            sections.forEachIndexed { index, (key, name, shown) ->
                Box(
                    Modifier
                        .size(40.dp)
                        .clip(RoundedCornerShape(10.dp))
                        .background(if (index == current) accent.copy(alpha = 0.22f) else Color.Transparent)
                        .semantics { contentDescription = name }
                        .clickable(role = Role.Button) { scope.launch { grid.animateScrollToItem(heads[index]) } },
                    contentAlignment = Alignment.Center,
                ) {
                    if (key == "recent") Glyph(Glyphs.clock, ink.muted, 24.dp) else StickerArt(stickers.file(shown.first().id), 34.dp)
                }
            }
        }
        Box(Modifier.fillMaxWidth().height(1.dp).background(ink.line))
        LazyVerticalGrid(
            columns = GridCells.Fixed(5),
            modifier = Modifier.weight(1f).fillMaxWidth().padding(horizontal = 8.dp),
            state = grid,
        ) {
            for ((key, name, shown) in sections) {
                item(key = key, span = { GridItemSpan(maxLineSpan) }) {
                    Text(
                        name,
                        Modifier.padding(start = 4.dp, top = 10.dp, bottom = 4.dp),
                        color = ink.muted,
                        fontSize = 12.sp,
                        fontWeight = FontWeight.SemiBold,
                    )
                }
                for (sticker in shown) {
                    item(key = "$key-${sticker.id}") {
                        Box(
                            Modifier
                                .aspectRatio(1f)
                                .clip(RoundedCornerShape(10.dp))
                                .clickable(role = Role.Button) { onPick(sticker.id) }
                                .padding(2.dp),
                            contentAlignment = Alignment.Center,
                        ) { StickerArt(stickers.file(sticker.id), CELL_SIDE.dp) }
                    }
                }
            }
        }
    }
}

/** The emoji a reaction can be, once core has listed them, and `recent.reactions`, newest first. */
internal class Reactions(
    val groups: List<EmojiGroup>?,
    val recent: List<String>,
)

/** What fills the emoji row after the recent ones, chosen by the author. */
private val DEFAULT_REACTIONS = listOf("👍", "❤️", "😂", "😮", "😢", "🙏")

/** The emoji row's length before +, chosen by the author. */
private const val ROW_REACTIONS = 6

/** The picker strip's icon for each of core's groups, chosen by the author. */
private val GROUP_ICONS =
    mapOf(
        "smileys_and_emotion" to "😀",
        "people_and_body" to "👋",
        "animals_and_nature" to "🐻",
        "food_and_drink" to "🍔",
        "travel_and_places" to "✈️",
        "activities" to "⚽",
        "objects" to "💡",
        "symbols" to "❤️",
        "flags" to "🏳️",
    )

@Composable
private fun groupName(group: String): String =
    when (group) {
        "smileys_and_emotion" -> stringResource(R.string.emoji_smileys)
        "people_and_body" -> stringResource(R.string.emoji_people)
        "animals_and_nature" -> stringResource(R.string.emoji_animals)
        "food_and_drink" -> stringResource(R.string.emoji_food)
        "travel_and_places" -> stringResource(R.string.emoji_travel)
        "activities" -> stringResource(R.string.emoji_activities)
        "objects" -> stringResource(R.string.emoji_objects)
        "symbols" -> stringResource(R.string.emoji_symbols)
        "flags" -> stringResource(R.string.emoji_flags)
        else -> group
    }

/** The row under a message's options: the newest recent emoji, then the defaults not already there, then +. */
@Composable
private fun ReactRow(
    recent: List<String>,
    ink: Ink,
    onPick: (String) -> Unit,
    onMore: () -> Unit,
) {
    val shown = (recent + DEFAULT_REACTIONS).distinct().take(ROW_REACTIONS)
    val more = stringResource(R.string.emoji_more)
    Row(
        Modifier.fillMaxWidth().padding(horizontal = 10.dp, vertical = 6.dp),
        horizontalArrangement = Arrangement.SpaceBetween,
        verticalAlignment = Alignment.CenterVertically,
    ) {
        for (emoji in shown) {
            Box(
                Modifier.size(40.dp).clip(CircleShape).clickable(role = Role.Button) { onPick(emoji) },
                contentAlignment = Alignment.Center,
            ) { Text(emoji, fontSize = 24.sp) }
        }
        Box(
            Modifier
                .size(40.dp)
                .clip(CircleShape)
                .semantics { contentDescription = more }
                .clickable(role = Role.Button, onClick = onMore),
            contentAlignment = Alignment.Center,
        ) { Text("+", color = ink.muted, fontSize = 22.sp, fontWeight = FontWeight.Bold) }
    }
}

/**
 * A message's reactions, one chip per exact emoji with its count, yours tinted. Tapping yours removes your
 * newest with that emoji; tapping another adds the same.
 */
@OptIn(androidx.compose.foundation.layout.ExperimentalLayoutApi::class)
@Composable
private fun Chips(
    list: List<Chip>,
    onBubble: Boolean,
    mine: Boolean,
    dark: Boolean,
    ink: Ink,
    enabled: Boolean,
    onChip: (Chip) -> Unit,
) {
    if (list.isEmpty()) return
    FlowRow(
        Modifier.padding(top = 6.dp, bottom = 2.dp),
        horizontalArrangement = Arrangement.spacedBy(4.dp),
        verticalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        for (chip in list) {
            val yours = chip.mine != null
            val back =
                when {
                    onBubble && mine ->
                        Color.White.copy(
                            alpha =
                                when {
                                    yours && dark -> 0.32f
                                    yours -> 0.38f
                                    dark -> 0.14f
                                    else -> 0.18f
                                },
                        )
                    dark -> if (yours) Color(0xFF34507A) else Color(0xFF2A333F)
                    else -> if (yours) Color(0xFFC9D7EE) else Color(0xFFE1E6EE)
                }
            Row(
                Modifier
                    .height(26.dp)
                    .clip(RoundedCornerShape(13.dp))
                    .background(back)
                    .clickable(enabled = enabled, role = Role.Button) { onChip(chip) }
                    .padding(horizontal = 8.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(4.dp),
            ) {
                Text(chip.emoji, fontSize = 14.sp)
                if (chip.count > 1) {
                    Text(
                        chip.count.toString(),
                        color = if (onBubble && mine) Color.White else ink.text,
                        fontSize = 12.sp,
                        fontWeight = FontWeight.Bold,
                    )
                }
            }
        }
    }
}

/** One emoji in the picker, with its skin tones, the emoji itself left out. */
private data class EmojiCell(
    val emoji: String,
    val tones: List<String>,
)

/** A section of the picker: its key, its strip icon, its name and its emoji. */
private data class EmojiSection(
    val key: String,
    val icon: String,
    val name: String,
    val cells: List<EmojiCell>,
)

/** How many emoji the picker shows in a row. */
private const val EMOJI_ROW = 8

/** One line of the picker: a section's name, or a row of up to [EMOJI_ROW] of its emoji. */
private sealed interface EmojiLine {
    val key: String
}

private data class EmojiHead(
    override val key: String,
    val name: String,
) : EmojiLine

private data class EmojiRow(
    override val key: String,
    val cells: List<EmojiCell>,
) : EmojiLine

/**
 * Puts a popup just above one of [count] equal columns of what it belongs to, column 0 at the left, kept
 * inside the window.
 */
private class Above(
    val column: Int,
    val count: Int,
) : PopupPositionProvider {
    override fun calculatePosition(
        anchorBounds: IntRect,
        windowSize: IntSize,
        layoutDirection: LayoutDirection,
        popupContentSize: IntSize,
    ): IntOffset {
        val width = anchorBounds.width / count
        val x = anchorBounds.left + width * column + (width - popupContentSize.width) / 2
        val y = anchorBounds.top - popupContentSize.height
        return IntOffset(x.coerceIn(0, maxOf(0, windowSize.width - popupContentSize.width)), maxOf(0, y))
    }
}

/**
 * Every emoji, in a sheet over the chat: a search field, a strip of group icons that jumps and follows the
 * scroll, and the groups in one grid, Recent first while it has any. Search matches the English names and
 * shortcodes. A long press on an emoji with tones shows them above it, and nothing is remembered.
 */
@Composable
private fun EmojiSheet(
    reactions: Reactions,
    ink: Ink,
    dark: Boolean,
    onClose: () -> Unit,
    onPick: (String) -> Unit,
) {
    val scrim = remember { MutableInteractionSource() }
    val block = remember { MutableInteractionSource() }
    val height = (LocalConfiguration.current.screenHeightDp * 0.6f).dp
    val sheet = if (dark) Color(0xFF1E2833) else Color.White
    var query by remember { mutableStateOf("") }
    var toned by remember { mutableStateOf<String?>(null) }
    val groups = reactions.groups.orEmpty()
    val recent = reactions.recent
    // Built once per list, not on every recomposition: there are about 1,900 emoji.
    val tones = remember(groups) { groups.flatMap { it.emoji_list }.associate { it.emoji to it.tone_list } }
    val names = groups.map { groupName(it.group) }
    val recentName = stringResource(R.string.stickers_recent)
    val sections =
        remember(groups, recent, names, recentName) {
            buildList {
                if (recent.isNotEmpty()) {
                    add(EmojiSection("recent", "🕘", recentName, recent.map { EmojiCell(it, tones[it].orEmpty()) }))
                }
                groups.forEachIndexed { index, group ->
                    val icon = GROUP_ICONS[group.group] ?: group.emoji_list.firstOrNull()?.emoji.orEmpty()
                    add(EmojiSection(group.group, icon, names[index], group.emoji_list.map { EmojiCell(it.emoji, it.tone_list) }))
                }
            }
        }
    val needle = query.trim().trim(':').lowercase()
    val found =
        remember(groups, needle) {
            if (needle.isEmpty()) {
                null
            } else {
                groups
                    .flatMap { it.emoji_list }
                    .filter { item -> item.name.lowercase().contains(needle) || item.shortcode_list.any { it.contains(needle) } }
                    .map { EmojiCell(it.emoji, it.tone_list) }
            }
        }
    // A row of emoji is one item drawn on one canvas, not eight items each with its own text: composing
    // and laying out every emoji as it scrolled in made the picker stutter.
    val lines =
        remember(sections) {
            buildList {
                for (section in sections) {
                    add(EmojiHead(section.key, section.name))
                    section.cells.chunked(EMOJI_ROW).forEachIndexed { n, row -> add(EmojiRow("${section.key}-$n", row)) }
                }
            }
        }
    val heads = remember(lines) { lines.indices.filter { lines[it] is EmojiHead } }
    val foundLines = remember(found) { found?.chunked(EMOJI_ROW)?.mapIndexed { n, row -> EmojiRow("found-$n", row) } }
    val grid = rememberLazyListState()
    val scope = rememberCoroutineScope()
    val rtl = LocalLayoutDirection.current == LayoutDirection.Rtl
    // Each emoji is laid out once while the sheet is open, then only drawn.
    val measurer = rememberTextMeasurer(cacheSize = 0)
    val density = LocalDensity.current
    val glyphs = remember(density) { HashMap<String, TextLayoutResult>() }
    val glyph = TextStyle(color = ink.text, fontSize = 24.sp)

    @Composable
    fun Line(row: EmojiRow) {
        // The emoji under a point, counted from the start of the row whichever way the screen reads.
        fun index(
            x: Float,
            width: Int,
        ): Int {
            val column = (x * EMOJI_ROW / width).toInt().coerceIn(0, EMOJI_ROW - 1)
            return if (rtl) EMOJI_ROW - 1 - column else column
        }
        Box(
            Modifier
                .fillMaxWidth()
                .aspectRatio(EMOJI_ROW.toFloat())
                .drawBehind {
                    val width = size.width / EMOJI_ROW
                    row.cells.forEachIndexed { index, cell ->
                        val layout = glyphs.getOrPut(cell.emoji) { measurer.measure(cell.emoji, glyph) }
                        val column = if (rtl) EMOJI_ROW - 1 - index else index
                        drawText(
                            layout,
                            topLeft =
                                Offset(
                                    column * width + (width - layout.size.width) / 2,
                                    (size.height - layout.size.height) / 2,
                                ),
                        )
                    }
                }.pointerInput(row, rtl) {
                    detectTapGestures(
                        onLongPress = { at ->
                            val index = index(at.x, size.width)
                            if (row.cells.getOrNull(index)?.tones?.isNotEmpty() == true) toned = "${row.key}/$index"
                        },
                        onTap = { at -> row.cells.getOrNull(index(at.x, size.width))?.let { onPick(it.emoji) } },
                    )
                },
        ) {
            val index = toned?.takeIf { it.startsWith("${row.key}/") }?.substringAfter('/')?.toInt()
            val cell = index?.let { row.cells.getOrNull(it) }
            if (index != null && cell != null) {
                Popup(
                    popupPositionProvider = Above(if (rtl) EMOJI_ROW - 1 - index else index, EMOJI_ROW),
                    onDismissRequest = { toned = null },
                    properties = PopupProperties(focusable = true),
                ) {
                    Row(
                        Modifier
                            .shadow(8.dp, RoundedCornerShape(22.dp))
                            .clip(RoundedCornerShape(22.dp))
                            .background(sheet)
                            .padding(horizontal = 6.dp, vertical = 4.dp),
                    ) {
                        for (emoji in listOf(cell.emoji) + cell.tones) {
                            Box(
                                Modifier.size(42.dp).clip(CircleShape).clickable(role = Role.Button) {
                                    toned = null
                                    onPick(emoji)
                                },
                                contentAlignment = Alignment.Center,
                            ) { Text(emoji, fontSize = 26.sp) }
                        }
                    }
                }
            }
        }
    }

    Box(
        Modifier
            .fillMaxSize()
            .background(Color.Black.copy(alpha = 0.42f))
            .clickable(scrim, null, onClick = onClose),
        contentAlignment = Alignment.BottomCenter,
    ) {
        Column(
            Modifier
                .fillMaxWidth()
                // 60% of the screen, or less while the keyboard leaves less room.
                .heightIn(max = height)
                .fillMaxHeight()
                .clip(RoundedCornerShape(topStart = 22.dp, topEnd = 22.dp))
                .background(sheet)
                .clickable(block, null) {},
        ) {
            Box(
                Modifier
                    .padding(top = 8.dp, bottom = 8.dp)
                    .align(Alignment.CenterHorizontally)
                    .size(width = 36.dp, height = 4.dp)
                    .clip(RoundedCornerShape(2.dp))
                    .background(ink.muted.copy(alpha = 0.4f)),
            )
            BasicTextField(
                value = query,
                onValueChange = { query = it },
                modifier =
                    Modifier
                        .padding(horizontal = 14.dp)
                        .fillMaxWidth()
                        .background(ink.field, RoundedCornerShape(20.dp))
                        .border(1.dp, ink.line, RoundedCornerShape(20.dp))
                        .padding(horizontal = 16.dp, vertical = 10.dp),
                textStyle = TextStyle(color = ink.text, fontSize = 15.sp),
                cursorBrush = SolidColor(accent),
                singleLine = true,
                decorationBox = { inner ->
                    if (query.isEmpty()) Text(stringResource(R.string.emoji_search), color = ink.muted, fontSize = 15.sp)
                    inner()
                },
            )
            if (found == null) {
                GroupStrip(sections, heads, grid) { index -> scope.launch { grid.animateScrollToItem(heads[index]) } }
            } else {
                Spacer(Modifier.height(8.dp))
            }
            Box(Modifier.fillMaxWidth().height(1.dp).background(ink.line))
            LazyColumn(
                modifier = Modifier.weight(1f).fillMaxWidth().padding(horizontal = 8.dp),
                state = grid,
            ) {
                if (foundLines != null) {
                    if (foundLines.isEmpty()) {
                        item(key = "none") {
                            Text(
                                stringResource(R.string.emoji_none),
                                Modifier.fillMaxWidth().padding(top = 24.dp),
                                color = ink.muted,
                                fontSize = 14.sp,
                                textAlign = TextAlign.Center,
                            )
                        }
                    }
                    items(foundLines, key = { it.key }) { Line(it) }
                } else {
                    items(lines, key = { it.key }, contentType = { it is EmojiHead }) { line ->
                        when (line) {
                            is EmojiHead ->
                                Text(
                                    line.name,
                                    Modifier.padding(start = 4.dp, top = 10.dp, bottom = 4.dp),
                                    color = ink.muted,
                                    fontSize = 12.sp,
                                    fontWeight = FontWeight.SemiBold,
                                )
                            is EmojiRow -> Line(line)
                        }
                    }
                }
            }
        }
    }
}

/**
 * The picker's strip of group icons. It alone reads the grid's scroll, so scrolling recomposes only the
 * strip's highlight, never the sheet and its grid.
 */
@Composable
private fun GroupStrip(
    sections: List<EmojiSection>,
    heads: List<Int>,
    grid: androidx.compose.foundation.lazy.LazyListState,
    onJump: (Int) -> Unit,
) {
    val current by remember(heads) {
        derivedStateOf { heads.indexOfLast { it <= grid.firstVisibleItemIndex }.coerceAtLeast(0) }
    }
    Row(
        Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()).padding(horizontal = 8.dp, vertical = 6.dp),
        horizontalArrangement = Arrangement.spacedBy(2.dp),
    ) {
        sections.forEachIndexed { index, section ->
            Box(
                Modifier
                    .size(38.dp)
                    .clip(RoundedCornerShape(10.dp))
                    .background(if (index == current) accent.copy(alpha = 0.22f) else Color.Transparent)
                    .semantics { contentDescription = section.name }
                    .clickable(role = Role.Button) { onJump(index) },
                contentAlignment = Alignment.Center,
            ) { Text(section.icon, fontSize = 20.sp) }
        }
    }
}

/**
 * An answer: a line naming its question, which jumps to it, then each question with its answer. A
 * deleted question says Message unavailable; one further up says so and loads when tapped. Without the
 * question, the ids it holds are shown as they are.
 */
@Composable
private fun AnswerLines(
    answer: Answer,
    talk: Talk,
    mine: Boolean,
    onFill: Color,
    quiet: Color,
    selecting: Boolean,
    onJump: (Long) -> Unit,
) {
    val asked = talk.bySeq(answer.question)
    val tree = asked?.question
    val removed = talk.gone[answer.question] == true
    val fill = if (mine) Color.White.copy(alpha = 0.18f) else Color.Black.copy(alpha = 0.08f)
    Text(
        when {
            asked != null -> quoteLine(asked)
            removed -> stringResource(R.string.chat_unavailable)
            else -> stringResource(R.string.question_further_up)
        },
        Modifier
            .padding(bottom = 5.dp)
            .fillMaxWidth()
            .clip(RoundedCornerShape(8.dp))
            .background(fill)
            .clickable(enabled = !selecting && !removed) { onJump(answer.question) }
            .padding(horizontal = 8.dp, vertical = 4.dp),
        color = quiet,
        fontSize = 13.sp,
        fontStyle = if (removed) FontStyle.Italic else FontStyle.Normal,
        maxLines = 1,
        overflow = TextOverflow.Ellipsis,
    )
    if (answer.skipped) {
        Text(stringResource(R.string.question_skipped_all), color = onFill, fontSize = 14.sp, fontStyle = FontStyle.Italic)
        return
    }
    answer.answers.forEachIndexed { index, item ->
        if (index > 0) Box(Modifier.padding(vertical = 3.dp).fillMaxWidth().height(1.dp).background(quiet.copy(alpha = 0.3f)))
        val node = tree?.let { nodeOf(it, item.node) }
        Text(
            node?.text ?: item.node,
            color = onFill,
            fontSize = 14.sp,
            fontWeight = if (node != null) FontWeight.Bold else FontWeight.Normal,
            fontFamily = if (node != null) FontFamily.Default else FontFamily.Monospace,
        )
        Text(
            answerLine(node, item),
            color = onFill,
            fontSize = 14.sp,
            fontStyle = if (item.skipped) FontStyle.Italic else FontStyle.Normal,
        )
    }
}

/** One answer as words: the picked options' names, the number with its unit, or the text written in. */
@Composable
private fun answerLine(
    node: QuestionNode?,
    item: AnswerItem,
): String {
    if (item.skipped) return stringResource(R.string.question_skipped)
    val parts = ArrayList<String>()
    val number = item.number
    if (number != null) {
        val unit = node?.number?.unit
        parts.add(if (unit.isNullOrBlank()) plain(number) else "${plain(number)} $unit")
    }
    for (id in item.picked) parts.add(node?.choice?.options?.firstOrNull { it.id == id }?.text ?: id)
    item.own?.let { parts.add("“$it”") }
    return parts.joinToString(", ")
}

/** The answer sheet's state: the path so far, what each question holds, and whether it is at the summary. */
private class Form(
    val question: Bubble,
    val tree: Question,
    val editing: Bubble?,
    prior: Answer?,
) {
    val path = mutableStateListOf(tree.first)
    val drafts = mutableStateMapOf<String, Draft>()
    val skipped = mutableStateMapOf<String, Boolean>()
    var summary by mutableStateOf(false)
    var skippedAll by mutableStateOf(false)

    init {
        if (prior != null && !prior.skipped) {
            for (item in prior.answers) {
                if (item.skipped) skipped[item.node] = true else drafts[item.node] = Draft.of(item)
            }
        }
    }

    val here: String get() = path[path.lastIndex]

    fun item(id: String): AnswerItem? {
        if (skipped[id] == true) return AnswerItem(id, skipped = true)
        val node = nodeOf(tree, id) ?: return null
        return itemOf(node, drafts[id] ?: Draft())
    }

    /** Goes on from this question, answered or skipped, to the next one or the summary. */
    fun onward(skip: Boolean) {
        val id = here
        skipped[id] = skip
        val node = nodeOf(tree, id) ?: return
        val item = item(id) ?: return
        val next = nextOf(node, item)
        if (next != null && nodeOf(tree, next) != null) path.add(next) else summary = true
    }

    fun back() {
        if (summary) {
            summary = false
            skippedAll = false
        } else if (path.size > 1) {
            path.removeAt(path.lastIndex)
        }
    }

    fun answer(): Answer {
        val seq = question.seq ?: 0L
        if (skippedAll) return Answer(seq, skipped = true)
        return Answer(seq, answers = path.mapNotNull { item(it) })
    }
}

/** One question at a time from the bottom, up to 90% of the screen, then a summary and Send. */
@Composable
private fun AnswerSheet(
    form: Form,
    ink: Ink,
    dark: Boolean,
    onClose: () -> Unit,
    onSend: (Answer) -> Unit,
) {
    val scrim = remember { MutableInteractionSource() }
    val block = remember { MutableInteractionSource() }
    val tallest = (LocalConfiguration.current.screenHeightDp * 0.9f).dp
    Box(
        Modifier
            .fillMaxSize()
            .background(Color.Black.copy(alpha = 0.42f))
            .clickable(scrim, null, onClick = onClose),
        contentAlignment = Alignment.BottomCenter,
    ) {
        Column(
            Modifier
                .fillMaxWidth()
                .heightIn(max = tallest)
                .clip(RoundedCornerShape(topStart = 22.dp, topEnd = 22.dp))
                .background(if (dark) Color(0xFF1E2833) else Color.White)
                .clickable(block, null) {},
        ) {
            Box(
                Modifier
                    .padding(top = 8.dp, bottom = 2.dp)
                    .align(Alignment.CenterHorizontally)
                    .size(width = 36.dp, height = 4.dp)
                    .clip(RoundedCornerShape(2.dp))
                    .background(ink.muted.copy(alpha = 0.4f)),
            )
            val step =
                when {
                    form.summary && form.editing != null -> stringResource(R.string.question_changing)
                    form.summary -> stringResource(R.string.question_yours)
                    else -> stringResource(R.string.question_step, form.path.size)
                }
            Row(Modifier.fillMaxWidth().padding(start = 18.dp, end = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                Text(step, Modifier.weight(1f), color = ink.muted, fontSize = 12.sp, fontWeight = FontWeight.SemiBold)
                IconButton(stringResource(R.string.chat_close), onClose) { Cross(ink.muted) }
            }
            if (form.summary) {
                Summary(form, ink, onSend)
            } else {
                Step(form, ink)
            }
        }
    }
}

@Composable
private fun ColumnScope.Summary(
    form: Form,
    ink: Ink,
    onSend: (Answer) -> Unit,
) {
    Text(
        stringResource(R.string.question_check),
        Modifier.padding(start = 18.dp, end = 18.dp, bottom = 8.dp),
        color = ink.text,
        fontSize = 18.sp,
        fontWeight = FontWeight.Bold,
    )
    Column(Modifier.weight(1f, fill = false).verticalScroll(rememberScrollState()).padding(horizontal = 18.dp)) {
        if (form.skippedAll) {
            Text(stringResource(R.string.question_skipped_all), color = ink.text, fontSize = 14.sp, fontStyle = FontStyle.Italic)
        } else {
            form.path.forEachIndexed { index, id ->
                val item = form.item(id) ?: return@forEachIndexed
                val node = nodeOf(form.tree, id)
                if (index > 0) Box(Modifier.fillMaxWidth().height(1.dp).background(ink.line))
                Column(Modifier.padding(vertical = 8.dp)) {
                    Text(node?.text ?: id, color = ink.text, fontSize = 14.sp, fontWeight = FontWeight.Bold)
                    Text(
                        answerLine(node, item),
                        color = ink.muted,
                        fontSize = 14.sp,
                        fontStyle = if (item.skipped) FontStyle.Italic else FontStyle.Normal,
                    )
                }
            }
        }
    }
    Footer(ink) {
        SheetButton(stringResource(R.string.question_back), ink.text) { form.back() }
        Spacer(Modifier.weight(1f))
        SheetButton(stringResource(R.string.question_send), onAccent, main = true) { onSend(form.answer()) }
    }
}

@Composable
private fun ColumnScope.Step(
    form: Form,
    ink: Ink,
) {
    val id = form.here
    val node = nodeOf(form.tree, id) ?: return
    val draft = form.drafts[id] ?: Draft()

    fun update(next: Draft) {
        form.drafts[id] = next
    }
    Text(
        node.text,
        Modifier.padding(start = 18.dp, end = 18.dp, bottom = 8.dp),
        color = ink.text,
        fontSize = 18.sp,
        fontWeight = FontWeight.Bold,
        lineHeight = 24.sp,
    )
    Column(Modifier.weight(1f, fill = false).verticalScroll(rememberScrollState()).padding(horizontal = 10.dp)) {
        val choice = node.choice
        val range = node.number
        val multi = choice?.multi == true
        if (choice != null) {
            for (option in choice.options) {
                val on = option.id in draft.picked
                ChoiceRow(option.text, on, multi, ink) {
                    update(
                        if (multi) {
                            draft.copy(picked = if (on) draft.picked - option.id else draft.picked + option.id)
                        } else {
                            draft.copy(picked = listOf(option.id), ownOn = false)
                        },
                    )
                }
            }
        }
        if (range != null) {
            val problem = if (draft.ownOn) null else numberProblem(range, draft.number)
            Column(Modifier.padding(start = 8.dp, end = 8.dp, top = 4.dp, bottom = 4.dp).alpha(if (draft.ownOn) 0.4f else 1f)) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    SheetField(
                        value = if (draft.ownOn) "" else draft.number,
                        hint = plain(range.min),
                        enabled = !draft.ownOn,
                        number = true,
                        ink = ink,
                        modifier = Modifier.weight(1f),
                    ) { update(draft.copy(number = it)) }
                    range.unit?.takeIf { it.isNotBlank() }?.let {
                        Text(it, Modifier.padding(start = 10.dp), color = ink.text, fontWeight = FontWeight.SemiBold)
                    }
                }
                val bad = problem == NumberProblem.NotNumber || problem == NumberProblem.Range || problem == NumberProblem.Step
                Text(
                    when (problem) {
                        NumberProblem.Range, NumberProblem.NotNumber ->
                            stringResource(R.string.question_bad_range, plain(range.min), plain(range.max))
                        NumberProblem.Step -> stringResource(R.string.question_bad_step, plain(range.step ?: 1.0))
                        else -> rangeLine(range)
                    },
                    Modifier.padding(top = 6.dp),
                    color = if (bad) RED else ink.muted,
                    fontSize = 12.sp,
                )
            }
        }
        // Write your own: last, and opening a box. It replaces a single pick or the number, and adds to a multi choice's picks.
        ChoiceRow(stringResource(R.string.question_own), draft.ownOn, multi, ink) {
            update(
                when {
                    multi -> draft.copy(ownOn = !draft.ownOn)
                    range != null -> draft.copy(ownOn = !draft.ownOn)
                    else -> draft.copy(ownOn = true, picked = emptyList())
                },
            )
        }
        if (draft.ownOn) {
            SheetField(
                value = draft.own,
                hint = stringResource(R.string.question_own_hint),
                enabled = true,
                number = false,
                ink = ink,
                modifier = Modifier.padding(start = 40.dp, end = 8.dp, bottom = 8.dp).fillMaxWidth(),
            ) { update(draft.copy(own = it.take(OWN_TEXT))) }
        }
    }
    val ready = itemOf(node, draft) != null
    Footer(ink) {
        if (form.path.size > 1) SheetButton(stringResource(R.string.question_back), ink.text) { form.back() }
        SheetButton(stringResource(R.string.question_skip_all), ink.muted) {
            form.skippedAll = true
            form.summary = true
        }
        Spacer(Modifier.weight(1f))
        SheetButton(stringResource(R.string.question_skip), ink.muted) { form.onward(skip = true) }
        SheetButton(stringResource(R.string.question_next), onAccent, main = true, enabled = ready) { form.onward(skip = false) }
    }
}

/** Written-in text's limit, as core's. */
private const val OWN_TEXT = 2000

@Composable
private fun rangeLine(range: NumberRange): String {
    val unit = range.unit?.takeIf { it.isNotBlank() }
    val base = stringResource(R.string.question_range, plain(range.min), plain(range.max))
    val withUnit = if (unit != null) "$base $unit" else base
    val step = range.step
    return if (step != null && step != 1.0) withUnit + stringResource(R.string.question_in_steps, plain(step)) else withUnit
}

@Composable
private fun ChoiceRow(
    label: String,
    on: Boolean,
    multi: Boolean,
    ink: Ink,
    onClick: () -> Unit,
) {
    Row(
        Modifier
            .fillMaxWidth()
            .heightIn(min = 46.dp)
            .clip(RoundedCornerShape(10.dp))
            .clickable(role = if (multi) Role.Checkbox else Role.RadioButton, onClick = onClick)
            .padding(horizontal = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        val shape = if (multi) RoundedCornerShape(5.dp) else CircleShape
        Box(
            Modifier
                .size(20.dp)
                .clip(shape)
                .background(if (on && multi) accent else Color.Transparent)
                .border(2.dp, if (on) accent else ink.muted, shape),
            contentAlignment = Alignment.Center,
        ) {
            if (on && multi) Checks(Color.White, two = false)
            if (on && !multi) Box(Modifier.size(10.dp).clip(CircleShape).background(accent))
        }
        Text(label, color = ink.text, fontSize = 15.sp)
    }
}

@Composable
private fun SheetField(
    value: String,
    hint: String,
    enabled: Boolean,
    number: Boolean,
    ink: Ink,
    modifier: Modifier,
    onChange: (String) -> Unit,
) {
    BasicTextField(
        value = value,
        onValueChange = onChange,
        enabled = enabled,
        modifier =
            modifier
                .background(ink.field, RoundedCornerShape(12.dp))
                .border(1.dp, ink.line, RoundedCornerShape(12.dp))
                .padding(horizontal = 12.dp, vertical = 10.dp),
        textStyle = TextStyle(color = ink.text, fontSize = 15.sp),
        cursorBrush = SolidColor(accent),
        maxLines = if (number) 1 else 4,
        keyboardOptions =
            if (number) {
                KeyboardOptions(keyboardType = KeyboardType.Decimal)
            } else {
                KeyboardOptions(autoCorrectEnabled = true)
            },
        decorationBox = { inner ->
            if (value.isEmpty()) Text(hint, color = ink.muted, fontSize = 15.sp)
            inner()
        },
    )
}

@Composable
private fun Footer(
    ink: Ink,
    content: @Composable RowScope.() -> Unit,
) {
    Box(Modifier.fillMaxWidth().height(1.dp).background(ink.line))
    Row(
        Modifier.fillMaxWidth().padding(start = 8.dp, end = 8.dp, top = 8.dp, bottom = 14.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(2.dp),
        content = content,
    )
}

@Composable
private fun SheetButton(
    label: String,
    color: Color,
    main: Boolean = false,
    enabled: Boolean = true,
    onClick: () -> Unit,
) {
    Text(
        label,
        Modifier
            .clip(RoundedCornerShape(18.dp))
            .background(if (main) accent.copy(alpha = if (enabled) 1f else 0.45f) else Color.Transparent)
            .clickable(enabled = enabled, role = Role.Button, onClick = onClick)
            .padding(horizontal = if (main) 16.dp else 12.dp, vertical = 9.dp),
        color = color,
        fontSize = 14.sp,
        fontWeight = FontWeight.Bold,
        maxLines = 1,
    )
}

@Composable
private fun SmileMark(color: Color) {
    Canvas(Modifier.size(24.dp)) {
        val unit = size.width / 24f
        val stroke = stroke24(unit)
        drawCircle(color, radius = 9f * unit, center = Offset(12f * unit, 12f * unit), style = stroke)
        drawPath(
            Path().apply {
                moveTo(8.5f * unit, 14.5f * unit)
                quadraticTo(12f * unit, 18.5f * unit, 15.5f * unit, 14.5f * unit)
            },
            color,
            style = stroke,
        )
        drawCircle(color, radius = 1.2f * unit, center = Offset(9f * unit, 9.5f * unit))
        drawCircle(color, radius = 1.2f * unit, center = Offset(15f * unit, 9.5f * unit))
    }
}

@Composable
private fun KeysMark(color: Color) {
    Canvas(Modifier.size(24.dp)) {
        val unit = size.width / 24f
        val stroke = stroke24(unit)
        drawRoundRect(
            color,
            topLeft = Offset(2.5f * unit, 6f * unit),
            size = androidx.compose.ui.geometry.Size(19f * unit, 12f * unit),
            cornerRadius = androidx.compose.ui.geometry.CornerRadius(2f * unit),
            style = stroke,
        )
        for (x in listOf(6f, 10f, 14f, 18f)) drawCircle(color, radius = 1f * unit, center = Offset(x * unit, 10f * unit))
        drawLine(color, Offset(7f * unit, 14f * unit), Offset(17f * unit, 14f * unit), strokeWidth = 2f * unit, cap = StrokeCap.Round)
    }
}

/** A main-list message's thread: its replies, how many are new, and whether it says Comment, having none. */
internal data class ThreadCount(
    val total: Int,
    val fresh: Int,
    val comment: Boolean,
    val channel: Boolean = false,
)

/** The thread under a message in a group's or channel's main list; a channel's post with none says Comment. */
private fun threadCount(
    group: GroupView?,
    groups: GroupState?,
    bubble: Bubble,
    main: Boolean,
): ThreadCount? {
    if (group == null || groups == null || !main) return null
    val seq = bubble.seq ?: return null
    val key = group.chat to seq
    val total = groups.unread.replies[key] ?: 0
    val fresh = groups.unread.threads[key] ?: 0
    if (total > 0) return ThreadCount(total, fresh, false, group.type == TYPE_CHANNEL)
    if (group.type == TYPE_CHANNEL && bubble.line == null && !bubble.failed) return ThreadCount(0, 0, true)
    return null
}

/** The thread's count inside the bubble at its foot, such as "128 replies", with its new ones as a badge. */
@Composable
private fun ThreadFoot(
    count: ThreadCount,
    mine: Boolean,
    onFill: Color,
    enabled: Boolean,
    onClick: () -> Unit,
) {
    // The design's foot: the bubble's whole width under a thin line, the count in the accent, a chevron at the end.
    val tone = if (mine) onFill else kit().accent
    val rule = if (isDark() || mine) Color.White.copy(alpha = 0.08f) else Color.Black.copy(alpha = 0.08f)
    Row(
        Modifier
            .layoutId(FOOT_ID)
            .drawBehind { drawLine(rule, Offset(0f, 0f), Offset(size.width, 0f), 1.dp.toPx()) }
            .clickable(enabled = enabled, role = Role.Button, onClick = onClick)
            .padding(horizontal = 10.dp, vertical = 7.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Glyph(GroupGlyphs.bubble, tone, 16.dp)
        Text(
            if (count.comment) {
                stringResource(R.string.group_comment)
            } else {
                pluralStringResource(
                    if (count.channel) R.plurals.channel_comments else R.plurals.group_replies,
                    count.total,
                    exact(count.total.toLong()),
                )
            },
            Modifier.weight(1f),
            color = tone,
            fontSize = 13.sp,
            fontWeight = FontWeight.Bold,
            maxLines = 1,
        )
        CountPill(count.fresh, false, small = true)
        Glyph(chevronGlyph(), tone, 16.dp)
    }
}

/** Who is typing where this screen shows: its topic, or its thread; a channel's main list shows none. */
@Composable
private fun typingNow(
    groups: GroupState,
    chat: String,
    thread: Long?,
    topicsOn: Boolean,
    topic: String,
    channel: Boolean,
): List<Typist> {
    val all = groups.typing[chat].orEmpty()
    var now by remember { mutableLongStateOf(System.currentTimeMillis()) }
    LaunchedEffect(all) {
        while (all.isNotEmpty()) {
            now = System.currentTimeMillis()
            delay(1000)
        }
    }
    return all.filter {
        now - it.at < 6000 &&
            if (thread != null) {
                it.thread == thread
            } else {
                !channel && it.thread == null && (!topicsOn || (it.topic ?: "") == topic)
            }
    }
}

/**
 * A group's or channel's header: back, its photo and name over the member count or who is typing,
 * opening its page, and the requests icon at the far end while join requests wait.
 */
@Composable
internal fun GroupBar(
    title: String,
    chat: String,
    status: String?,
    live: Boolean,
    requests: Boolean,
    ink: Ink,
    onBack: () -> Unit,
    onRequests: () -> Unit,
    onPage: () -> Unit,
    icon: (@Composable () -> Unit)? = null,
    end: (@Composable () -> Unit)? = null,
) {
    val host = LocalGroups.current ?: return
    val rtl = LocalLayoutDirection.current == LayoutDirection.Rtl
    Row(Modifier.fillMaxWidth().height(64.dp).padding(start = 4.dp, end = 4.dp), verticalAlignment = Alignment.CenterVertically) {
        IconButton(stringResource(R.string.back), onBack, 48.dp) { BackMark(ink.text, rtl) }
        Row(
            Modifier.weight(1f).fillMaxHeight().clickable(role = Role.Button, onClick = onPage),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Box(Modifier.padding(start = 2.dp, end = 10.dp)) { icon?.invoke() ?: GroupAvatar(chat, host.state, host.engine, title, 44.dp) }
            Column(Modifier.weight(1f)) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    ChatTypeMark(host.state.infos[chat]?.type, ink, 16.dp)
                    Text(
                        title,
                        color = ink.text,
                        fontSize = 17.sp,
                        fontWeight = FontWeight.Bold,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                }
                if (status != null) {
                    Text(
                        status,
                        color = if (live) accentText() else ink.muted,
                        fontSize = 13.sp,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        style = TextStyle(textDirection = TextDirection.Content),
                    )
                }
            }
        }
        if (requests) RequestsMark(onRequests)
        end?.invoke()
    }
}

/**
 * The thread sheet's head, as the design draws it: the handle, then Thread, or Comments under a channel's
 * post, with who is typing in it or its count under it, and the cross at its end.
 */
@Composable
private fun ThreadBar(
    title: String,
    typing: String?,
    count: String?,
    ink: Ink,
    onClose: () -> Unit,
) {
    Column(Modifier.fillMaxWidth()) {
        Box(
            Modifier
                .padding(top = 10.dp, bottom = 4.dp)
                .align(Alignment.CenterHorizontally)
                .size(36.dp, 4.dp)
                .clip(RoundedCornerShape(2.dp))
                .background(kit().off),
        )
        Row(Modifier.fillMaxWidth().padding(start = 18.dp, end = 8.dp, bottom = 4.dp), verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f)) {
                Text(title, color = ink.text, fontSize = 17.sp, fontWeight = FontWeight.SemiBold)
                when {
                    typing != null -> Text(typing, color = accentText(), fontSize = 13.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    count != null -> Text(count, color = ink.muted, fontSize = 13.sp, maxLines = 1)
                }
            }
            IconButton(stringResource(R.string.chat_cancel), onClose, 44.dp) { Cross(ink.text) }
        }
    }
}

/**
 * A membership line: a small centred pill with its time, its reactions as chips centred under it and
 * its thread's count a small link under them. It acts as a message: a tap opens its options.
 */
@Composable
private fun PillRow(
    bubble: Bubble,
    talk: Talk,
    selecting: Boolean,
    flashing: Boolean,
    ink: Ink,
    dark: Boolean,
    replies: ThreadCount?,
    onOpen: () -> Unit,
    onSelect: () -> Unit,
    onChip: (Chip) -> Unit,
    onThread: () -> Unit,
) {
    val (me, people) = LocalWho.current
    val line = bubble.line ?: return
    val selected = bubble.key in talk.selected
    val tint =
        when {
            selected -> accent.copy(alpha = 0.20f)
            flashing -> accent.copy(alpha = 0.28f)
            else -> Color.Transparent
        }
    Column(
        Modifier
            .fillMaxWidth()
            .background(tint)
            .pointerInput(bubble.key, selecting) {
                detectTapGestures(
                    onTap = { if (selecting) onSelect() else onOpen() },
                    onLongPress = { if (!selecting) onSelect() },
                )
            }.padding(horizontal = 24.dp, vertical = 6.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        // The design's line: its words alone, with no time, in a small tinted pill.
        Text(
            lineText(line.action, bubble.account.orEmpty(), line.accountList, me, people),
            Modifier
                .padding(top = 2.dp)
                .clip(RoundedCornerShape(11.dp))
                .background(kit().tint)
                .padding(horizontal = 12.dp, vertical = 3.dp),
            color = ink.muted,
            fontSize = 12.5.sp,
            fontWeight = FontWeight.SemiBold,
            textAlign = TextAlign.Center,
        )
        val chips = talk.chips(bubble)
        if (chips.isNotEmpty()) {
            Box(Modifier.padding(top = 2.dp)) {
                Chips(chips, onBubble = false, mine = false, dark = dark, ink = ink, enabled = !selecting, onChip = onChip)
            }
        }
        if (replies != null && !replies.comment) {
            Row(
                Modifier
                    .padding(top = 2.dp)
                    .clip(RoundedCornerShape(8.dp))
                    .clickable(enabled = !selecting, role = Role.Button, onClick = onThread)
                    .padding(horizontal = 6.dp, vertical = 2.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(4.dp),
            ) {
                Text(
                    pluralStringResource(R.plurals.group_replies, replies.total, exact(replies.total.toLong())),
                    color = accentText(),
                    fontSize = 12.sp,
                    fontWeight = FontWeight.SemiBold,
                )
                CountPill(replies.fresh, false, small = true)
            }
        }
    }
}

/** Members whose name or username starts the word being typed after @, at most 5, above the text box. */
@Composable
private fun MentionList(
    chat: String,
    draft: String,
    me: String?,
    people: Map<String, AccountBrief>,
    ink: Ink,
    onPick: (String) -> Unit,
) {
    val host = LocalGroups.current ?: return
    val token = draft.substringAfterLast(' ').substringAfterLast('\n')
    if (!token.startsWith("@")) return
    val wanted = token.drop(1)
    val members = membersOf(chat, host)
    val list =
        members
            .orEmpty()
            .map { it.account }
            .filter { it != me }
            .mapNotNull { people[it] }
            .filter { it.username.startsWith(wanted, ignoreCase = true) || it.display_name?.contains(wanted, ignoreCase = true) == true }
            .take(5)
    if (list.isEmpty()) return
    Column(Modifier.fillMaxWidth().border(1.dp, ink.line).background(ink.page)) {
        for (brief in list) {
            val name = brief.display_name?.takeIf { it.isNotBlank() } ?: brief.username
            Row(
                Modifier
                    .fillMaxWidth()
                    .clickable(role = Role.Button) { onPick(draft.dropLast(token.length) + "@" + brief.username + " ") }
                    .padding(horizontal = 14.dp, vertical = 6.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Avatar(brief.id, name, 32.dp)
                Text(name, Modifier.padding(start = 10.dp), color = ink.text, fontSize = 15.sp, maxLines = 1)
                Text("@${brief.username}", Modifier.padding(start = 8.dp), color = ink.muted, fontSize = 13.sp, maxLines = 1)
            }
        }
    }
}

/** Seconds until slow mode lets this account write in the main list again; owners and moderators are exempt. */
@Composable
private fun slowLeft(
    group: GroupView?,
    groups: GroupState?,
    main: Boolean,
    managing: Boolean,
): Long {
    if (group == null || groups == null || !main || managing) return 0
    val until = groups.slowUntil[group.chat] ?: return 0
    var now by remember { mutableLongStateOf(System.currentTimeMillis()) }
    LaunchedEffect(until) {
        while (System.currentTimeMillis() < until) {
            now = System.currentTimeMillis()
            delay(500)
        }
        now = System.currentTimeMillis()
    }
    return ((until - now + 999) / 1000).coerceAtLeast(0)
}

private fun slowClock(seconds: Long) = "%d:%02d".format(seconds / 60, seconds % 60)
