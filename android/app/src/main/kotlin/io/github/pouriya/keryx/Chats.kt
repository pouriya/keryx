package io.github.pouriya.keryx

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.gestures.detectVerticalDragGestures
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.ime
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.layout.union
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
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
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.compositionLocalOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateMapOf
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
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.LayoutCoordinates
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.layout.onPlaced
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.layout.positionInParent
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalLayoutDirection
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.PlatformTextStyle
import androidx.compose.ui.text.TextRange
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.TextFieldValue
import androidx.compose.ui.text.style.LineHeightStyle
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextDirection
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.LayoutDirection
import androidx.compose.ui.unit.TextUnit
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
import kotlin.math.roundToInt

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
    /** The preview says what a suggestion with no text suggests, drawn after a folder icon. */
    val suggests: Boolean = false,
)

/** One account's folders, the whole arrangement in tab order, and the one to open on. */
internal data class FolderSet(
    val folders: List<Folder>,
    val default: String,
) {
    /** The folder holding every chat: the one with no name. */
    val all: Folder? get() = folders.firstOrNull { it.name == null }

    /** Folders that count towards the limit of 20: every one but All. */
    val named: Int get() = folders.count { it.name != null }

    private fun change(
        id: String,
        how: (Folder) -> Folder,
    ) = copy(folders = folders.map { if (it.id == id) how(it) else it })

    /** New pins go under the ones already pinned, in the order given. */
    fun pin(
        id: String,
        chats: List<String>,
    ) = change(id) { folder -> folder.copy(pin_list = folder.pin_list + chats.filter { it !in folder.pin_list }) }

    fun unpin(
        id: String,
        chats: List<String>,
    ) = change(id) { it.copy(pin_list = it.pin_list - chats.toSet()) }

    fun pins(
        id: String,
        order: List<String>,
    ) = change(id) { it.copy(pin_list = order) }

    /** All holds every chat already, so adding to it changes nothing. */
    fun add(
        id: String,
        chats: List<String>,
    ) = change(id) { folder ->
        val list = folder.chat_list ?: return@change folder
        folder.copy(chat_list = list + chats.filter { it !in list })
    }

    /** Out of this folder alone, and out of its pins. They stay in All. */
    fun takeOut(
        id: String,
        chats: List<String>,
    ) = change(id) { folder ->
        val list = folder.chat_list ?: return@change folder
        folder.copy(chat_list = list - chats.toSet(), pin_list = folder.pin_list - chats.toSet())
    }

    fun plus(
        id: String,
        name: String,
        chats: List<String>,
    ) = copy(folders = folders + Folder(id, name, chats, emptyList()))

    fun rename(
        id: String,
        name: String,
    ) = change(id) { it.copy(name = name) }

    /** Removing the default makes All the default. */
    fun remove(id: String) = copy(folders = folders.filter { it.id != id }, default = if (default == id) all?.id ?: default else default)

    fun order(ids: List<String>) = copy(folders = ids.mapNotNull { id -> folders.firstOrNull { it.id == id } })

    /** Where Back from a chat opened through search lands: the first folder holding it by name, A to Z, else All. */
    fun holding(chat: String): String? =
        folders
            .filter { it.name != null && it.chat_list?.contains(chat) == true }
            .sortedWith(compareBy(String.CASE_INSENSITIVE_ORDER) { it.name.orEmpty() })
            .firstOrNull()
            ?.id ?: all?.id
}

/**
 * The folders as core shows them, the open tab, and the two edit sessions: selecting rows, and the Folders
 * sheet. Each session is an arrangement edited here and sent whole on its check, so Back throws it away.
 */
internal class FolderState(
    private val send: (Command) -> Unit,
) {
    var shown by mutableStateOf<FolderSet?>(null)
        private set

    /** The open tab, kept here so it outlives the list while a chat is open. */
    var tab by mutableStateOf<String?>(null)
    var edit by mutableStateOf<FolderSet?>(null)
        private set
    var sheet by mutableStateOf<FolderSet?>(null)
        private set

    /** A string resource to show for a moment. */
    var toast by mutableStateOf<Int?>(null)
    private var editBase: FolderSet? = null
    private var sheetBase: FolderSet? = null
    private var refused: String? = null
    private var saved = false
    private var idFor: ((String) -> Unit)? = null

    val editChanged: Boolean get() = edit != null && edit != editBase
    val sheetChanged: Boolean get() = sheet != null && sheet != sheetBase

    fun startEdit() {
        val now = shown ?: return
        if (edit != null) return
        edit = now
        editBase = now
    }

    fun changeEdit(next: FolderSet) {
        if (edit != null) edit = next
    }

    fun endEdit() {
        edit = null
        editBase = null
        settleTab()
    }

    fun saveEdit() {
        val next = edit ?: return
        if (next == editBase) return endEdit()
        // Shown before the tab is settled, or a folder made in this session would not be found.
        edit = null
        editBase = null
        save(next)
    }

    fun openSheet() {
        val now = shown ?: return
        sheet = now
        sheetBase = now
    }

    fun changeSheet(next: FolderSet) {
        if (sheet != null) sheet = next
    }

    fun endSheet() {
        sheet = null
        sheetBase = null
    }

    fun saveSheet() {
        val next = sheet ?: return
        val changed = next != sheetBase
        endSheet()
        if (changed) save(next)
    }

    /** A new folder's id, which needs a connection. Nothing is saved until the session's check. */
    fun askId(then: (String) -> Unit) {
        idFor = then
        send(Command.FolderId)
    }

    private fun save(next: FolderSet) {
        saved = true
        shown = next
        settleTab()
        send(Command.Folders(next.folders, next.default))
    }

    /** The open tab must be a folder on screen, and is the default until one is chosen. */
    private fun settleTab() {
        val on = edit ?: shown ?: return
        if (on.folders.none { it.id == tab }) tab = on.default
    }

    fun apply(event: Event) {
        when (event) {
            is Event.Error -> {
                val asked = idFor
                if (asked != null) {
                    idFor = null
                    toast = if (event.code == "offline") R.string.folders_offline else refusal(event.code)
                } else {
                    refused = event.code
                }
                return
            }
            is Event.FolderId -> {
                val then = idFor
                idFor = null
                then?.invoke(event.folder)
            }
            is Event.Folders -> {
                val next = FolderSet(event.folder_list, event.default)
                val code = refused
                // Core answers a refused save with its error, then at once the server's folders.
                if (code != null && saved) {
                    saved = false
                    toast = refusal(code)
                    if (edit != null) endEdit()
                    if (sheet != null) endSheet()
                } else {
                    if (edit != null && next != editBase) {
                        endEdit()
                        toast = R.string.folders_changed
                    }
                    if (sheet != null && next != sheetBase) {
                        endSheet()
                        toast = R.string.folders_changed
                    }
                }
                shown = next
                settleTab()
            }
            else -> Unit
        }
        refused = null
    }

    fun clear() {
        shown = null
        tab = null
        edit = null
        editBase = null
        sheet = null
        sheetBase = null
        toast = null
        refused = null
        saved = false
        idFor = null
    }

    private fun refusal(code: String) =
        when (code) {
            "too_many_folders" -> R.string.folders_refused_many
            "no_permission" -> R.string.folders_refused_permission
            "unknown_folder" -> R.string.folders_refused_unknown
            else -> R.string.folders_refused
        }
}

