package io.github.pouriya.keryx

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.gestures.detectVerticalDragGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.compositionLocalOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
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
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalLayoutDirection
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextDirection
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.LayoutDirection
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.zIndex
import kotlinx.coroutines.delay
import java.time.Instant
import java.time.LocalDate
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.time.format.FormatStyle
import java.time.temporal.ChronoUnit

/** How the list's title stands. The words replace the name until the list is current. */
internal enum class Link { Waiting, Connecting, Updating, Connected }

/** One conversation row. The name is worked out when it is drawn. */
internal data class ChatRow(
    val id: String,
    val type: Int,
    val lastAt: Long?,
    val lastSeq: Long?,
    val marks: List<MarkItem>,
    val preview: String,
    val mine: Boolean,
    val waiting: Boolean,
    /** Set when the preview is a call: video, or voice. */
    val callVideo: Boolean? = null,
    /** Its place among the pinned chats, from 0 on top, or none. */
    val pin: Int? = null,
)

/** The conversations on screen: the pinned ones first in their order, then the rest newest first. */
internal class ChatRows {
    val rows = mutableStateListOf<ChatRow>()

    /** The pinned chats in their order, first on top. */
    fun pinned(): List<String> = rows.filter { it.pin != null }.sortedBy { it.pin }.map { it.id }

    /** The whole pinned order, here or from another device: every chat it leaves out is unpinned. */
    fun pins(order: List<String>) {
        val unpinned = mutableListOf<ChatRow>()
        for (index in rows.indices) {
            val place = order.indexOf(rows[index].id).takeIf { it >= 0 }
            if (rows[index].pin != null && place == null) {
                unpinned.add(rows[index].copy(pin = null))
            } else if (rows[index].pin != place) {
                rows[index] = rows[index].copy(pin = place)
            }
        }
        rows.removeAll { row -> unpinned.any { it.id == row.id } }
        settle()
        // A chat that loses its pin goes back among the rest by its time, not where its pin held it.
        for (row in unpinned) {
            val at = rows.indexOfFirst { it.pin == null && (it.lastAt ?: 0) < (row.lastAt ?: 0) }
            rows.add(if (at < 0) rows.size else at, row)
        }
    }

    /** Pinned rows go to the top in their order. The rest keep theirs, which is newest first. */
    private fun settle() {
        val sorted = rows.sortedBy { it.pin ?: Int.MAX_VALUE }
        if (sorted == rows.toList()) return
        rows.clear()
        rows.addAll(sorted)
    }

    fun replace(list: List<ChatItem>) {
        val previous = rows.associateBy { it.id }
        rows.clear()
        for (item in list) {
            val old = previous[item.chat]
            val kept = old != null && old.lastSeq == item.last_content_seq
            rows.add(
                ChatRow(
                    id = item.chat,
                    type = item.type,
                    lastAt = item.last_at,
                    lastSeq = item.last_content_seq,
                    marks = item.mark_list,
                    preview = if (old != null && kept) old.preview else "",
                    mine = if (old != null && kept) old.mine else false,
                    waiting = if (old != null && kept) old.waiting else false,
                    callVideo = if (old != null && kept) old.callVideo else null,
                    pin = item.pin,
                ),
            )
        }
        settle()
    }

    fun message(
        message: Shown,
        preview: String,
    ) {
        val kind = message.content_kind ?: return
        if (kind == CALL) {
            showCall(message, preview)
            return
        }
        val index = rows.indexOfFirst { it.id == message.chat }
        if (index < 0) {
            rows.add(
                0,
                ChatRow(
                    id = message.chat,
                    type = DIRECT,
                    lastAt = message.saved_at,
                    lastSeq = message.seq,
                    marks = emptyList(),
                    preview = preview,
                    mine = message.mine,
                    waiting = false,
                ),
            )
            settle()
            return
        }
        val row = rows[index]
        if (row.lastSeq != null && message.seq < row.lastSeq && !row.waiting) return
        val next =
            row.copy(
                lastAt = message.saved_at,
                lastSeq = message.seq,
                preview = preview,
                mine = message.mine,
                waiting = false,
                callVideo = null,
            )
        val newer = row.waiting || row.lastSeq == null || message.seq > row.lastSeq
        if (newer) {
            rows.removeAt(index)
            rows.add(0, next)
            settle()
        } else {
            rows[index] = next
        }
    }

