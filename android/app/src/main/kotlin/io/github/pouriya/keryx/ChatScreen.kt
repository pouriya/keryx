package io.github.pouriya.keryx

import android.graphics.ImageDecoder
import android.graphics.drawable.AnimatedImageDrawable
import android.graphics.drawable.Drawable
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
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.aspectRatio
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
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
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
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.platform.LocalLayoutDirection
import androidx.compose.ui.platform.LocalSoftwareKeyboardController
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextLayoutResult
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.LayoutDirection
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.time.Instant
import java.time.LocalDate
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.time.format.FormatStyle
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

    data class Confirm(
        val list: List<Bubble>,
    ) : Modal

    data class Times(
        val bubble: Bubble,
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
    ) : Line
}

/** The result of asking for one message's times: `null` while it is on its way. */
internal data class TimesAnswer(
    val seq: Long,
    val member_list: List<TimesItem>?,
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
    onOlder: () -> Unit,
    onSeen: (Long) -> Unit,
    onDraft: (DraftText?) -> Unit,
    onFile: (FileInfo) -> Unit,
    stickers: Stickers,
    onStickers: () -> Unit,
    onSticker: (id: String, reply: Bubble?) -> Unit,
    onAnswer: (question: Bubble, answer: Answer, editing: Bubble?) -> Unit,
) {
    val dark = isSystemInDarkTheme()
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
    val allMine = picked.isNotEmpty() && picked.all { it.mine }

    fun copy(text: String) {
        clipboard.setText(AnnotatedString(text))
        toast = copied
    }

    val rows = rowsOf(talk.bubbles.toList()).asReversed()
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
        val wanted = latest
        if (wanted == kept) return@LaunchedEffect
        if (wanted != null) delay(DRAFT_AFTER_MS)
        kept = wanted
        onDraft(wanted)
    }
    DisposableEffect(Unit) {
        onDispose {
            if (latest != kept) onDraft(latest)
        }
    }
    // The draft the server holds for this chat comes back once, if nothing was typed meanwhile.
    LaunchedEffect(talk.restored, talk.bubbles.size) {
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
    val newest = talk.bubbles.lastOrNull()
    val atBottom by remember { derivedStateOf { state.firstVisibleItemIndex == 0 && state.firstVisibleItemScrollOffset < 40 } }
    var below by remember { mutableIntStateOf(0) }
    var previous by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(newest?.key) {
        val before = previous
        previous = newest?.key
        if (newest == null || before == null) return@LaunchedEffect
        // The old newest message still on screen means the reader was at the end: follow, else only count.
        val wasAtEnd = state.layoutInfo.visibleItemsInfo.any { it.key == before }
        if (wasAtEnd || newest.mine) {
            state.scrollToItem(0)
        } else if (!newest.mine) {
            below++
        }
    }
    LaunchedEffect(atBottom) {
        if (atBottom) below = 0
    }
    val lastOther = talk.bubbles.lastOrNull { !it.mine && it.seq != null }?.seq
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
        val at = rows.indexOfFirst { it is Line.One && it.bubble.seq == seq }
        if (at >= 0) {
            flash = "s$seq"
            talk.seeking = null
        } else if (talk.hasOlder) {
            talk.seeking = seq
            onOlder()
        }
    }
    LaunchedEffect(flash, rows.size) {
        val key = flash ?: return@LaunchedEffect
        val at = rows.indexOfFirst { it is Line.One && it.bubble.key == key }
        if (at >= 0) state.animateScrollToItem(at)
    }
    LaunchedEffect(talk.seeking, rows.size, talk.hasOlder) {
        val seq = talk.seeking ?: return@LaunchedEffect
        if (rows.any { it is Line.One && it.bubble.seq == seq }) {
            flash = "s$seq"
            talk.seeking = null
        } else if (talk.hasOlder) {
            onOlder()
        } else {
            talk.seeking = null
        }
    }

    Bars(ink.page, dark)
    Box(
        Modifier
            .fillMaxSize()
            .background(ink.page)
            .windowInsetsPadding(WindowInsets.statusBars)
            .windowInsetsPadding(WindowInsets.ime.union(WindowInsets.navigationBars)),
    ) {
        Column(Modifier.fillMaxSize()) {
            if (selecting) {
                SelectionBar(talk.selected.size, ink) { talk.selected.clear() }
            } else {
                TopBar(title, if (link == Link.Connected) null else statusText(link), ink, onBack)
            }
            LazyColumn(
                Modifier.weight(1f).fillMaxWidth(),
                state = state,
                reverseLayout = true,
            ) {
                items(rows, key = { rowKey(it) }) { row ->
                    when (row) {
                        is Line.Day -> DayRow(row.at, ink, dark)
                        is Line.One ->
                            MessageRow(
                                bubble = row.bubble,
                                joined = row.joined,
                                type = type,
                                me = me,
                                people = people,
                                marks = marks,
                                talk = talk,
                                selecting = selecting,
                                flashing = flash == row.bubble.key,
                                ink = ink,
                                dark = dark,
                                onOpen = {
                                    modal = if (row.bubble.locked) Modal.NoKey else Modal.Options(row.bubble)
                                },
                                onSelect = { talk.toggle(row.bubble) },
                                onCopy = ::copy,
                                onTime = {
                                    if (row.bubble.seq != null) {
                                        onTimes(row.bubble.seq)
                                        modal = Modal.Times(row.bubble)
                                    }
                                },
                                onReply = {
                                    editing = null
                                    replying = row.bubble
                                },
                                onJump = ::jump,
                                onFile = onFile,
                                stickers = stickers,
                                onAnswer = {
                                    val tree = row.bubble.question
                                    if (tree != null) openForm(Form(row.bubble, tree, null, null))
                                },
                            )
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
                    line = firstLine(note.text.orEmpty()),
                    ink = ink,
                ) {
                    replying = null
                    if (editing != null) {
                        editing = null
                        draft = ""
                    }
                }
            }
            Composer(
                focus = focus,
                draft = draft,
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
                    if (text.isNotEmpty()) {
                        val target = editing
                        if (target != null) onEdit(target, text) else onSend(text, replying)
                        draft = ""
                        replying = null
                        editing = null
                    }
                },
                onRemove = { modal = Modal.Confirm(picked) },
            )
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
            Overlay(ink, dark, onDismiss = { modal = null }) {
                when (shown) {
                    is Modal.Options ->
                        OptionsModal(
                            bubble = shown.bubble,
                            name = nameOf(shown.bubble, me, people),
                            ink = ink,
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
                    Modal.NoKey -> NoKeyModal(ink) { modal = null }
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
                            ink = ink,
                        ) { modal = null }
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
    }

private fun dayOf(at: Long): LocalDate = Instant.ofEpochMilli(at).atZone(ZoneId.systemDefault()).toLocalDate()

private fun rowsOf(list: List<Bubble>): List<Line> {
    val out = ArrayList<Line>()
    var day: LocalDate? = null
    var before: Bubble? = null
    for (bubble in list) {
        val today = dayOf(bubble.at)
        if (today != day) {
            out.add(Line.Day(bubble.at))
            day = today
            before = null
        }
        out.add(Line.One(bubble, before != null && before.mine == bubble.mine))
        before = bubble
    }
    return out
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
    status: String?,
    ink: Ink,
    onBack: () -> Unit,
) {
    val rtl = LocalLayoutDirection.current == LayoutDirection.Rtl
    Row(Modifier.fillMaxWidth().height(56.dp).padding(start = 4.dp, end = 8.dp), verticalAlignment = Alignment.CenterVertically) {
        IconButton(stringResource(R.string.back), onBack) { BackMark(ink.text, rtl) }
        Box(
            Modifier.padding(start = 2.dp, end = 10.dp).size(36.dp).clip(CircleShape).background(tile),
            contentAlignment = Alignment.Center,
        ) {
            Text(
                title.firstOrNull()?.uppercaseChar()?.toString() ?: "",
                color = onAccent,
                fontSize = 15.sp,
                fontWeight = FontWeight.Bold,
            )
        }
        Column(Modifier.weight(1f)) {
            Text(title, color = ink.text, fontSize = 16.sp, fontWeight = FontWeight.Bold, maxLines = 1, overflow = TextOverflow.Ellipsis)
            if (status != null) Text(status, color = ink.muted, fontSize = 12.sp, maxLines = 1)
        }
    }
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
            stringResource(R.string.chat_selected, count),
            Modifier.padding(start = 6.dp),
            color = ink.text,
            fontSize = 18.sp,
            fontWeight = FontWeight.Bold,
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
    onAnswer: () -> Unit,
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
            }.pointerInput(bubble.key) {
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
                StickerBubble(bubble, sticker, type, me, people, marks, talk, selecting, stickers, ink, onOpen, onSelect, onJump)
                return@Row
            }
            Column(
                Modifier
                    .widthIn(max = maxWidth)
                    .clip(RoundedCornerShape(16.dp))
                    .background(fill)
                    .pointerInput(bubble.key, selecting) {
                        detectTapGestures(
                            onTap = { if (selecting) onSelect() else onOpen() },
                            onLongPress = { if (!selecting) onSelect() },
                        )
                    }.padding(start = 10.dp, end = 10.dp, top = 7.dp, bottom = 6.dp),
            ) {
                if (bubble.reply != null || bubble.replyLocal != null) {
                    Quote(bubble, talk, me, people, bubble.mine, ink, onJump)
                }
                val answerable = !bubble.mine && bubble.seq != null && talk.answerTo(bubble.seq, mine = true) == null
                Content(bubble, quiet, onFill, selecting, onOpen, onSelect, onCopy, onFile, talk, answerable, onAnswer, onJump)
                Row(
                    Modifier.align(Alignment.End).padding(top = 2.dp),
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
                        if (bubble.edited) {
                            Text(stringResource(R.string.chat_edited), Modifier.padding(end = 4.dp), color = quiet, fontSize = 11.sp)
                        }
                        Text(
                            timeOf(bubble.at),
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
            }
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
    if (type != 1) {
        Checks(quiet, two = false)
        return
    }
    val them = marks.firstOrNull { it.account != me }
    when {
        them == null || them.received_seq < seq -> Checks(quiet, two = false)
        (them.seen_seq ?: 0) >= seq -> Checks(SEEN, two = true)
        else -> Checks(quiet, two = true)
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
                Text(nameOf(target, me, people), color = onFill, fontSize = 12.sp, fontWeight = FontWeight.Bold, maxLines = 1)
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
    val text = bubble.text?.takeIf { it.isNotBlank() }
    if (text != null) return firstLine(text)
    if (bubble.sticker != null) return stringResource(R.string.kind_sticker)
    if (bubble.answer != null) return stringResource(R.string.kind_answered)
    bubble.question?.let { return firstQuestion(it) }
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
            if (!text.isNullOrEmpty()) CopyText(text, onFill, selecting, onOpen, onSelect, onCopy)
        }
    }
}

@Composable
private fun CopyText(
    text: String,
    color: Color,
    selecting: Boolean,
    onOpen: () -> Unit,
    onSelect: () -> Unit,
    onCopy: (String) -> Unit,
) {
    val spans = remember(text) { copyables(text) }
    var layout by remember { mutableStateOf<TextLayoutResult?>(null) }
    val styled =
        remember(text, spans) {
            buildAnnotatedString {
                var at = 0
                for (span in spans) {
                    append(text.substring(at, span.start))
                    withStyle(SpanStyle(textDecoration = TextDecoration.Underline)) { append(text.substring(span.start, span.end)) }
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
        onTextLayout = { layout = it },
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
                    if (draft.isEmpty()) Text(stringResource(R.string.chat_message_hint), color = ink.muted, fontSize = 16.sp)
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
                        .background(accent.copy(alpha = if (draft.isBlank()) 0.45f else 1f))
                        .clickable(role = Role.Button, onClick = onSend),
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
        Column(
            Modifier
                .fillMaxWidth()
                .clip(RoundedCornerShape(22.dp))
                .background(if (dark) Color(0xFF1E2833) else Color.White)
                .clickable(block, null) {},
        ) { content() }
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
) {
    QuoteBox(quoteLine(bubble), ink)
    Column(Modifier.padding(top = 4.dp, bottom = 6.dp)) {
        OptionRow(stringResource(R.string.chat_reply), ink.text, onReply) { ReplyMark(ink.text) }
        if (onShowAnswer != null) {
            OptionRow(stringResource(R.string.question_show_answer), ink.text, onShowAnswer) { DownMark(ink.text) }
        }
        if (onEditAnswer != null) {
            OptionRow(stringResource(R.string.chat_edit), ink.text, onEditAnswer) { PencilMark(ink.text) }
        }
        if (bubble.kind == CONTENT_TEXT && !bubble.text.isNullOrEmpty()) {
            OptionRow(stringResource(R.string.chat_copy), ink.text, onCopy) { CopyMark(ink.text) }
        }
        if (bubble.mine) {
            if (bubble.kind == CONTENT_TEXT && !bubble.text.isNullOrEmpty()) {
                OptionRow(stringResource(R.string.chat_edit), ink.text, onEdit) { PencilMark(ink.text) }
            }
            OptionRow(stringResource(R.string.chat_delete), RED, onDelete) { TrashMark(RED) }
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
        if (count == 1) stringResource(R.string.chat_delete_one) else stringResource(R.string.chat_delete_many, count),
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
private fun TimesModal(
    bubble: Bubble,
    answer: TimesAnswer?,
    me: String?,
    ink: Ink,
    onClose: () -> Unit,
) {
    ModalTitle(stringResource(R.string.chat_times), ink.text)
    QuoteBox(quoteLine(bubble), ink)
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
    }
}

/**
 * The picker, in the keyboard's place: a strip of pack icons, each its pack's first sticker, over every
 * pack in one grid under its name. An icon jumps to its pack, and the strip follows the scroll.
 */
@Composable
private fun StickerPanel(
    stickers: Stickers,
    height: Dp,
    ink: Ink,
    onPick: (String) -> Unit,
) {
    val packs =
        stickers.packs.orEmpty().mapNotNull { pack ->
            val shown = pack.sticker_list.filter { !stickers.gone(it.id) }
            if (shown.isEmpty()) null else pack to shown
        }
    val heads = ArrayList<Int>()
    var at = 0
    for ((_, shown) in packs) {
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
            packs.forEachIndexed { index, (pack, shown) ->
                Box(
                    Modifier
                        .size(40.dp)
                        .clip(RoundedCornerShape(10.dp))
                        .background(if (index == current) accent.copy(alpha = 0.22f) else Color.Transparent)
                        .semantics { contentDescription = pack.name }
                        .clickable(role = Role.Button) { scope.launch { grid.animateScrollToItem(heads[index]) } },
                    contentAlignment = Alignment.Center,
                ) { StickerArt(stickers.file(shown.first().id), 34.dp) }
            }
        }
        Box(Modifier.fillMaxWidth().height(1.dp).background(ink.line))
        LazyVerticalGrid(
            columns = GridCells.Fixed(5),
            modifier = Modifier.weight(1f).fillMaxWidth().padding(horizontal = 8.dp),
            state = grid,
        ) {
            for ((pack, shown) in packs) {
                item(key = "pack-${pack.id}", span = { GridItemSpan(maxLineSpan) }) {
                    Text(
                        pack.name,
                        Modifier.padding(start = 4.dp, top = 10.dp, bottom = 4.dp),
                        color = ink.muted,
                        fontSize = 12.sp,
                        fontWeight = FontWeight.SemiBold,
                    )
                }
                for (sticker in shown) {
                    item(key = sticker.id) {
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