/** The conversations, newest first. Each folder puts its own pins on top when it is shown. */
internal class ChatRows {
    val rows = mutableStateListOf<ChatRow>()

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
                    suggests = old != null && kept && old.suggests,
                ),
            )
        }
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
        // An ack is quiet: it moves neither the chat nor its preview.
        if (kind == CONTENT_ACK) return
        val suggests = kind == CONTENT_SUGGESTION && message.text.isNullOrBlank()
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
                    suggests = suggests,
                ),
            )
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
                suggests = suggests,
            )
        val newer = row.waiting || row.lastSeq == null || message.seq > row.lastSeq
        if (newer) {
            rows.removeAt(index)
            rows.add(0, next)
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
        val next = row.copy(preview = text ?: row.preview, mine = true, waiting = true, callVideo = null, suggests = false)
        rows.removeAt(index)
        rows.add(0, next)
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

    /** A new device's notice as the self chat's preview, while nothing newer is there. */
    fun notice(
        event: Event.Notice,
        preview: String,
    ) {
        val index = rows.indexOfFirst { it.id == event.chat }
        if (index < 0) return
        val row = rows[index]
        if (row.lastSeq != null && event.seq < row.lastSeq) return
        rows[index] =
            row.copy(
                lastSeq = event.seq,
                preview = preview,
                lastAt = event.saved_at,
                mine = false,
                waiting = false,
                callVideo = null,
                suggests = false,
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
                row.copy(lastSeq = null, preview = "", mine = false, waiting = false, callVideo = null, suggests = false)
            } else {
                row.copy(
                    lastSeq = previous.seq,
                    preview = preview(previous),
                    mine = previous.mine,
                    waiting = false,
                    callVideo = if (previous.kind == CALL) previous.call?.video == true else null,
                    suggests = previous.kind == CONTENT_SUGGESTION && previous.text.isNullOrBlank(),
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
private const val TOAST_FOR = 3000L
private const val MAX_FOLDERS = 20
private const val NAME_MAX = 32
private val star = Color(0xFFE0A526)
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
    folders: FolderState,
    search: MessageSearch,
    onSearch: (String) -> Unit,
    onOlder: () -> Unit,
    onHit: (Hit) -> Unit,
    onPerson: (AccountBrief) -> Unit,
    onOpen: (ChatRow) -> Unit,
    onProfile: (String) -> Unit,
    onAnswer: (Shown) -> Unit,
    onDecline: (Shown) -> Unit,
) {
    val dark = isDark()
    val ink = ink()
    val accentText = if (dark) Color(0xFFC5D4E8) else accent
    var open by remember { mutableStateOf(false) }
    var query by remember { mutableStateOf("") }
    var applied by remember { mutableStateOf("") }
    var busy by remember { mutableStateOf(false) }
    var part by remember { mutableStateOf(SearchPart.All) }
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
    LaunchedEffect(folders.toast) {
        if (folders.toast != null) {
            delay(TOAST_FOR)
            folders.toast = null
        }
    }

    fun closeSearch() {
        open = false
        query = ""
        applied = ""
        busy = false
        part = SearchPart.All
    }
    BackHandler(enabled = open) { closeSearch() }
    // A long press on a row, not its photo, starts selecting, as messages do, chosen by the author.
    // Selecting is an edit session: everything shows at once and goes out only on the check.
    val selected = remember { mutableStateListOf<String>() }
    val selecting = folders.edit != null
    val arrangement = folders.edit ?: folders.shown
    val here = arrangement?.let { a -> a.folders.firstOrNull { it.id == folders.tab } ?: a.all }
    val inAll = here?.chat_list == null

    // A session with nothing selected and nothing changed is over.
    fun settle() {
        if (selected.isEmpty() && !folders.editChanged) folders.endEdit()
    }

    fun discard() {
        selected.clear()
        folders.endEdit()
    }
    BackHandler(enabled = selecting) { discard() }
    var moving by remember { mutableStateOf(false) }
    var naming by remember { mutableStateOf<Naming?>(null) }
    var removing by remember { mutableStateOf<Folder?>(null) }
    // While a pinned row is dragged, the pinned order as it stands under the finger.
    var dragOrder by remember { mutableStateOf<List<String>?>(null) }
    var dragId by remember { mutableStateOf<String?>(null) }
    var dragBy by remember { mutableFloatStateOf(0f) }
    var rowHeight by remember { mutableIntStateOf(0) }
    val listState = rememberLazyListState()
    Bars(ink.page, dark)
    val connected = link == Link.Connected
    val saved = stringResource(R.string.saved_messages)
    val title = if (connected) serverName?.takeIf { it.isNotBlank() } ?: host else statusText(link)
    val filtering = open && !busy && applied.isNotEmpty()
    val inFolder = here?.chat_list?.toSet()?.let { set -> rows.filter { it.id in set } } ?: rows
    val byId = inFolder.associateBy { it.id }
    val pins = here?.pin_list.orEmpty()
    val pinned = dragOrder ?: pins.filter { it in byId }
    val ordered = pinned.mapNotNull { byId[it] } + inFolder.filter { it.id !in pins }
    // Search looks through every chat, whatever tab is open, chosen by the author.
    val shown =
        if (filtering) {
            // Anywhere in the name, as the server's directory matches.
            val matched = rows.filter { label(it, people, saved).contains(applied, ignoreCase = true) }
            val self = rows.firstOrNull { it.type == 0 }
            if (self != null && found.any { it.id == me } && matched.none { it.id == self.id }) {
                listOf(self) + matched
            } else {
                matched
            }
        } else {
            ordered
        }
    LaunchedEffect(shown.map { it.id }) {
        selected.retainAll(shown.map { it.id }.toSet())
        settle()
    }
    // Every pinned row gets a handle while selecting, once two or more are pinned, chosen by the author.
    val handles = selecting && !filtering && pinned.size >= 2
    // The selected chats in the list's order, which is the order new pins and folders take them in.
    val chosen = shown.filter { it.id in selected }.map { it.id }

    // A handle's gesture outlives the composition that made it, so these read the session as it is now.
    val pinnedNow by rememberUpdatedState(pinned)

    fun finishDrag(keep: Boolean) {
        val order = dragOrder
        val edit = folders.edit
        val folder = edit?.let { e -> e.folders.firstOrNull { it.id == folders.tab } ?: e.all }
        if (keep && order != null && edit != null && folder != null && order != folder.pin_list.filter { it in order }) {
            folders.changeEdit(edit.pins(folder.id, order + folder.pin_list.filter { it !in order }))
        }
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

    fun act(how: (FolderSet) -> FolderSet) {
        folders.edit?.let { folders.changeEdit(how(it)) }
        selected.clear()
        settle()
    }

    // New asks core for an id at once, and at 20 folders says so with no request, chosen by the author.
    fun newFolder(chats: List<String>?) {
        val count = (if (chats == null) folders.sheet else folders.edit)?.named ?: return
        if (count >= MAX_FOLDERS) {
            folders.toast = R.string.folders_at_most
        } else {
            naming = Naming(null, "", chats)
        }
    }
    val known = rows.flatMap { row -> row.marks.map { it.account } }.toSet()
    val strangers = if (filtering) found.filter { it.id != me && it.id !in known } else emptyList()
    val names = chosen.mapNotNull { id -> rows.firstOrNull { it.id == id } }.map { label(it, people, saved) }
    Box(Modifier.fillMaxSize().background(ink.page).windowInsetsPadding(WindowInsets.statusBars)) {
        Column(Modifier.fillMaxSize()) {
            if (selecting) {
                EditBar(selected.size, folders.editChanged, ink, onClose = ::discard) {
                    selected.clear()
                    folders.saveEdit()
                }
            } else if (open) {
                val ring = if (applied.isNotEmpty() && search.text == applied && !search.done) search.percent else null
                SearchRow(query, busy, ring, ink, focus, { query = it }, ::closeSearch)
            } else {
                TitleRow(title, connected, ink, { me?.let(onProfile) }) { open = true }
            }
            ringing?.let { call ->
                Incoming(call, { onAnswer(call) }, { onDecline(call) })
            }
            val listInsets = if (arrangement == null) Modifier.windowInsetsPadding(WindowInsets.navigationBars) else Modifier
            if (open) {
                SearchBody(
                    applied,
                    part,
                    if (filtering) shown else emptyList(),
                    strangers,
                    asking = busy || (applied.isNotEmpty() && foundFor != applied),
                    search,
                    rows,
                    people,
                    me,
                    unread,
                    ink,
                    accentText,
                    Modifier.weight(1f),
                    onPart = { part = it },
                    onRow = { row ->
                        arrangement?.holding(row.id)?.let { folders.tab = it }
                        closeSearch()
                        onOpen(row)
                    },
                    onPerson = { account ->
                        closeSearch()
                        // A new chat is in All only.
                        arrangement?.all?.let { folders.tab = it.id }
                        onPerson(account)
                    },
                    onProfile = onProfile,
                    onOlder = onOlder,
                    onHit = { hit ->
                        arrangement?.holding(hit.chat)?.let { folders.tab = it }
                        closeSearch()
                        onHit(hit)
                    },
                )
            } else {
                LazyColumn(Modifier.weight(1f).then(listInsets), state = listState) {
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
                            if (handles && row.id in pinned) {
                                Modifier.pointerInput(row.id) {
                                    detectVerticalDragGestures(
                                        onDragStart = {
                                            dragOrder = pinnedNow
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
                            row.id in pins,
                            ink,
                            accentText,
                            selecting,
                            row.id in selected,
                            handle,
                            place.onSizeChanged { if (dragging || rowHeight == 0) rowHeight = it.height },
                            onProfile,
                            onLongClick = {
                                folders.startEdit()
                                if (folders.edit != null && row.id !in selected) selected.add(row.id)
                            },
                        ) {
                            if (selecting) {
                                if (row.id in selected) selected.remove(row.id) else selected.add(row.id)
                                settle()
                            } else {
                                // Back from a chat opened through search lands on the first folder holding it.
                                if (filtering) arrangement?.holding(row.id)?.let { folders.tab = it }
                                onOpen(row)
                            }
                        }
                    }
                }
            }
            if (open) {
                val accounts = if (filtering) shown.size + strangers.size else 0
                SearchTabs(
                    part,
                    accounts.takeIf { applied.isNotEmpty() && !busy && foundFor == applied },
                    search.hits.size.takeIf { applied.isNotEmpty() && search.text == applied && search.done },
                    ink,
                    accentText,
                    dark,
                ) { part = it }
            } else if (selecting && here != null) {
                Strip(
                    inAll,
                    chosen.isNotEmpty() && chosen.all { it in pins },
                    chosen.isNotEmpty(),
                    ink,
                    onPin = {
                        val all = chosen.all { it in pins }
                        act { if (all) it.unpin(here.id, chosen) else it.pin(here.id, chosen) }
                    },
                    onMove = { moving = true },
                    onRemove = { act { it.takeOut(here.id, chosen) } },
                    onNew = { newFolder(chosen) },
                )
            }
            if (arrangement != null && !open) {
                Tabs(
                    arrangement,
                    here?.id,
                    rows,
                    unread,
                    ink,
                    accentText,
                    dark,
                    longPress = !selecting,
                    onTab = { folders.tab = it },
                    onLong = { folders.openSheet() },
                )
            }
        }
        val edit = folders.edit
        if (moving && edit != null && here != null) {
            MoveSheet(
                edit,
                here,
                chosen,
                names,
                ink,
                dark,
                accentText,
                onClose = { moving = false },
                onCopy = { to ->
                    moving = false
                    act { it.add(to, chosen) }
                },
                onMove = { to ->
                    moving = false
                    act { it.add(to, chosen).takeOut(here.id, chosen) }
                },
                onRemove = {
                    moving = false
                    act { it.takeOut(here.id, chosen) }
                },
                onNew = {
                    moving = false
                    newFolder(chosen)
                },
            )
        }
        folders.sheet?.let { sheet ->
            FolderSheet(
                sheet,
                folders.sheetChanged,
                ink,
                dark,
                accentText,
                onChange = { folders.changeSheet(it) },
                onSave = { folders.saveSheet() },
                onClose = { folders.endSheet() },
                onRename = { naming = Naming(it.id, it.name.orEmpty(), null) },
                onRemove = { removing = it },
                onNew = { newFolder(null) },
            )
        }
        naming?.let { asked ->
            val whoNames = asked.chats?.mapNotNull { id -> rows.firstOrNull { it.id == id } }?.map { label(it, people, saved) }
            NameBox(asked, whoNames, ink, dark, accentText, onCancel = { naming = null }) { name ->
                naming = null
                val folder = asked.folder
                if (folder != null) {
                    folders.sheet?.let { folders.changeSheet(it.rename(folder, name)) }
                } else if (asked.chats != null) {
                    folders.askId { id ->
                        folders.edit?.let { edit -> folders.changeEdit(edit.plus(id, name, asked.chats)) }
                        selected.clear()
                        settle()
                    }
                } else {
                    folders.askId { id -> folders.sheet?.let { sheet -> folders.changeSheet(sheet.plus(id, name, emptyList())) } }
                }
            }
        }
        removing?.let { folder ->
            val sheet = folders.sheet
            RemoveBox(folder, sheet?.default == folder.id, ink, dark, accentText, onCancel = { removing = null }) {
                removing = null
                sheet?.let { folders.changeSheet(it.remove(folder.id)) }
            }
        }
        folders.toast?.let { text ->
            Text(
                stringResource(text),
                Modifier
                    .align(Alignment.BottomCenter)
                    .windowInsetsPadding(WindowInsets.navigationBars)
                    .padding(start = 16.dp, end = 16.dp, bottom = 64.dp)
                    .clip(RoundedCornerShape(12.dp))
                    .background(Color(0xFF2B333D))
                    .padding(horizontal = 14.dp, vertical = 11.dp),
                color = Color.White,
                fontSize = 13.5.sp,
            )
        }
    }
}

/** The name box's errand: a folder to rename, or a new one, from these chats or from the Folders sheet. */
private data class Naming(
    val folder: String?,
    val start: String,
    val chats: List<String>?,
)

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
    ring: Int?,
    ink: Ink,
    focus: FocusRequester,
    onQuery: (String) -> Unit,
    onClose: () -> Unit,
) {
    val rtl = LocalLayoutDirection.current == LayoutDirection.Rtl
    Row(Modifier.fillMaxWidth().height(56.dp).padding(end = 12.dp), verticalAlignment = Alignment.CenterVertically) {
        IconButton(stringResource(R.string.back), onClose, 48.dp) { BackMark(ink.text, rtl) }
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
            if (ring != null) {
                Ring(ring, ink)
            } else if (busy) {
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
    tag: String? = null,
    onClick: () -> Unit,
) {
    val name = account.display_name?.takeIf { it.isNotBlank() } ?: account.username
    Row(
        Modifier.fillMaxWidth().clickable(role = Role.Button, onClick = onClick).padding(horizontal = 16.dp, vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Avatar(account.id, name, 48.dp)
        Column(Modifier.padding(start = 12.dp).weight(1f)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(
                    name,
                    Modifier.weight(1f, fill = false),
                    color = ink.text,
                    fontSize = 16.sp,
                    fontWeight = FontWeight.SemiBold,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                if (tag != null) {
                    Text(
                        tag,
                        Modifier
                            .padding(start = 8.dp)
                            .border(1.dp, ink.line, RoundedCornerShape(6.dp))
                            .padding(horizontal = 5.dp),
                        color = if (isDark()) Color(0xFFC5D4E8) else accent,
                        fontSize = 11.sp,
                        fontWeight = FontWeight.SemiBold,
                    )
                }
            }
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
    pinned: Boolean,
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
                54.dp,
                if (person != null) {
                    Modifier.clip(CircleShape).combinedClickable(role = Role.Button, onLongClick = {}) { onProfile(person) }
                } else {
                    Modifier
                },
            )
            if (row.type == DIRECT && person != null && !(selecting && selected)) {
                PresenceDot(LocalProfiles.current?.presence?.get(person)?.state, 10.dp, 2.dp, ink.page, Modifier.align(Alignment.BottomEnd))
            }
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
                    fontSize = 17.sp,
                    fontWeight = if (unread > 0) FontWeight.Bold else FontWeight.SemiBold,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                if (row.lastAt != null) {
                    Text(chatWhen(row.lastAt), color = timeColor, fontSize = 13.sp, fontWeight = FontWeight.Medium)
                }
            }
            Row(Modifier.padding(top = 2.dp), verticalAlignment = Alignment.CenterVertically) {
                Tick(row, me, if (seen(row, me)) accentText else ink.muted)
                if (row.suggests && row.preview.isNotEmpty()) {
                    Box(Modifier.padding(end = 4.dp)) { Glyph(Glyphs.folder, ink.muted, 16.dp) }
                }
                Text(
                    previewText(row),
                    Modifier.weight(1f),
                    color = ink.muted,
                    fontSize = 15.sp,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                if (unread > 0) {
                    Text(
                        unread.toString(),
                        Modifier
                            .padding(start = 8.dp)
                            .height(22.dp)
                            .clip(RoundedCornerShape(11.dp))
                            .background(accent)
                            .padding(horizontal = 7.dp),
                        color = onAccent,
                        fontSize = 13.sp,
                        fontWeight = FontWeight.SemiBold,
                        textAlign = TextAlign.Center,
                    )
                } else if (pinned) {
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

/** The list's header while selecting: X, the count, and the check, which fills once something changed. */
@Composable
private fun EditBar(
    count: Int,
    changed: Boolean,
    ink: Ink,
    onClose: () -> Unit,
    onSave: () -> Unit,
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
        SaveButton(changed, ink, onSave)
    }
}

/** The check that saves a session: filled once something changed, quiet before. */
@Composable
private fun SaveButton(
    changed: Boolean,
    ink: Ink,
    onSave: () -> Unit,
) {
    val label = stringResource(R.string.folders_save)
    Box(
        Modifier
            .size(40.dp)
            .clip(CircleShape)
            .background(if (changed) accent else Color.Transparent)
            .semantics { contentDescription = label }
            .clickable(enabled = changed, role = Role.Button, onClick = onSave)
            .graphicsLayer { alpha = if (changed) 1f else 0.35f },
        contentAlignment = Alignment.Center,
    ) { Glyph(Glyphs.check, if (changed) onAccent else ink.text, width = 2.4f) }
}

/** The actions while selecting, at the bottom: in All, Pin, Copy and New; inside a folder, Move and Remove too. */
@Composable
private fun Strip(
    inAll: Boolean,
    unpin: Boolean,
    enabled: Boolean,
    ink: Ink,
    onPin: () -> Unit,
    onMove: () -> Unit,
    onRemove: () -> Unit,
    onNew: () -> Unit,
) {
    Box(Modifier.fillMaxWidth().height(1.dp).background(ink.line))
    Row(Modifier.fillMaxWidth().padding(top = 6.dp, bottom = 4.dp), horizontalArrangement = Arrangement.SpaceAround) {
        StripButton(
            if (unpin) Glyphs.unpin else Glyphs.pin,
            stringResource(if (unpin) R.string.chats_unpin else R.string.chats_pin),
            enabled,
            ink,
            onPin,
        )
        if (!inAll) StripButton(Glyphs.move, stringResource(R.string.chats_move), enabled, ink, onMove)
        StripButton(Glyphs.copy, stringResource(R.string.folders_copy), enabled, ink, onMove)
        if (!inAll) StripButton(Glyphs.folderOut, stringResource(R.string.folders_remove), enabled, ink, onRemove)
        StripButton(Glyphs.folderPlus, stringResource(R.string.folders_new), enabled, ink, onNew)
    }
}

@Composable
private fun StripButton(
    icon: List<String>,
    label: String,
    enabled: Boolean,
    ink: Ink,
    onClick: () -> Unit,
) {
    Column(
        Modifier
            .width(64.dp)
            .clip(RoundedCornerShape(10.dp))
            .clickable(enabled = enabled, role = Role.Button, onClick = onClick)
            .graphicsLayer { alpha = if (enabled) 1f else 0.35f }
            .padding(vertical = 4.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Glyph(icon, ink.text)
        Text(label, color = ink.text, fontSize = 11.5.sp, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
    }
}

/** The folders as tabs at the bottom, by the thumb. Each counts its unread chats, then +99. */
@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun Tabs(
    arrangement: FolderSet,
    on: String?,
    rows: List<ChatRow>,
    unread: Map<String, Int>,
    ink: Ink,
    accentText: Color,
    dark: Boolean,
    longPress: Boolean,
    onTab: (String) -> Unit,
    onLong: () -> Unit,
) {
    val allName = stringResource(R.string.folders_all)
    val scroll = rememberScrollState()
    var shownWidth by remember { mutableIntStateOf(0) }
    // Where each tab sits in the row, so the open one can be scrolled into sight.
    val placed = remember { mutableStateMapOf<String, IntRange>() }
    val margin = with(LocalDensity.current) { 8.dp.roundToPx() }
    val openAt = on?.let { placed[it] }
    // The open tab always shows, chosen by the author, also when Back from search opened one out of sight.
    LaunchedEffect(on, openAt, shownWidth) {
        val at = openAt ?: return@LaunchedEffect
        if (shownWidth == 0) return@LaunchedEffect
        val left = at.first
        val right = at.last + 2 * margin
        when {
            left < scroll.value -> scroll.animateScrollTo(left)
            right > scroll.value + shownWidth -> scroll.animateScrollTo(right - shownWidth)
        }
    }
    Column(Modifier.fillMaxWidth().background(ink.page).windowInsetsPadding(WindowInsets.navigationBars)) {
        Box(Modifier.fillMaxWidth().height(1.dp).background(ink.line))
        Row(
            Modifier
                .fillMaxWidth()
                .onSizeChanged { shownWidth = it.width }
                .horizontalScroll(scroll)
                .padding(horizontal = 8.dp, vertical = 6.dp),
            horizontalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            for (folder in arrangement.folders) {
                val chats = folder.chat_list?.toSet()
                val count = rows.count { (chats == null || it.id in chats) && (unread[it.id] ?: 0) > 0 }
                val chosen = folder.id == on
                Row(
                    Modifier
                        .onPlaced { place ->
                            val x = place.positionInParent().x.toInt()
                            val span = x..(x + place.size.width)
                            if (placed[folder.id] != span) placed[folder.id] = span
                        }
                        // A short name like All still makes a tab big enough for a thumb, chosen by the author.
                        .widthIn(min = 64.dp)
                        .clip(RoundedCornerShape(10.dp))
                        .background(if (chosen) accent.copy(alpha = if (dark) 0.28f else 0.14f) else Color.Transparent)
                        .combinedClickable(
                            role = Role.Tab,
                            onLongClick = if (longPress) onLong else null,
                        ) { onTab(folder.id) }
                        .padding(horizontal = 10.dp, vertical = 6.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(5.dp, Alignment.CenterHorizontally),
                ) {
                    Text(
                        folder.name ?: allName,
                        Modifier.widthIn(max = 160.dp),
                        color = if (chosen) accentText else ink.muted,
                        fontSize = 13.sp,
                        fontWeight = FontWeight.SemiBold,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                    if (count > 0) {
                        // The circle centres the number: a Text given a height keeps its line at the top.
                        Box(
                            Modifier
                                .height(18.dp)
                                .widthIn(min = 18.dp)
                                .clip(RoundedCornerShape(9.dp))
                                .background(accent)
                                .padding(horizontal = 5.dp),
                            contentAlignment = Alignment.Center,
                        ) {
                            Text(
                                if (count > 99) "+99" else count.toString(),
                                color = onAccent,
                                fontSize = 11.sp,
                                fontWeight = FontWeight.Bold,
                                textAlign = TextAlign.Center,
                                style = badgeText,
                            )
                        }
                    }
                }
            }
        }
    }
}

/** Who a sheet or box is about: one name, two, or two and how many others, each name cut short on its own. */
@Composable
private fun Who(
    names: List<String>,
    color: Color,
    size: TextUnit,
    weight: FontWeight,
    modifier: Modifier = Modifier,
    tail: String? = null,
) {
    Row(modifier) {
        val style = TextStyle(color = color, fontSize = size, fontWeight = weight)
        if (names.isNotEmpty()) {
            Text(
                names[0],
                Modifier.weight(1f, fill = false),
                style = style,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
        if (names.size >= 2) {
            Text(
                stringResource(if (names.size == 2) R.string.folders_who_and else R.string.folders_who_comma),
                style = style,
                maxLines = 1,
                softWrap = false,
            )
            Text(names[1], Modifier.weight(1f, fill = false), style = style, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
        if (names.size > 2) {
            val others = names.size - 2
            Text(pluralStringResource(R.plurals.folders_who_others, others, others), style = style, maxLines = 1, softWrap = false)
        }
        if (tail != null) Text(" $tail", style = style, maxLines = 1, softWrap = false)
    }
}

/** A sheet at the bottom over a dimmed list. Back or the dim throws it away. */
@Composable
private fun BottomSheet(
    dark: Boolean,
    onClose: () -> Unit,
    content: @Composable () -> Unit,
) {
    BackHandler(onBack = onClose)
    val scrim = remember { MutableInteractionSource() }
    Box(Modifier.fillMaxSize().background(Color(0x6B000000)).clickable(scrim, null, onClick = onClose))
    Box(Modifier.fillMaxSize(), contentAlignment = Alignment.BottomCenter) {
        val block = remember { MutableInteractionSource() }
        Column(
            Modifier
                .fillMaxWidth()
                .clip(RoundedCornerShape(topStart = 18.dp, topEnd = 18.dp))
                .background(if (dark) Color(0xFF1E2833) else Color.White)
                .clickable(block, null) {}
                .windowInsetsPadding(WindowInsets.navigationBars)
                .padding(top = 8.dp, bottom = 18.dp),
        ) {
            Box(
                Modifier
                    .padding(bottom = 8.dp)
                    .size(36.dp, 4.dp)
                    .clip(RoundedCornerShape(2.dp))
                    .background(if (dark) Color(0xFF3A4656) else Color(0xFFD3D8E0))
                    .align(Alignment.CenterHorizontally),
            )
            content()
        }
    }
}

/** Move and Copy's sheet: the other folders, each with what it would do, then Remove and New folder. */
@Composable
private fun MoveSheet(
    arrangement: FolderSet,
    here: Folder,
    chosen: List<String>,
    names: List<String>,
    ink: Ink,
    dark: Boolean,
    accentText: Color,
    onClose: () -> Unit,
    onCopy: (String) -> Unit,
    onMove: (String) -> Unit,
    onRemove: () -> Unit,
    onNew: () -> Unit,
) {
    val inAll = here.chat_list == null
    BottomSheet(dark, onClose) {
        Who(names, ink.text, 16.sp, FontWeight.Bold, Modifier.padding(start = 16.dp, end = 16.dp, top = 2.dp, bottom = 8.dp))
        here.name?.let { name ->
            Text(
                stringResource(R.string.folders_in, name),
                Modifier.padding(start = 16.dp, end = 16.dp, bottom = 10.dp),
                color = ink.muted,
                fontSize = 13.sp,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
        Column(Modifier.heightIn(max = 420.dp).verticalScroll(rememberScrollState())) {
            for (folder in arrangement.folders) {
                val name = folder.name ?: continue
                if (folder.id == here.id) continue
                val list = folder.chat_list.orEmpty()
                Row(
                    Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 9.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(12.dp),
                ) {
                    Glyph(Glyphs.folder, ink.text)
                    Column(Modifier.weight(1f)) {
                        Text(name, color = ink.text, fontSize = 15.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
                        Text(
                            pluralStringResource(R.plurals.folders_chats, list.size, list.size),
                            color = ink.muted,
                            fontSize = 12.5.sp,
                            maxLines = 1,
                        )
                    }
                    if (chosen.all { it in list }) {
                        Text(stringResource(R.string.folders_already_in), color = ink.muted, fontSize = 12.5.sp)
                    } else {
                        if (!inAll) Chip(stringResource(R.string.chats_move), false, ink, accentText) { onMove(folder.id) }
                        Chip(stringResource(R.string.folders_copy), true, ink, accentText) { onCopy(folder.id) }
                    }
                }
            }
            here.name?.let { name ->
                SheetRow(Glyphs.folderOut, stringResource(R.string.folders_remove_from, name), ink.danger, onRemove)
            }
            SheetRow(
                Glyphs.folderPlus,
                stringResource(if (chosen.size == 1) R.string.folders_new_with_it else R.string.folders_new_with_these),
                accentText,
                onNew,
            )
        }
    }
}

@Composable
private fun Chip(
    label: String,
    fill: Boolean,
    ink: Ink,
    accentText: Color,
    onClick: () -> Unit,
) {
    Text(
        label,
        Modifier
            .clip(RoundedCornerShape(8.dp))
            .background(if (fill) accent else Color.Transparent)
            .border(1.dp, if (fill) accent else ink.line, RoundedCornerShape(8.dp))
            .clickable(role = Role.Button, onClick = onClick)
            .padding(horizontal = 10.dp, vertical = 5.dp),
        color = if (fill) onAccent else accentText,
        fontSize = 12.sp,
        fontWeight = FontWeight.SemiBold,
    )
}

@Composable
private fun SheetRow(
    icon: List<String>,
    label: String,
    color: Color,
    onClick: () -> Unit,
) {
    Row(
        Modifier.fillMaxWidth().clickable(role = Role.Button, onClick = onClick).padding(horizontal = 16.dp, vertical = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Glyph(icon, color)
        Text(
            label,
            Modifier.weight(1f),
            color = color,
            fontSize = 15.sp,
            fontWeight = FontWeight.SemiBold,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
    }
}

/** The Folders sheet, from a long press on a tab: its own session, saved with its check. */
@Composable
private fun FolderSheet(
    arrangement: FolderSet,
    changed: Boolean,
    ink: Ink,
    dark: Boolean,
    accentText: Color,
    onChange: (FolderSet) -> Unit,
    onSave: () -> Unit,
    onClose: () -> Unit,
    onRename: (Folder) -> Unit,
    onRemove: (Folder) -> Unit,
    onNew: () -> Unit,
) {
    val allName = stringResource(R.string.folders_all)
    var dragId by remember { mutableStateOf<String?>(null) }
    var dragBy by remember { mutableFloatStateOf(0f) }

    // The finger is followed on the screen, not in its handle, which moves with the row it drags.
    var fingerFrom by remember { mutableFloatStateOf(0f) }
    var rowHeight by remember { mutableIntStateOf(0) }
    val current by rememberUpdatedState(arrangement)
    val ids = arrangement.folders.map { it.id }

    // While dragging the rows keep their order and the ones passed slide aside, since moving the
    // held row in the layout would break its own gesture. The order changes once, when it lets go.
    val from = ids.indexOf(dragId)
    val height = rowHeight.toFloat()
    val to = if (from < 0 || height <= 0f) from else (from + (dragBy / height).roundToInt()).coerceIn(0, ids.lastIndex)

    fun finish(keep: Boolean) {
        val now = current.folders.map { it.id }
        val at = now.indexOf(dragId)
        val step = rowHeight.toFloat()
        if (keep && at >= 0 && step > 0f) {
            val goes = (at + (dragBy / step).roundToInt()).coerceIn(0, now.lastIndex)
            if (goes != at) onChange(current.order(now.toMutableList().apply { add(goes, removeAt(at)) }))
        }
        dragId = null
        dragBy = 0f
    }

    BottomSheet(dark, onClose) {
        Row(Modifier.fillMaxWidth().padding(start = 16.dp, end = 8.dp, bottom = 6.dp), verticalAlignment = Alignment.CenterVertically) {
            Text(
                stringResource(R.string.folders_title),
                Modifier.weight(1f),
                color = ink.text,
                fontSize = 16.sp,
                fontWeight = FontWeight.Bold,
            )
            SaveButton(changed, ink, onSave)
        }
        Column(Modifier.heightIn(max = 460.dp).verticalScroll(rememberScrollState(), enabled = dragId == null)) {
            for (id in ids) {
                val folder = arrangement.folders.firstOrNull { it.id == id } ?: continue
                // Keyed, so each row keeps its own handle and state when folders come, go or move.
                key(id) {
                    val dragging = id == dragId
                    val index = ids.indexOf(id)
                    val aside =
                        when {
                            dragging || from < 0 -> 0f
                            index in (from + 1)..to -> -height
                            index in to until from -> height
                            else -> 0f
                        }
                    val handleAt = remember { arrayOfNulls<LayoutCoordinates>(1) }
                    val move = stringResource(R.string.chats_move)
                    Row(
                        (
                            if (dragging) {
                                Modifier.zIndex(1f).graphicsLayer {
                                    translationY = dragBy.coerceIn(-from * height, (ids.lastIndex - from) * height)
                                }
                            } else {
                                Modifier.graphicsLayer { translationY = aside }
                            }
                        )
                            .fillMaxWidth()
                            .background(if (dragging) (if (dark) Color(0xFF1E2833) else Color.White) else Color.Transparent)
                            .onSizeChanged { if (dragging || rowHeight == 0) rowHeight = it.height }
                            .padding(start = 4.dp, end = 8.dp, top = 2.dp, bottom = 2.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Box(
                            Modifier
                                .size(40.dp)
                                .semantics { contentDescription = move }
                                .onGloballyPositioned { handleAt[0] = it }
                                .pointerInput(id) {
                                    detectVerticalDragGestures(
                                        onDragStart = { from ->
                                            fingerFrom = handleAt[0]?.localToRoot(from)?.y ?: 0f
                                            dragId = id
                                            dragBy = 0f
                                        },
                                        onDragEnd = { finish(true) },
                                        onDragCancel = { finish(false) },
                                    ) { change, _ ->
                                        change.consume()
                                        val y = handleAt[0]?.localToRoot(change.position)?.y ?: return@detectVerticalDragGestures
                                        dragBy = y - fingerFrom
                                    }
                                },
                            contentAlignment = Alignment.Center,
                        ) { Glyph(Glyphs.handle, ink.muted, 20.dp) }
                        Text(
                            folder.name ?: allName,
                            Modifier.weight(1f).padding(start = 4.dp),
                            color = ink.text,
                            fontSize = 15.sp,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                        val isDefault = arrangement.default == id
                        IconButton(stringResource(R.string.folders_default), { onChange(arrangement.copy(default = id)) }) {
                            Glyph(Glyphs.star, if (isDefault) star else ink.muted, 20.dp, filled = isDefault)
                        }
                        if (folder.name != null) {
                            IconButton(
                                stringResource(R.string.folders_rename),
                                { onRename(folder) },
                            ) { Glyph(Glyphs.edit, ink.muted, 19.dp) }
                            IconButton(
                                stringResource(R.string.folders_remove),
                                { onRemove(folder) },
                            ) { Glyph(Glyphs.trash, ink.danger, 19.dp) }
                        } else {
                            Box(Modifier.width(80.dp))
                        }
                    }
                }
            }
            SheetRow(Glyphs.plus, stringResource(R.string.folders_new_folder), accentText, onNew)
        }
    }
}

/** A badge's number, as tall as its digits, so the circle centres them and not the font's spare room. */
private val badgeText =
    TextStyle(
        lineHeight = 11.sp,
        platformStyle = PlatformTextStyle(includeFontPadding = false),
        lineHeightStyle = LineHeightStyle(LineHeightStyle.Alignment.Center, LineHeightStyle.Trim.Both),
    )

/** A box in the middle, above the keyboard, over a dimmed screen. */
@Composable
internal fun Dialog(
    dark: Boolean,
    onCancel: () -> Unit,
    content: @Composable () -> Unit,
) {
    BackHandler(onBack = onCancel)
    val scrim = remember { MutableInteractionSource() }
    Box(
        Modifier
            .fillMaxSize()
            .background(Color(0x6B000000))
            .clickable(scrim, null, onClick = onCancel)
            .imePadding()
            .padding(horizontal = 20.dp),
        contentAlignment = Alignment.Center,
    ) {
        val block = remember { MutableInteractionSource() }
        Column(
            Modifier
                .fillMaxWidth()
                .clip(RoundedCornerShape(16.dp))
                .background(if (dark) Color(0xFF1E2833) else Color.White)
                .clickable(block, null) {}
                .padding(18.dp),
        ) { content() }
    }
}

@Composable
private fun DialogButtons(
    cancel: String,
    done: String,
    doneColor: Color,
    enabled: Boolean,
    accentText: Color,
    onCancel: () -> Unit,
    onDone: () -> Unit,
) {
    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.End)) {
        Text(
            cancel,
            Modifier.clip(
                RoundedCornerShape(10.dp),
            ).clickable(role = Role.Button, onClick = onCancel).padding(horizontal = 10.dp, vertical = 8.dp),
            color = accentText,
            fontSize = 15.sp,
            fontWeight = FontWeight.SemiBold,
        )
        Text(
            done,
            Modifier
                .clip(RoundedCornerShape(10.dp))
                .clickable(enabled = enabled, role = Role.Button, onClick = onDone)
                .graphicsLayer { alpha = if (enabled) 1f else 0.4f }
                .padding(horizontal = 10.dp, vertical = 8.dp),
            color = doneColor,
            fontSize = 15.sp,
            fontWeight = FontWeight.SemiBold,
        )
    }
}

/** A folder's name: 1 to 32 characters, checked here, as the root AGENTS.md says. */
@Composable
private fun NameBox(
    asked: Naming,
    who: List<String>?,
    ink: Ink,
    dark: Boolean,
    accentText: Color,
    onCancel: () -> Unit,
    onDone: (String) -> Unit,
) {
    // The cursor starts after the name, chosen by the author, so a rename goes on from its end.
    var field by remember { mutableStateOf(TextFieldValue(asked.start, TextRange(asked.start.length))) }
    val name = field.text
    val focus = remember { FocusRequester() }
    LaunchedEffect(Unit) { runCatching { focus.requestFocus() } }
    val trimmed = name.trim()
    val length = name.codePointCount(0, name.length)
    Dialog(dark, onCancel) {
        Text(
            stringResource(if (asked.folder != null) R.string.folders_rename_title else R.string.folders_new_folder),
            Modifier.padding(bottom = 12.dp),
            color = ink.text,
            fontSize = 17.sp,
            fontWeight = FontWeight.Bold,
        )
        BasicTextField(
            value = field,
            onValueChange = {
                val kept = cut(it.text)
                field =
                    if (kept == it.text) {
                        it
                    } else {
                        TextFieldValue(kept, TextRange(minOf(it.selection.start, kept.length), minOf(it.selection.end, kept.length)))
                    }
            },
            modifier =
                Modifier
                    .fillMaxWidth()
                    .focusRequester(focus)
                    .border(1.5.dp, accent, RoundedCornerShape(12.dp))
                    .padding(horizontal = 12.dp, vertical = 12.dp),
            textStyle = TextStyle(color = ink.text, fontSize = 16.sp),
            cursorBrush = SolidColor(accent),
            singleLine = true,
            keyboardOptions = KeyboardOptions(autoCorrectEnabled = true),
        )
        Text(
            "$length / $NAME_MAX",
            Modifier.fillMaxWidth().padding(top = 4.dp, bottom = 12.dp),
            color = ink.muted,
            fontSize = 12.sp,
            textAlign = TextAlign.End,
        )
        if (!who.isNullOrEmpty()) {
            Who(
                who,
                ink.muted,
                13.sp,
                FontWeight.Normal,
                Modifier.padding(bottom = 14.dp),
                tail = pluralStringResource(R.plurals.folders_go_in, who.size),
            )
        }
        DialogButtons(
            stringResource(R.string.chat_cancel),
            stringResource(if (asked.folder != null) R.string.folders_save else R.string.folders_create),
            accentText,
            trimmed.isNotEmpty(),
            accentText,
            onCancel,
        ) { onDone(trimmed) }
    }
}

/** At most 32 characters, counting what a person sees as one character the way Kotlin's code points do. */
private fun cut(text: String): String {
    if (text.codePointCount(0, text.length) <= NAME_MAX) return text
    return text.substring(0, text.offsetByCodePoints(0, NAME_MAX))
}

/** Removing asks once, inside the Folders sheet's session, so Back before its check still undoes it. */
@Composable
private fun RemoveBox(
    folder: Folder,
    wasDefault: Boolean,
    ink: Ink,
    dark: Boolean,
    accentText: Color,
    onCancel: () -> Unit,
    onRemove: () -> Unit,
) {
    Dialog(dark, onCancel) {
        Text(
            stringResource(R.string.folders_remove_title, folder.name.orEmpty()),
            Modifier.padding(bottom = 12.dp),
            color = ink.text,
            fontSize = 17.sp,
            fontWeight = FontWeight.Bold,
            maxLines = 2,
            overflow = TextOverflow.Ellipsis,
        )
        val body = stringResource(R.string.folders_remove_body)
        val default = stringResource(R.string.folders_remove_default)
        Text(
            if (wasDefault) "$body $default" else body,
            Modifier.padding(bottom = 16.dp),
            color = ink.text,
            fontSize = 14.5.sp,
        )
        DialogButtons(
            stringResource(R.string.chat_cancel),
            stringResource(R.string.folders_remove),
            ink.danger,
            true,
            accentText,
            onCancel,
            onRemove,
        )
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
    size: Dp = 40.dp,
    mark: @Composable () -> Unit,
) {
    Box(
        Modifier
            .size(size)
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
    // One path with rounded joins, so the shaft and both arms meet at the tip with no seam.
    Canvas(Modifier.size(26.dp)) {
        val unit = size.width / 24f

        fun x(at: Float) = (if (rtl) 24f - at else at) * unit
        val path =
            Path().apply {
                moveTo(x(11.5f), 5.5f * unit)
                lineTo(x(5f), 12f * unit)
                lineTo(x(11.5f), 18.5f * unit)
                moveTo(x(5f), 12f * unit)
                lineTo(x(19f), 12f * unit)
            }
        drawPath(path, color, style = Stroke(width = 2.4.dp.toPx(), cap = StrokeCap.Round, join = StrokeJoin.Round))
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

/** The search's tabs, chosen by the author: All, then Accounts, then Messages. */
internal enum class SearchPart { All, Accounts, Messages }

/** How many of each part All shows before See all. */
private const val ALL_SHOWS = 3

/** How much of a hit's text before the match stays, so the match lands in its two lines. */
private const val HIT_LEAD = 48

/** The message part of a search: what was found so far, and how far the archive has been looked through. */
internal class MessageSearch {
    var text by mutableStateOf("")
        private set
    val hits = mutableStateListOf<Hit>()
    private var scanned by mutableStateOf(0L)
    private var total by mutableStateOf(0L)
    var done by mutableStateOf(true)
        private set
    var more by mutableStateOf(false)
        private set

    /** How much of the archive has been looked through, 0 to 100. */
    val percent: Int
        get() = if (total <= 0) 0 else (scanned * 100 / total).toInt().coerceIn(0, 100)

    fun start(text: String) {
        this.text = text
        hits.clear()
        scanned = 0
        total = 0
        done = text.isEmpty()
        more = false
    }

    fun older() {
        if (more) done = false
    }

    /** One read's hits, unless they belong to a search typed over since. */
    fun take(event: Event.MessageHits) {
        if (event.text != text) return
        hits.addAll(event.hit_list)
        scanned = event.scanned
        total = event.total
        done = event.done
        more = event.more
    }
}

/** A ring filling 0 to 100 with the percent inside, while the archive is looked through, chosen by the author. */
@Composable
private fun Ring(
    percent: Int,
    ink: Ink,
) {
    val tint = if (isDark()) Color(0xFFC5D4E8) else accent
    Box(Modifier.size(30.dp), contentAlignment = Alignment.Center) {
        Canvas(Modifier.size(28.dp)) {
            val stroke = 2.5.dp.toPx()
            drawCircle(ink.line, style = Stroke(stroke))
            drawArc(accent, -90f, 360f * percent / 100f, false, style = Stroke(stroke, cap = StrokeCap.Round))
        }
        Text("$percent%", color = tint, fontSize = 8.5.sp, fontWeight = FontWeight.Bold, maxLines = 1)
    }
}

/** The search's tabs at the bottom, by the thumb, as the folders' are; a count shows once its part is finished. */
@Composable
private fun SearchTabs(
    part: SearchPart,
    accounts: Int?,
    messages: Int?,
    ink: Ink,
    accentText: Color,
    dark: Boolean,
    onPart: (SearchPart) -> Unit,
) {
    Column(Modifier.fillMaxWidth().background(ink.page).windowInsetsPadding(WindowInsets.navigationBars.union(WindowInsets.ime))) {
        Box(Modifier.fillMaxWidth().height(1.dp).background(ink.line))
        Row(Modifier.fillMaxWidth().padding(horizontal = 8.dp, vertical = 6.dp), horizontalArrangement = Arrangement.spacedBy(4.dp)) {
            for (each in SearchPart.entries) {
                val chosen = each == part
                val count =
                    when (each) {
                        SearchPart.All -> null
                        SearchPart.Accounts -> accounts
                        SearchPart.Messages -> messages
                    }
                Row(
                    Modifier
                        .weight(1f)
                        .clip(RoundedCornerShape(10.dp))
                        .background(if (chosen) accent.copy(alpha = if (dark) 0.28f else 0.14f) else Color.Transparent)
                        .clickable(role = Role.Tab) { onPart(each) }
                        .padding(vertical = 7.dp),
                    horizontalArrangement = Arrangement.Center,
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Text(
                        stringResource(
                            when (each) {
                                SearchPart.All -> R.string.search_all
                                SearchPart.Accounts -> R.string.search_accounts
                                SearchPart.Messages -> R.string.search_messages
                            },
                        ),
                        color = if (chosen) accentText else ink.muted,
                        fontSize = 13.5.sp,
                        fontWeight = FontWeight.SemiBold,
                        maxLines = 1,
                    )
                    if (count != null) {
                        Text(
                            count.toString(),
                            Modifier.padding(start = 4.dp),
                            color = (if (chosen) accentText else ink.muted).copy(alpha = 0.75f),
                            fontSize = 12.sp,
                            fontWeight = FontWeight.SemiBold,
                        )
                    }
                }
            }
        }
    }
}

/** A heading in the results, with See all at its end when there is more than shows. */
@Composable
private fun SearchHead(
    text: String,
    ink: Ink,
    accentText: Color,
    onAll: (() -> Unit)?,
) {
    Row(
        Modifier.fillMaxWidth().padding(start = 16.dp, end = 16.dp, top = 12.dp, bottom = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(
            text.uppercase(),
            Modifier.weight(1f),
            color = ink.muted,
            fontSize = 12.5.sp,
            fontWeight = FontWeight.Bold,
            letterSpacing = 0.4.sp,
        )
        if (onAll != null) {
            Text(
                stringResource(R.string.search_see_all),
                Modifier.clip(RoundedCornerShape(6.dp)).clickable(role = Role.Button, onClick = onAll).padding(4.dp),
                color = accentText,
                fontSize = 13.sp,
                fontWeight = FontWeight.SemiBold,
            )
        }
    }
}

/** What a search shows above its tabs: accounts, messages or both, people first. */
@Composable
private fun SearchBody(
    text: String,
    part: SearchPart,
    chats: List<ChatRow>,
    strangers: List<AccountBrief>,
    asking: Boolean,
    search: MessageSearch,
    rows: List<ChatRow>,
    people: Map<String, AccountBrief>,
    me: String?,
    unread: Map<String, Int>,
    ink: Ink,
    accentText: Color,
    modifier: Modifier,
    onPart: (SearchPart) -> Unit,
    onRow: (ChatRow) -> Unit,
    onPerson: (AccountBrief) -> Unit,
    onProfile: (String) -> Unit,
    onOlder: () -> Unit,
    onHit: (Hit) -> Unit,
) {
    val saved = stringResource(R.string.saved_messages)
    val newTag = stringResource(R.string.search_new)
    val hits = if (search.text == text) search.hits.toList() else emptyList()
    val scanning = search.text == text && !search.done
    if (text.isEmpty()) {
        Column(modifier.fillMaxWidth().padding(horizontal = 28.dp, vertical = 60.dp), horizontalAlignment = Alignment.CenterHorizontally) {
            Text(stringResource(R.string.search), color = ink.text, fontSize = 16.sp, fontWeight = FontWeight.SemiBold)
            Text(
                stringResource(R.string.search_hint),
                Modifier.padding(top = 4.dp),
                color = ink.muted,
                fontSize = 14.5.sp,
                textAlign = TextAlign.Center,
            )
        }
        return
    }
    val nothing = chats.isEmpty() && strangers.isEmpty() && hits.isEmpty() && !asking && !scanning
    val accountsShown = part != SearchPart.Messages
    val messagesShown = part != SearchPart.Accounts
    val empty =
        when (part) {
            SearchPart.All -> nothing
            SearchPart.Accounts -> chats.isEmpty() && strangers.isEmpty() && !asking
            SearchPart.Messages -> hits.isEmpty() && !scanning
        }
    if (empty) {
        Column(modifier.fillMaxWidth().padding(horizontal = 28.dp, vertical = 60.dp), horizontalAlignment = Alignment.CenterHorizontally) {
            Text(stringResource(R.string.search_nothing), color = ink.text, fontSize = 16.sp, fontWeight = FontWeight.SemiBold)
            Text(
                stringResource(R.string.search_no_match, text),
                Modifier.padding(top = 4.dp),
                color = ink.muted,
                fontSize = 14.5.sp,
                textAlign = TextAlign.Center,
            )
        }
        return
    }
    LazyColumn(modifier.fillMaxWidth()) {
        if (accountsShown) {
            val all = part == SearchPart.All
            val count = chats.size + strangers.size
            if (all && (count > 0 || asking)) {
                item(key = "head-accounts") {
                    SearchHead(
                        stringResource(R.string.search_accounts),
                        ink,
                        accentText,
                        if (count > ALL_SHOWS) ({ onPart(SearchPart.Accounts) }) else null,
                    )
                }
            }
            val firstChats = if (all) chats.take(ALL_SHOWS) else chats
            val firstStrangers = if (all) strangers.take((ALL_SHOWS - firstChats.size).coerceAtLeast(0)) else strangers
            if (!all && firstChats.isNotEmpty()) {
                item(key = "head-chats") { SearchHead(stringResource(R.string.search_in_chats), ink, accentText, null) }
            }
            items(firstChats, key = { "chat-${it.id}" }) { row ->
                Conversation(
                    row,
                    label(row, people, saved),
                    me,
                    unread[row.id] ?: 0,
                    false,
                    ink,
                    accentText,
                    false,
                    false,
                    null,
                    Modifier,
                    onProfile,
                    onLongClick = {},
                ) { onRow(row) }
            }
            if (!all && (strangers.isNotEmpty() || asking)) {
                item(key = "head-server") { SearchHead(stringResource(R.string.search_on_server), ink, accentText, null) }
            }
            items(firstStrangers, key = { "person-${it.id}" }) { account ->
                PersonRow(account, ink, if (all) newTag else null) { onPerson(account) }
            }
            if (!all && asking && strangers.isEmpty()) {
                item(key = "asking") {
                    Text(
                        stringResource(R.string.search_asking),
                        Modifier.padding(horizontal = 16.dp, vertical = 10.dp),
                        color = ink.muted,
                        fontSize = 14.sp,
                    )
                }
            }
        }
        if (messagesShown) {
            val all = part == SearchPart.All
            val shown = if (all) hits.take(ALL_SHOWS) else hits
            if (all && hits.isNotEmpty()) {
                item(key = "head-messages") {
                    SearchHead(
                        stringResource(R.string.search_messages),
                        ink,
                        accentText,
                        if (hits.size > ALL_SHOWS || search.more) ({ onPart(SearchPart.Messages) }) else null,
                    )
                }
            }
            items(shown, key = { "hit-${it.chat}-${it.seq}" }) { hit ->
                val row = rows.firstOrNull { it.id == hit.chat }
                HitRow(hit, row, row?.let { label(it, people, saved) }.orEmpty(), me, ink) { onHit(hit) }
            }
            if (!all && search.done && search.more) {
                item(key = "older") {
                    Text(
                        stringResource(R.string.search_show_older),
                        Modifier
                            .fillMaxWidth()
                            .clickable(role = Role.Button, onClick = onOlder)
                            .padding(horizontal = 16.dp, vertical = 12.dp),
                        color = accentText,
                        fontSize = 14.sp,
                        fontWeight = FontWeight.SemiBold,
                    )
                }
            }
        }
    }
}

/** A found message: its chat's photo and name, its time, and two lines around the match, tinted. */
@Composable
private fun HitRow(
    hit: Hit,
    row: ChatRow?,
    name: String,
    me: String?,
    ink: Ink,
    onClick: () -> Unit,
) {
    val person =
        when (row?.type) {
            DIRECT -> row.let { other(it, me)?.account }
            SELF -> me
            else -> null
        }
    val mark = accent.copy(alpha = if (isDark()) 0.42f else 0.22f)
    val lead = hit.before.replace('\n', ' ')
    val cut =
        if (lead.length > HIT_LEAD) {
            // Cut at a word where one is near, so the line starts with a whole word.
            val from = lead.length - HIT_LEAD
            val space = lead.indexOf(' ', from)
            val start = if (space in from until from + 12) space + 1 else from
            "…" + lead.substring(if (Character.isLowSurrogate(lead[start])) start + 1 else start)
        } else {
            lead
        }
    val text =
        androidx.compose.ui.text.buildAnnotatedString {
            append(cut)
            pushStyle(androidx.compose.ui.text.SpanStyle(background = mark, color = ink.text))
            append(hit.matched.replace('\n', ' '))
            pop()
            append(hit.after.replace('\n', ' '))
        }
    Row(
        Modifier.fillMaxWidth().clickable(role = Role.Button, onClick = onClick).padding(horizontal = 16.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Avatar(person, name, 44.dp)
        Column(Modifier.padding(start = 12.dp).weight(1f)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(
                    name,
                    Modifier.weight(1f),
                    color = ink.text,
                    fontSize = 15.5.sp,
                    fontWeight = FontWeight.SemiBold,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                Text(chatWhen(hit.saved_at), color = ink.muted, fontSize = 12.sp)
            }
            Text(text, Modifier.padding(top = 1.dp), color = ink.muted, fontSize = 14.sp, maxLines = 2, overflow = TextOverflow.Ellipsis)
        }
    }
}