    fun pending(
        chat: String,
        text: String?,
    ) {
        val index = rows.indexOfFirst { it.id == chat }
        if (index < 0) return
        val row = rows[index]
        val next = row.copy(preview = text ?: row.preview, mine = true, waiting = true, callVideo = null)
        rows.removeAt(index)
        rows.add(0, next)
        settle()
    }

    fun sent(
        chat: String,
        seq: Long,
        at: Long,
    ) {
        val index = rows.indexOfFirst { it.id == chat }
        if (index < 0) return
        val row = rows[index]
        if (!row.waiting) return
        rows[index] = row.copy(waiting = false, lastSeq = seq, lastAt = at)
    }

    /** A call stays where the chat already is. Its time does not move, and it is not unread. */
    private fun showCall(
        message: Shown,
        preview: String,
    ) {
        val video = message.call?.video == true
        val index = rows.indexOfFirst { it.id == message.chat }
        if (index < 0) {
            rows.add(
                0,
                ChatRow(
                    id = message.chat,
                    type = DIRECT,
                    lastAt = message.saved_at,
                    lastSeq = message.seq,
                    marks = emptyList(),
                    preview = preview,
                    mine = message.mine,
                    waiting = false,
                    callVideo = video,
                ),
            )
            settle()
            return
        }
        val row = rows[index]
        if (row.lastSeq != null && message.seq < row.lastSeq) return
        rows[index] =
            row.copy(
                lastSeq = message.seq,
                preview = preview,
                mine = message.mine,
                waiting = false,
                callVideo = video,
            )
    }

    /** The call's end names it missed or not, once that row is already the preview. */
    fun callStatus(
        chat: String,
        seq: Long,
        preview: String,
    ) {
        val index = rows.indexOfFirst { it.id == chat }
        if (index < 0) return
        val row = rows[index]
        if (row.callVideo == null || row.lastSeq != seq) return
        rows[index] = row.copy(preview = preview)
    }

    fun marks(
        chat: String,
        mark: MarkItem,
    ) {
        val index = rows.indexOfFirst { it.id == chat }
        if (index < 0) return
        val row = rows[index]
        val marks = row.marks.toMutableList()
        val at = marks.indexOfFirst { it.account == mark.account }
        if (at >= 0) marks[at] = mark else marks.add(mark)
        rows[index] = row.copy(marks = marks)
    }

    /** The last message was removed: the row falls back to the newest one still shown, or to none. */
    fun walkBack(
        chat: String,
        talk: Talk,
        preview: (Bubble) -> String,
    ) {
        val index = rows.indexOfFirst { it.id == chat }
        if (index < 0) return
        val row = rows[index]
        val last = row.lastSeq ?: return
        if (talk.gone[last] != true) return
        val previous = talk.bubbles.lastOrNull { it.seq != null && it.seq < last && it.kind != null }
        rows[index] =
            if (previous == null) {
                row.copy(lastSeq = null, preview = "", mine = false, waiting = false, callVideo = null)
            } else {
                row.copy(
                    lastSeq = previous.seq,
                    preview = preview(previous),
                    mine = previous.mine,
                    waiting = false,
                    callVideo = if (previous.kind == CALL) previous.call?.video == true else null,
                )
            }
    }

    fun clear() = rows.clear()
}

private const val CALL = 8
private const val SELF = 0
private const val DIRECT = 1
private const val GROUP = 2
private const val FILTER_AFTER = 300L
private val accent = Color(0xFF4576C4)
private val onAccent = Color(0xFFFFFFFF)

