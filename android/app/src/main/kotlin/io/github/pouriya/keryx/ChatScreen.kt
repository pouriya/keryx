package io.github.pouriya.keryx

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectHorizontalDragGestures
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
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
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalLayoutDirection
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextLayoutResult
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.LayoutDirection
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
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
            modal != null -> modal = null
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
        if (replying != null || editing != null) runCatching { focus.requestFocus() }
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
                Content(bubble, quiet, onFill, selecting, onOpen, onSelect, onCopy, onFile)
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
) {
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
                .padding(start = 16.dp, end = 4.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            BasicTextField(
                value = draft,
                onValueChange = onDraft,
                modifier = Modifier.weight(1f).padding(vertical = 10.dp).focusRequester(focus),
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
) {
    QuoteBox(quoteLine(bubble), ink)
    Column(Modifier.padding(top = 4.dp, bottom = 6.dp)) {
        OptionRow(stringResource(R.string.chat_reply), ink.text, onReply) { ReplyMark(ink.text) }
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