@Composable
internal fun Chats(
    link: Link,
    serverName: String?,
    host: String,
    people: Map<String, AccountBrief>,
    me: String?,
    unread: Map<String, Int>,
    rows: List<ChatRow>,
    ringing: Shown?,
    found: List<AccountBrief>,
    foundFor: String,
    onSearch: (String) -> Unit,
    onPerson: (AccountBrief) -> Unit,
    onOpen: (ChatRow) -> Unit,
    onProfile: (String) -> Unit,
    onAnswer: (Shown) -> Unit,
    onDecline: (Shown) -> Unit,
    onPins: (List<String>) -> Unit,
) {
    val dark = isDark()
    val ink = ink()
    val accentText = if (dark) Color(0xFFC5D4E8) else accent
    var open by remember { mutableStateOf(false) }
    var query by remember { mutableStateOf("") }
    var applied by remember { mutableStateOf("") }
    var busy by remember { mutableStateOf(false) }
    val focus = remember { FocusRequester() }
    LaunchedEffect(query, open) {
        if (!open || query.isEmpty()) {
            busy = false
            applied = ""
            onSearch("")
            return@LaunchedEffect
        }
        busy = true
        delay(FILTER_AFTER)
        applied = query
        onSearch(query)
    }
    LaunchedEffect(foundFor, applied) {
        if (applied.isNotEmpty() && foundFor == applied) busy = false
    }
    LaunchedEffect(open) {
        if (open) runCatching { focus.requestFocus() }
    }

    fun closeSearch() {
        open = false
        query = ""
        applied = ""
        busy = false
    }
    BackHandler(enabled = open) { closeSearch() }
    // A long press on a row, not its photo, starts selecting, as messages do, chosen by the author.
    val selected = remember { mutableStateListOf<String>() }
    val selecting = selected.isNotEmpty()
    BackHandler(enabled = selecting) { selected.clear() }
    LaunchedEffect(rows.map { it.id }) { selected.retainAll(rows.map { it.id }.toSet()) }
    // While a pinned row is dragged, the pinned order as it stands under the finger.
    var dragOrder by remember { mutableStateOf<List<String>?>(null) }
    var dragId by remember { mutableStateOf<String?>(null) }
    var dragBy by remember { mutableFloatStateOf(0f) }
    var rowHeight by remember { mutableIntStateOf(0) }
    val current by rememberUpdatedState(rows)
    val listState = rememberLazyListState()
    Bars(ink.page, dark)
    val connected = link == Link.Connected
    val saved = stringResource(R.string.saved_messages)
    val title = if (connected) serverName?.takeIf { it.isNotBlank() } ?: host else statusText(link)
    val filtering = open && !busy && applied.isNotEmpty()
    val ordered =
        dragOrder?.let { order -> rows.sortedBy { row -> order.indexOf(row.id).takeIf { it >= 0 } ?: Int.MAX_VALUE } } ?: rows
    val shown =
        if (filtering) {
            val matched = rows.filter { label(it, people, saved).startsWith(applied, ignoreCase = true) }
            val self = rows.firstOrNull { it.type == 0 }
            if (self != null && found.any { it.id == me } && matched.none { it.id == self.id }) {
                listOf(self) + matched
            } else {
                matched
            }
        } else {
            ordered
        }
    // Every pinned row gets a handle while selecting, once two or more are pinned, chosen by the author.
    val pinnedCount = rows.count { it.pin != null }
    val handles = selecting && !filtering && pinnedCount >= 2

    fun finishDrag(keep: Boolean) {
        val order = dragOrder
        if (keep && order != null && order != current.filter { it.pin != null }.sortedBy { it.pin }.map { it.id }) onPins(order)
        dragId = null
        dragBy = 0f
        dragOrder = null
    }

    fun dragStep() {
        val order = dragOrder ?: return
        val id = dragId ?: return
        val at = order.indexOf(id)
        val height = rowHeight.toFloat().takeIf { it > 0f } ?: return
        val to =
            when {
                dragBy > height / 2 && at < order.lastIndex -> at + 1
                dragBy < -height / 2 && at > 0 -> at - 1
                else -> {
                    if (at == 0) dragBy = dragBy.coerceAtLeast(0f)
                    if (at == order.lastIndex) dragBy = dragBy.coerceAtMost(0f)
                    return
                }
            }
        // Keep the list where it is: a lazy list follows its first visible row when that row moves.
        val first = listState.firstVisibleItemIndex
        val offset = listState.firstVisibleItemScrollOffset
        dragOrder = order.toMutableList().apply { add(to, removeAt(at)) }
        dragBy += if (to > at) -height else height
        listState.requestScrollToItem(first, offset)
    }
    val known = rows.flatMap { row -> row.marks.map { it.account } }.toSet()
    val strangers = if (filtering) found.filter { it.id != me && it.id !in known } else emptyList()
    Box(Modifier.fillMaxSize().background(ink.page).windowInsetsPadding(WindowInsets.statusBars)) {
        Column(Modifier.fillMaxSize()) {
            if (selecting) {
                PinBar(
                    selected.size,
                    ink,
                    onClose = { selected.clear() },
                    // New pins go under the ones already pinned, in the list's order, chosen by the author.
                    onPin = {
                        val pinned = rows.filter { it.pin != null }.sortedBy { it.pin }.map { it.id }
                        val added = rows.filter { it.id in selected && it.pin == null }.map { it.id }
                        if (added.isNotEmpty()) onPins(pinned + added)
                        selected.clear()
                    },
                    onUnpin = {
                        val pinned = rows.filter { it.pin != null }.sortedBy { it.pin }.map { it.id }
                        if (pinned.any { it in selected }) onPins(pinned.filter { it !in selected })
                        selected.clear()
                    },
                )
            } else if (open) {
                SearchRow(query, busy, ink, focus, { query = it }, ::closeSearch)
            } else {
                TitleRow(title, connected, ink, { me?.let(onProfile) }) { open = true }
            }
            ringing?.let { call ->
                Incoming(call, { onAnswer(call) }, { onDecline(call) })
            }
            LazyColumn(Modifier.weight(1f).windowInsetsPadding(WindowInsets.navigationBars), state = listState) {
                if (shown.isEmpty() && strangers.isEmpty()) {
                    item {
                        Text(
                            stringResource(R.string.no_chats),
                            Modifier.padding(horizontal = 16.dp, vertical = 28.dp),
                            color = ink.muted,
                            fontSize = 14.sp,
                        )
                    }
                }
                items(shown, key = { it.id }) { row ->
                    val dragging = row.id == dragId
                    val place =
                        when {
                            dragging -> Modifier.zIndex(1f).graphicsLayer { translationY = dragBy }
                            selecting -> Modifier.animateItem(fadeInSpec = null, fadeOutSpec = null)
                            else -> Modifier
                        }
                    val handle =
                        if (handles && row.pin != null) {
                            Modifier.pointerInput(row.id) {
                                detectVerticalDragGestures(
                                    onDragStart = {
                                        dragOrder = current.filter { it.pin != null }.sortedBy { it.pin }.map { it.id }
                                        dragId = row.id
                                        dragBy = 0f
                                    },
                                    onDragEnd = { finishDrag(true) },
                                    onDragCancel = { finishDrag(false) },
                                ) { change, amount ->
                                    change.consume()
                                    dragBy += amount
                                    dragStep()
                                }
                            }
                        } else {
                            null
                        }
                    Conversation(
                        row,
                        label(row, people, saved),
                        me,
                        unread[row.id] ?: 0,
                        ink,
                        accentText,
                        selecting,
                        row.id in selected,
                        handle,
                        place.onSizeChanged { if (dragging || rowHeight == 0) rowHeight = it.height },
                        onProfile,
                        onLongClick = { if (row.id !in selected) selected.add(row.id) },
                    ) {
                        if (selecting) {
                            if (row.id in selected) selected.remove(row.id) else selected.add(row.id)
                        } else {
                            onOpen(row)
                        }
                    }
                }
                items(strangers, key = { "person-${it.id}" }) { account ->
                    PersonRow(account, ink) {
                        closeSearch()
                        onPerson(account)
                    }
                }
            }
        }
    }
}

/** True for a moment as each connection try starts. */
internal val LocalTrying = compositionLocalOf { false }

/** Connecting… loses its dots for a moment at each try, so a retry shows, chosen by the author. */
@Composable
internal fun statusText(link: Link): String {
    val text =
        stringResource(
            when (link) {
                Link.Waiting -> R.string.waiting_network
                Link.Connecting -> R.string.connecting
                Link.Updating -> R.string.updating
                Link.Connected -> R.string.connecting
            },
        )
    return if (link == Link.Connecting && LocalTrying.current) text.removeSuffix("…") else text
}

@Composable
private fun TitleRow(
    title: String,
    connected: Boolean,
    ink: Ink,
    onMenu: () -> Unit,
    onSearch: () -> Unit,
) {
    Row(Modifier.fillMaxWidth().height(56.dp).padding(start = 4.dp, end = 4.dp), verticalAlignment = Alignment.CenterVertically) {
        IconButton(stringResource(R.string.profile_title), onMenu) { MenuMark(ink.text) }
        Text(
            title,
            Modifier.weight(1f).padding(horizontal = 4.dp),
            color = if (connected) ink.text else ink.muted,
            fontSize = if (connected) 20.sp else 16.sp,
            fontWeight = FontWeight.Bold,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
        IconButton(stringResource(R.string.search), onSearch) { Glass(ink.text) }
    }
}

@Composable
private fun SearchRow(
    query: String,
    busy: Boolean,
    ink: Ink,
    focus: FocusRequester,
    onQuery: (String) -> Unit,
    onClose: () -> Unit,
) {
    val rtl = LocalLayoutDirection.current == LayoutDirection.Rtl
    Row(Modifier.fillMaxWidth().height(56.dp).padding(end = 12.dp), verticalAlignment = Alignment.CenterVertically) {
        IconButton(stringResource(R.string.back), onClose) { BackMark(ink.text, rtl) }
        Row(
            Modifier
                .weight(1f)
                .height(44.dp)
                .border(1.dp, ink.line, RoundedCornerShape(12.dp))
                .background(ink.field, RoundedCornerShape(12.dp))
                .padding(start = 12.dp, end = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Glass(ink.muted)
            BasicTextField(
                value = query,
                onValueChange = onQuery,
                modifier = Modifier.weight(1f).focusRequester(focus),
                textStyle = TextStyle(color = ink.text, fontSize = 16.sp),
                singleLine = true,
                keyboardOptions = KeyboardOptions(autoCorrectEnabled = false),
                decorationBox = { inner ->
                    if (query.isEmpty()) Text(stringResource(R.string.search), color = ink.muted, fontSize = 16.sp)
                    inner()
                },
            )
            if (busy) {
                CircularProgressIndicator(
                    Modifier.size(18.dp),
                    color = accent,
                    strokeWidth = 2.dp,
                    strokeCap = StrokeCap.Round,
                )
            } else if (query.isNotEmpty()) {
                IconButton(stringResource(R.string.clear_search), onClose) { Cross(ink.muted) }
            }
        }
    }
}

@Composable
private fun PersonRow(
    account: AccountBrief,
    ink: Ink,
    onClick: () -> Unit,
) {
    val name = account.display_name?.takeIf { it.isNotBlank() } ?: account.username
    Row(
        Modifier.fillMaxWidth().clickable(role = Role.Button, onClick = onClick).padding(horizontal = 16.dp, vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Avatar(account.id, name, 48.dp)
        Column(Modifier.padding(start = 12.dp).weight(1f)) {
            Text(name, color = ink.text, fontSize = 16.sp, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text(
                "@${account.username}",
                color = ink.muted,
                fontSize = 14.sp,
                maxLines = 1,
                style = TextStyle(textDirection = TextDirection.Ltr),
            )
        }
    }
}

@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun Conversation(
    row: ChatRow,
    name: String,
    me: String?,
    unread: Int,
    ink: Ink,
    accentText: Color,
    selecting: Boolean,
    selected: Boolean,
    handle: Modifier?,
    modifier: Modifier,
    onProfile: (String) -> Unit,
    onLongClick: () -> Unit,
    onClick: () -> Unit,
) {
    val timeColor = if (unread > 0) accentText else ink.muted
    // A direct chat's photo opens that person's profile, not the chat, as root AGENTS.md chose, and
    // Saved messages shows your own photo, which opens your profile.
    val person =
        when (row.type) {
            DIRECT -> other(row, me)?.account
            SELF -> me
            else -> null
        }
    Row(
        modifier
            .fillMaxWidth()
            .background(if (selected) accent.copy(alpha = 0.20f) else ink.page)
            .combinedClickable(role = Role.Button, onLongClick = onLongClick, onClick = onClick)
            .padding(start = 16.dp, end = if (handle != null) 4.dp else 16.dp, top = 10.dp, bottom = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box {
            Avatar(
                person,
                name,
                48.dp,
                if (person != null) {
                    Modifier.clip(CircleShape).combinedClickable(role = Role.Button, onLongClick = {}) { onProfile(person) }
                } else {
                    Modifier
                },
            )
            if (selecting && selected) {
                Box(
                    Modifier
                        .align(Alignment.BottomEnd)
                        .size(20.dp)
                        .clip(CircleShape)
                        .background(accent)
                        .border(2.dp, ink.page, CircleShape),
                    contentAlignment = Alignment.Center,
                ) { Glyph(Glyphs.check, onAccent, size = 12.dp, width = 3f) }
            }
        }
        Column(Modifier.padding(start = 12.dp).weight(1f)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(
                    name,
                    Modifier.weight(1f),
                    color = ink.text,
                    fontSize = 16.sp,
                    fontWeight = if (unread > 0) FontWeight.Bold else FontWeight.SemiBold,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                if (row.lastAt != null) {
                    Text(chatWhen(row.lastAt), color = timeColor, fontSize = 12.sp, fontWeight = FontWeight.Medium)
                }
            }
            Row(Modifier.padding(top = 2.dp), verticalAlignment = Alignment.CenterVertically) {
                Tick(row, me, if (seen(row, me)) accentText else ink.muted)
                Text(
                    previewText(row),
                    Modifier.weight(1f),
                    color = ink.muted,
                    fontSize = 14.sp,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                if (unread > 0) {
                    Text(
                        unread.toString(),
                        Modifier
                            .padding(start = 8.dp)
                            .height(20.dp)
                            .clip(RoundedCornerShape(10.dp))
                            .background(accent)
                            .padding(horizontal = 6.dp),
                        color = onAccent,
                        fontSize = 12.sp,
                        fontWeight = FontWeight.SemiBold,
                        textAlign = TextAlign.Center,
                    )
                } else if (row.pin != null) {
                    val pinned = stringResource(R.string.chats_pinned)
                    Box(Modifier.padding(start = 8.dp).semantics { contentDescription = pinned }) {
                        Glyph(Glyphs.pin, ink.muted, 16.dp)
                    }
                }
            }
        }
        if (handle != null) {
            val move = stringResource(R.string.chats_move)
            Box(
                Modifier.padding(start = 4.dp).size(40.dp).semantics { contentDescription = move }.then(handle),
                contentAlignment = Alignment.Center,
            ) { Glyph(Glyphs.handle, ink.muted) }
        }
    }
}

/** The list's header while selecting: the count, then Pin and Unpin at the top right. */
@Composable
private fun PinBar(
    count: Int,
    ink: Ink,
    onClose: () -> Unit,
    onPin: () -> Unit,
    onUnpin: () -> Unit,
) {
    Row(Modifier.fillMaxWidth().height(56.dp).padding(start = 4.dp, end = 4.dp), verticalAlignment = Alignment.CenterVertically) {
        IconButton(stringResource(R.string.chat_cancel), onClose) { Cross(ink.text) }
        Text(
            stringResource(R.string.chat_selected, count),
            Modifier.weight(1f).padding(start = 6.dp),
            color = ink.text,
            fontSize = 18.sp,
            fontWeight = FontWeight.Bold,
        )
        IconButton(stringResource(R.string.chats_pin), onPin) { Glyph(Glyphs.pin, ink.text) }
        IconButton(stringResource(R.string.chats_unpin), onUnpin) { Glyph(Glyphs.unpin, ink.text) }
    }
}

@Composable
private fun Incoming(
    call: Shown,
    onAnswer: () -> Unit,
    onDecline: () -> Unit,
) {
    val video = call.call?.video == true
    Row(
        Modifier.fillMaxWidth().background(accent).padding(horizontal = 16.dp, vertical = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(
            stringResource(if (video) R.string.incoming_video else R.string.incoming_voice),
            Modifier.weight(1f),
            color = onAccent,
            fontSize = 15.sp,
            fontWeight = FontWeight.SemiBold,
        )
        Text(
            stringResource(R.string.decline),
            Modifier.clickable(role = Role.Button, onClick = onDecline).padding(8.dp),
            color = onAccent,
            fontWeight = FontWeight.SemiBold,
        )
        Text(
            stringResource(R.string.answer),
            Modifier.clickable(role = Role.Button, onClick = onAnswer).padding(8.dp),
            color = onAccent,
            fontWeight = FontWeight.Bold,
        )
    }
}

@Composable
internal fun IconButton(
    label: String,
    onClick: () -> Unit,
    mark: @Composable () -> Unit,
) {
    Box(
        Modifier
            .size(40.dp)
            .clip(CircleShape)
            .semantics { contentDescription = label }
            .clickable(role = Role.Button, onClick = onClick),
        contentAlignment = Alignment.Center,
    ) { mark() }
}

internal fun label(
    row: ChatRow,
    people: Map<String, AccountBrief>,
    saved: String,
): String {
    if (row.type == 0) return saved
    val account = row.marks.firstNotNullOfOrNull { people[it.account] } ?: return ""
    return account.display_name?.takeIf { it.isNotBlank() } ?: account.username
}

@Composable
private fun previewText(row: ChatRow): String {
    if (row.preview.isNotEmpty()) return row.preview
    if (row.lastSeq == null && !row.waiting) return stringResource(R.string.no_messages)
    return ""
}

private fun other(
    row: ChatRow,
    me: String?,
) = row.marks.firstOrNull { it.account != me }

private fun seen(
    row: ChatRow,
    me: String?,
): Boolean {
    val seq = row.lastSeq ?: return false
    return when (row.type) {
        DIRECT -> (other(row, me)?.seen_seq ?: 0) >= seq
        GROUP -> row.marks.any { it.account != me && (it.seen_seq ?: 0) >= seq }
        else -> false
    }
}

private enum class TickKind { None, Clock, One, Two }

private fun tick(
    row: ChatRow,
    me: String?,
): TickKind {
    if (!row.mine && !row.waiting) return TickKind.None
    if (row.waiting || row.lastSeq == null) {
        return if (row.type == DIRECT || row.type == GROUP || row.type == 3) TickKind.Clock else TickKind.None
    }
    val seq = row.lastSeq
    return when (row.type) {
        DIRECT -> {
            val them = other(row, me)
            if (them == null || them.received_seq < seq) TickKind.One else TickKind.Two
        }
        GROUP -> TickKind.One
        else -> TickKind.None
    }
}

@Composable
private fun Tick(
    row: ChatRow,
    me: String?,
    color: Color,
) {
    when (tick(row, me)) {
        TickKind.None -> Unit
        TickKind.Clock -> Clock(color)
        TickKind.One -> Checks(color, two = false)
        TickKind.Two -> Checks(color, two = true)
    }
}

/** One check, or the same pair as before, with the right check's long arm the same length as the left check's. */
@Composable
internal fun Checks(
    color: Color,
    two: Boolean,
) {
    Canvas(Modifier.padding(end = 4.dp).width(if (two) 20.dp else 16.dp).height(16.dp)) {
        val unit = 16.dp.toPx() / 24f
        val stroke = Stroke(width = 2f * unit, cap = StrokeCap.Round, join = StrokeJoin.Round)

        fun mark(
            ax: Float,
            ay: Float,
            bx: Float,
            by: Float,
            cx: Float,
            cy: Float,
        ) {
            drawPath(
                Path().apply {
                    moveTo(ax * unit, ay * unit)
                    lineTo(bx * unit, by * unit)
                    lineTo(cx * unit, cy * unit)
                },
                color,
                style = stroke,
            )
        }
        if (two) {
            mark(18f, 6f, 7f, 17f, 2f, 12f)
            mark(25.5f, 6.5f, 14.5f, 17.5f, 13f, 16f)
        } else {
            mark(20f, 6f, 9f, 17f, 4f, 12f)
        }
    }
}

@Composable
internal fun Clock(color: Color) {
    Canvas(Modifier.size(18.dp)) {
        val stroke = Stroke(width = 2.dp.toPx(), cap = StrokeCap.Round)
        drawCircle(color, radius = size.minDimension * 0.34f, style = stroke)
        drawLine(color, center, Offset(center.x, center.y - size.height * 0.18f), stroke.width)
        drawLine(color, center, Offset(center.x + size.width * 0.12f, center.y + size.height * 0.08f), stroke.width)
    }
}

@Composable
private fun MenuMark(color: Color) {
    Canvas(Modifier.size(22.dp)) {
        val stroke = Stroke(width = 2.dp.toPx(), cap = StrokeCap.Round)
        listOf(0.28f, 0.5f, 0.72f).forEach { y ->
            drawLine(color, Offset(size.width * 0.16f, size.height * y), Offset(size.width * 0.84f, size.height * y), stroke.width)
        }
    }
}

@Composable
private fun Glass(color: Color) {
    Canvas(Modifier.size(18.dp)) {
        val stroke = Stroke(width = 2.dp.toPx(), cap = StrokeCap.Round)
        val c = Offset(size.width * 0.42f, size.height * 0.42f)
        drawCircle(color, radius = size.minDimension * 0.26f, center = c, style = stroke)
        drawLine(color, Offset(size.width * 0.62f, size.height * 0.62f), Offset(size.width * 0.84f, size.height * 0.84f), stroke.width)
    }
}

@Composable
internal fun BackMark(
    color: Color,
    rtl: Boolean,
) {
    Canvas(Modifier.size(22.dp)) {
        val stroke = Stroke(width = 2.dp.toPx(), cap = StrokeCap.Round)
        val tip = if (rtl) size.width * 0.68f else size.width * 0.32f
        val tail = if (rtl) size.width * 0.28f else size.width * 0.72f
        drawLine(color, Offset(tail, size.height * 0.5f), Offset(tip, size.height * 0.5f), stroke.width)
        drawLine(color, Offset(tip, size.height * 0.5f), Offset((tip + tail) / 2f, size.height * 0.28f), stroke.width)
        drawLine(color, Offset(tip, size.height * 0.5f), Offset((tip + tail) / 2f, size.height * 0.72f), stroke.width)
    }
}

@Composable
internal fun Cross(color: Color) {
    Canvas(Modifier.size(18.dp)) {
        val stroke = Stroke(width = 2.dp.toPx(), cap = StrokeCap.Round)
        drawLine(color, Offset(size.width * 0.25f, size.height * 0.25f), Offset(size.width * 0.75f, size.height * 0.75f), stroke.width)
        drawLine(color, Offset(size.width * 0.75f, size.height * 0.25f), Offset(size.width * 0.25f, size.height * 0.75f), stroke.width)
    }
}

@Composable
internal fun chatWhen(at: Long): String {
    val zone = ZoneId.systemDefault()
    val then = Instant.ofEpochMilli(at).atZone(zone)
    val today = LocalDate.now(zone)
    val days = ChronoUnit.DAYS.between(then.toLocalDate(), today)
    return when {
        days <= 0L -> DateTimeFormatter.ofLocalizedTime(FormatStyle.SHORT).format(then)
        days == 1L -> stringResource(R.string.yesterday)
        days < 7L -> DateTimeFormatter.ofPattern("EEE").format(then)
        else -> DateTimeFormatter.ofLocalizedDate(FormatStyle.SHORT).format(then)
    }
}
