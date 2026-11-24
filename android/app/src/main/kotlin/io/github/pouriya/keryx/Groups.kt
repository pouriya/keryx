// Groups and channels, drawn in `tmp/design/groups.html` and approved as drawn: what the app knows
// of each, the + menu, the create sheet, the group's page with its members, Requests, History and
// Options, the Topics sheet, the topic tabs, invitations and asking to join. The chat itself stays in
// ChatScreen.kt and the list's rows in Chats.kt, as android/AGENTS.md chose.

package io.github.pouriya.keryx

import android.graphics.Bitmap
import android.graphics.BitmapFactory
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.PickVisualMediaRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectVerticalDragGestures
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsFocusedAsState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.IntrinsicSize
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
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
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalLayoutDirection
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.BaselineShift
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextDirection
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.LayoutDirection
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.zIndex
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.nio.ByteBuffer
import java.time.Instant
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.time.format.FormatStyle

internal const val TYPE_GROUP = 2
internal const val TYPE_CHANNEL = 3

private val accent = Color(0xFF4576C4)
private val onAccent = Color(0xFFFFFFFF)
private val tile = Color(0xFF2A436B)

/** A group's or channel's state from the chat list, kept as core sends it. */
internal data class GroupInfo(
    val type: Int,
    val role: String?,
    val topicsOn: Boolean,
    val topicIds: List<String>,
    val options: ChatOptions?,
    val findable: FindableItem?,
    val memberCount: Long?,
    val requestCount: Long?,
)

/** Someone typing in a group, and where. */
internal data class Typist(
    val account: String,
    val topic: String?,
    val thread: Long?,
    val at: Long,
)

/** Everything the app knows of this account's groups and channels, kept up to date from core's events. */
internal class GroupState {
    val infos = mutableStateMapOf<String, GroupInfo>()
    val profiles = mutableStateMapOf<String, ShownChatProfile>()

    /** Topic names by chat and topic, from the profile and from each topics change. */
    val topicNames = mutableStateMapOf<String, Map<String, String>>()
    val members = mutableStateMapOf<String, List<MemberItem>>()
    val requests = mutableStateMapOf<String, List<RequestItem>>()
    val invitations = mutableStateMapOf<String, InvitationShown>()
    val asked = mutableStateMapOf<String, Event.Asked>()
    val found = mutableStateListOf<FoundChat>()

    /** Bumped when a group's photo is kept, so whatever draws it reads it again. */
    val kept = mutableStateMapOf<String, Int>()
    val typing = mutableStateMapOf<String, List<Typist>>()

    /** The topic open in each chat: empty for General, none while its topic list shows. */
    val topicOpen = mutableStateMapOf<String, String>()

    /** Each chat's topics' own settings and moderators by topic, General's under [GENERAL_TOPIC]. */
    val settings = mutableStateMapOf<String, Map<String, TopicSetting>>()

    /** Topics hidden from this account, by chat: left out of its list until shown again. */
    val hiddenFromMe = mutableStateMapOf<String, Set<String>>()

    /** Topics' opened profiles, by chat and topic, General's under [GENERAL_TOPIC]. */
    val topicProfiles = mutableStateMapOf<Pair<String, String>, ShownTopicProfile>()

    /** A delete's or edit's copies, opened by core, by chat and the history row's `n`. Memory only. */
    val copies = mutableStateMapOf<Pair<String, Long>, List<Shown>>()
    var unread by mutableStateOf(Unread())

    /** An invitation being accepted, so Join turns until the chat opens. */
    val joining = mutableStateMapOf<String, Boolean>()

    /** A new chat made here, by its name, until core says its id. */
    var creating by mutableStateOf<String?>(null)

    /** The chat just made here, tinted on the list until it is opened or the list is left. */
    var fresh by mutableStateOf<String?>(null)

    /** The last refusal a group screen should show, until it is read. */
    var refusal by mutableStateOf<String?>(null)

    /** When slow mode lets this account write again in each chat, from the server's refusal or this phone's own send. */
    val slowUntil = mutableStateMapOf<String, Long>()
    private val topicIdWaiting = ArrayDeque<(String) -> Unit>()

    fun info(chat: String) = infos[chat]

    fun isGroupish(chat: String) = infos[chat]?.type.let { it == TYPE_GROUP || it == TYPE_CHANNEL }

    fun name(chat: String): String? = profiles[chat]?.name ?: infos[chat]?.findable?.name ?: invitations[chat]?.name ?: asked[chat]?.name

    /** Its topics in order, each with its name, leaving out those hidden from this account. */
    fun topics(chat: String): List<TopicName> {
        val info = infos[chat] ?: return emptyList()
        val names = topicNames[chat].orEmpty()
        val hidden = hiddenFromMe[chat].orEmpty()
        return info.topicIds.filter { it !in hidden }.mapIndexed { at, id -> TopicName(id, names[id].orEmpty(), at.toLong()) }
    }

    /** A topic's settings, General's for an empty or missing topic; a new one's are the defaults. */
    fun setting(
        chat: String,
        topic: String?,
    ): TopicSetting {
        val id = topic?.ifEmpty { null } ?: GENERAL_TOPIC
        return settings[chat]?.get(id) ?: TopicSetting(id)
    }

    /** A topic's profile, General's for an empty or missing topic. */
    fun topicProfile(
        chat: String,
        topic: String?,
    ): ShownTopicProfile? = topicProfiles[chat to (topic?.ifEmpty { null } ?: GENERAL_TOPIC)]

    /** Whether this account moderates a topic: the chat's owners and moderators, and the topic's own. */
    fun moderating(
        chat: String,
        topic: String?,
        me: String?,
    ): Boolean = managing(chat) || (me != null && me in setting(chat, topic).mod_list)

    private fun keepSetting(
        chat: String,
        item: TopicSetting,
    ) {
        settings[chat] = settings[chat].orEmpty() + (item.topic to item)
    }

    fun role(chat: String) = infos[chat]?.role

    fun managing(chat: String) = role(chat).let { it == "owner" || it == "moderator" }

    fun owning(chat: String) = role(chat) == "owner"

    /** Whether this account may add people or let requests in: owners and moderators, or members when the options let them. */
    fun mayAdd(chat: String): Boolean {
        val info = infos[chat] ?: return false
        if (managing(chat)) return true
        return info.options?.allow?.contains(prefixOf(info.type) + "member.add") == true
    }

    /** Whether this account writes in the main list: always in a group, owners and moderators in a channel. */
    fun mayPost(chat: String): Boolean {
        val info = infos[chat] ?: return true
        return info.type != TYPE_CHANNEL || managing(chat)
    }

    fun askTopicId(
        chat: String,
        engine: Engine,
        then: (String) -> Unit,
    ) {
        topicIdWaiting.addLast(then)
        engine.send(Command.TopicId(chat))
    }

    fun apply(
        event: Event,
        me: String?,
    ) {
        when (event) {
            is Event.Chats -> {
                val listed = event.chat_list.map { it.chat }.toSet()
                infos.keys.filter { it !in listed }.forEach { infos.remove(it) }
                for (item in event.chat_list) {
                    if (item.type != TYPE_GROUP && item.type != TYPE_CHANNEL) continue
                    infos[item.chat] =
                        GroupInfo(
                            item.type,
                            item.role,
                            item.topics_on,
                            item.topic_list,
                            item.options,
                            item.findable,
                            // A group's list carries no count: the members already asked for keep it.
                            item.member_count ?: members[item.chat]?.size?.toLong(),
                            item.request_count,
                        )
                    settings[item.chat] = item.topic_setting_list.associateBy { it.topic }
                    // A live topic the list leaves out is hidden from this account.
                    hiddenFromMe[item.chat] = item.topic_list.filter { id -> item.topic_setting_list.none { it.topic == id } }.toSet()
                    if (asked[item.chat]?.state == "waiting" || asked[item.chat]?.state == "let_in") asked.remove(item.chat)
                    invitations.remove(item.chat)?.let { joining.remove(item.chat) }
                }
            }
            is Event.ChatProfile -> {
                profiles[event.chat] = event.profile
                if (event.profile.topic_list.isNotEmpty()) {
                    topicNames[event.chat] = topicNames[event.chat].orEmpty() + event.profile.topic_list.associate { it.id to it.name }
                }
            }
            is Event.ChatPhoto -> kept[event.photo] = (kept[event.photo] ?: 0) + 1
            is Event.Topics -> {
                infos[event.chat]?.let { infos[event.chat] = it.copy(topicsOn = event.on, topicIds = event.topic_list.map { t -> t.id }) }
                topicNames[event.chat] = topicNames[event.chat].orEmpty() + event.topic_list.associate { it.id to it.name }
                val open = topicOpen[event.chat]
                if (open != null && open.isNotEmpty() && event.topic_list.none { it.id == open }) topicOpen.remove(event.chat)
            }
            is Event.TopicId -> topicIdWaiting.removeFirstOrNull()?.invoke(event.topic)
            is Event.TopicOptions -> {
                val o = event.options
                val item = TopicSetting(event.topic, o.allow, o.deny, o.slow_mode, o.write, o.closed, o.hidden, event.mod_list)
                val viewer = !o.hidden || managing(event.chat) || (me != null && me in event.mod_list)
                val hidden = hiddenFromMe[event.chat].orEmpty()
                if (viewer) {
                    keepSetting(event.chat, item)
                    if (event.topic in hidden) hiddenFromMe[event.chat] = hidden - event.topic
                } else {
                    settings[event.chat] = settings[event.chat].orEmpty() - event.topic
                    topicProfiles.remove(event.chat to event.topic)
                    hiddenFromMe[event.chat] = hidden + event.topic
                    if (topicOpen[event.chat] == event.topic) topicOpen.remove(event.chat)
                }
            }
            is Event.TopicProfile -> topicProfiles[event.chat to event.topic] = event.profile
            is Event.TopicPhoto -> kept[event.photo] = (kept[event.photo] ?: 0) + 1
            is Event.ChatOptionsShown -> infos[event.chat]?.let { infos[event.chat] = it.copy(options = event.options) }
            is Event.Findable -> infos[event.chat]?.let { infos[event.chat] = it.copy(findable = event.findable) }
            is Event.Members -> {
                members[event.chat] = event.member_list
                val mine = event.member_list.firstOrNull { it.account == me }
                infos[event.chat]?.let { info ->
                    infos[event.chat] =
                        info.copy(
                            role = mine?.role ?: info.role,
                            memberCount = event.member_list.size.toLong(),
                        )
                }
            }
            is Event.Requests -> {
                val waiting = event.request_list.filter { it.state == "waiting" }
                requests[event.chat] = waiting
                infos[event.chat]?.let { infos[event.chat] = it.copy(requestCount = waiting.size.toLong()) }
            }
            is Event.JoinRequest -> {
                val request = event.request
                val list = requests[request.chat].orEmpty().filter { it.account != request.account }
                val next = if (request.state == "waiting") list + request else list
                requests[request.chat] = next
                infos[request.chat]?.let { info ->
                    val before = info.requestCount ?: 0
                    val count =
                        if (request.state == "waiting") {
                            if (requests[request.chat].orEmpty().size > list.size && list.size.toLong() >= before) {
                                before + 1
                            } else {
                                before.coerceAtLeast(
                                    next.size.toLong(),
                                )
                            }
                        } else {
                            (before - 1).coerceAtLeast(next.size.toLong())
                        }
                    infos[request.chat] = info.copy(requestCount = count)
                }
            }
            is Event.Asked -> if (event.state == "gone") asked.remove(event.chat) else asked[event.chat] = event
            is Event.Invitation -> {
                val shown = event.invitation
                if (shown == null) {
                    invitations.remove(event.chat)
                    joining.remove(event.chat)
                } else {
                    invitations[event.chat] = shown
                }
            }
            is Event.ChatEventShown -> {
                val shown = event.event
                if (event.copy_list.isNotEmpty()) copies[shown.chat to shown.n] = event.copy_list
                // Owners and moderators get the history audience for the same membership changes.
                // Either audience invalidates the cached roster so a visible page asks again.
                if (shown.audience in 0..1 && shown.action in 1..8 && members.containsKey(shown.chat)) members.remove(shown.chat)
                // This account's own role, live, so its controls show without opening the members.
                val role = mapOf(6 to "owner", 7 to "moderator", 8 to "member")[shown.action]
                if (shown.audience == 0 && role != null && shown.target == me) {
                    infos[shown.chat]?.let { infos[shown.chat] = it.copy(role = role) }
                }
            }
            // A membership line also refreshes the roster. A role line naming this account changes its role.
            is Event.Line -> {
                if (members.containsKey(event.chat)) members.remove(event.chat)
                val role = mapOf("made_owner" to "owner", "made_moderator" to "moderator", "made_member" to "member")[event.action]
                if (role != null && me != null && me in event.account_list) {
                    infos[event.chat]?.let { infos[event.chat] = it.copy(role = role) }
                }
            }
            is Event.ChatLeft -> {
                infos.remove(event.chat)
                members.remove(event.chat)
                requests.remove(event.chat)
            }
            is Event.Typing -> {
                val now = System.currentTimeMillis()
                val list = typing[event.chat].orEmpty().filter { it.account != event.account && now - it.at < TYPING_FOR_MS }
                typing[event.chat] = if (event.on) list + Typist(event.account, event.topic, event.thread, now) else list
            }
            is Event.Message ->
                typing[event.message.chat]?.let { list ->
                    typing[event.message.chat] = list.filter { it.account != event.message.account }
                }
            is Event.SearchResults -> {
                found.clear()
                found.addAll(event.chat_list)
            }
            is Event.Error -> if (event.code in GROUP_CODES) refusal = event.code
            is Event.Failed -> {
                val seconds = event.seconds
                if (event.code == "slow_mode" && seconds != null) slowUntil[event.chat] = System.currentTimeMillis() + seconds * 1000
            }
            is Event.LoggedOut -> clear()
            else -> Unit
        }
    }

    fun clear() {
        infos.clear()
        profiles.clear()
        topicNames.clear()
        members.clear()
        requests.clear()
        invitations.clear()
        asked.clear()
        found.clear()
        typing.clear()
        topicOpen.clear()
        settings.clear()
        hiddenFromMe.clear()
        topicProfiles.clear()
        copies.clear()
        joining.clear()
        slowUntil.clear()
        unread = Unread()
        creating = null
        fresh = null
        refusal = null
        topicIdWaiting.clear()
    }
}

/** How long a typing mark stays without a fresh one. */
private const val TYPING_FOR_MS = 6_000L

/** Refusals a group screen says something about. */
private val GROUP_CODES =
    setOf(
        "no_permission", "invite_needed", "bad_commit", "too_many_members", "no_invitation", "photo_failed",
        "not_member", "slow_mode", "bad_profile", "last_owner",
    )

internal fun prefixOf(type: Int) = if (type == TYPE_CHANNEL) "channel." else "group."

/** The permission names "Files and media" covers: every kind that carries a file. */
internal fun fileKinds(type: Int) =
    listOf("voice", "image", "music", "video", "doc", "file", "media_bundle").map { prefixOf(type) + "message.send." + it }

/** The options as switches: reactions, stickers, files, members may add, and slow mode in seconds. */
internal data class Switches(
    val reactions: Boolean = true,
    val stickers: Boolean = true,
    val files: Boolean = true,
    val mayAdd: Boolean = false,
    val slow: Long? = null,
) {
    fun options(type: Int): ChatOptions {
        val prefix = prefixOf(type)
        val deny = ArrayList<String>()
        if (!reactions) deny.add(prefix + "message.send.reaction")
        if (!stickers) deny.add(prefix + "message.send.sticker")
        if (!files) deny.addAll(fileKinds(type))
        val allow = if (mayAdd) listOf(prefix + "member.add") else emptyList()
        return ChatOptions(allow, deny, slow)
    }

    companion object {
        fun of(
            type: Int,
            options: ChatOptions?,
        ): Switches {
            val prefix = prefixOf(type)
            val deny = options?.deny.orEmpty()
            return Switches(
                reactions = prefix + "message.send.reaction" !in deny,
                stickers = prefix + "message.send.sticker" !in deny,
                files = fileKinds(type).none { it in deny },
                mayAdd = options?.allow?.contains(prefix + "member.add") == true,
                slow = options?.slow_mode,
            )
        }
    }
}

/** Slow mode's choices, chosen by the author: off, 10 s, 30 s, 1 m, 5 m, 15 m and 1 h. */
internal val SLOW_CHOICES = listOf<Long?>(null, 10, 30, 60, 300, 900, 3600)

@Composable
internal fun slowLabel(seconds: Long?): String =
    when {
        seconds == null || seconds == 0L -> stringResource(R.string.group_off)
        seconds < 60 -> stringResource(R.string.group_seconds, seconds.toInt())
        seconds < 3600 -> stringResource(R.string.group_minutes, (seconds / 60).toInt())
        else -> stringResource(R.string.group_hours, (seconds / 3600).toInt())
    }

/** An unread count as the design draws it: up to 99, then +99. */
internal fun capped(count: Int) = if (count > 99) "+99" else count.toString()

/** The count pill: "4", "+99", or with a mention "@4" and "@+99", chosen by the author. */
@Composable
internal fun CountPill(
    count: Int,
    mentioned: Boolean,
    modifier: Modifier = Modifier,
    small: Boolean = false,
) {
    if (count <= 0) return
    // Roboto's @ hangs below the baseline the digits sit on, so it is raised to sit level with them, with a little
    // room before the count so the two do not touch.
    val text =
        buildAnnotatedString {
            if (mentioned) withStyle(SpanStyle(baselineShift = BaselineShift(0.12f), letterSpacing = 2.sp)) { append("@") }
            append(capped(count))
        }
    Box(
        modifier
            .height(if (small) 18.dp else 22.dp)
            .widthIn(min = if (small) 18.dp else 22.dp)
            .clip(RoundedCornerShape(if (small) 9.dp else 11.dp))
            .background(accent)
            .padding(horizontal = if (small) 5.dp else 7.dp),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            text,
            color = onAccent,
            fontSize = if (small) 11.sp else 13.sp,
            fontWeight = FontWeight.Bold,
            textAlign = TextAlign.Center,
            maxLines = 1,
            style = TextStyle(textDirection = TextDirection.Ltr),
        )
    }
}

/** The person with a clock: join requests wait. In the accent colour, with no number, chosen by the author. */
internal val requestsGlyph =
    listOf(
        "M8.5 4.2a3.3 3.3 0 1 0 0.01 0z",
        "M2.5 19c0-3.4 2.7-5.6 6-5.6c1.2 0 2.3.3 3.2.8",
        "M17 12.2a4.4 4.4 0 1 0 0.01 0z",
        "M17 14.4v2.4l1.6 1",
    )

@Composable
internal fun RequestsMark(
    onClick: () -> Unit,
    size: Dp = 22.dp,
) {
    val label = stringResource(R.string.group_requests)
    Box(
        Modifier
            .size(size + 14.dp)
            .clip(CircleShape)
            .semantics { contentDescription = label }
            .clickable(role = Role.Button, onClick = onClick),
        contentAlignment = Alignment.Center,
    ) { Glyph(requestsGlyph, accentText(), size) }
}

@Composable
internal fun accentText() = kit().accent

/** A name's colour on the first message of a run, the same for a person everywhere. */
internal fun nameColor(
    account: String?,
    dark: Boolean,
): Color {
    val light = listOf(0xFFC0392B, 0xFF2E7D32, 0xFF6A1B9A, 0xFFEF6C00, 0xFF00838F, 0xFF4576C4, 0xFFAD1457)
    val night = listOf(0xFFFF8A80, 0xFF81C784, 0xFFCE93D8, 0xFFFFB74D, 0xFF4DD0E1, 0xFF7FA6E6, 0xFFF48FB1)
    val list = if (dark) night else light
    val at = ((account?.hashCode() ?: 0) and 0x7fffffff) % list.size
    return Color(list[at])
}

/** A person's name as the app shows it: display name, else username, else nothing. */
internal fun personName(
    account: String?,
    people: Map<String, AccountBrief>,
): String {
    val brief = account?.let { people[it] } ?: return ""
    return brief.display_name?.takeIf { it.isNotBlank() } ?: brief.username
}

/** Several names: "Sara", "Sara and Babak", "Sara, Babak and Mina". */
@Composable
internal fun namesText(names: List<String>): String =
    when (names.size) {
        0 -> ""
        1 -> names[0]
        else ->
            stringResource(
                R.string.group_names_and,
                names.dropLast(1).joinToString(stringResource(R.string.group_names_comma)),
                names.last(),
            )
    }

/** A membership line's words, such as "Sara added Babak and Mina", with "You" for this account. */
@Composable
internal fun lineText(
    action: String,
    actor: String,
    accountList: List<String>,
    me: String?,
    people: Map<String, AccountBrief>,
): String {
    val you = stringResource(R.string.group_you_object)
    val targets = namesText(accountList.map { if (it == me) you else personName(it, people) })
    val mine = actor == me
    val who = personName(actor, people)
    return when (action) {
        "created" -> if (mine) stringResource(R.string.line_created_you) else stringResource(R.string.line_created, who)
        "added" -> if (mine) stringResource(R.string.line_added_you, targets) else stringResource(R.string.line_added, who, targets)
        "let_in" -> if (mine) stringResource(R.string.line_let_in_you, targets) else stringResource(R.string.line_let_in, who, targets)
        "joined" -> if (mine) stringResource(R.string.line_joined_you) else stringResource(R.string.line_joined, who)
        "removed" -> if (mine) stringResource(R.string.line_removed_you, targets) else stringResource(R.string.line_removed, who, targets)
        "left" -> if (mine) stringResource(R.string.line_left_you) else stringResource(R.string.line_left, who)
        "made_owner" -> if (mine) stringResource(R.string.line_owner_you, targets) else stringResource(R.string.line_owner, who, targets)
        "made_moderator" ->
            if (mine) {
                stringResource(
                    R.string.line_moderator_you,
                    targets,
                )
            } else {
                stringResource(R.string.line_moderator, who, targets)
            }
        "made_member" -> if (mine) stringResource(R.string.line_member_you, targets) else stringResource(R.string.line_member, who, targets)
        else -> ""
    }
}

/** The same words without Compose, for the chat list's preview. */
internal fun lineWords(
    context: android.content.Context,
    action: String,
    actor: String,
    accountList: List<String>,
    me: String?,
    people: Map<String, AccountBrief>,
): String {
    val you = context.getString(R.string.group_you_object)
    val names = accountList.map { if (it == me) you else personName(it, people) }
    val targets =
        when (names.size) {
            0 -> ""
            1 -> names[0]
            else ->
                context.getString(
                    R.string.group_names_and,
                    names.dropLast(1).joinToString(context.getString(R.string.group_names_comma)),
                    names.last(),
                )
        }
    val mine = actor == me
    val who = personName(actor, people)

    fun pick(
        yours: Int,
        theirs: Int,
        withTargets: Boolean,
    ) = if (mine) {
        if (withTargets) context.getString(yours, targets) else context.getString(yours)
    } else if (withTargets) {
        context.getString(theirs, who, targets)
    } else {
        context.getString(theirs, who)
    }
    return when (action) {
        "created" -> pick(R.string.line_created_you, R.string.line_created, false)
        "added" -> pick(R.string.line_added_you, R.string.line_added, true)
        "let_in" -> pick(R.string.line_let_in_you, R.string.line_let_in, true)
        "joined" -> pick(R.string.line_joined_you, R.string.line_joined, false)
        "removed" -> pick(R.string.line_removed_you, R.string.line_removed, true)
        "left" -> pick(R.string.line_left_you, R.string.line_left, false)
        "made_owner" -> pick(R.string.line_owner_you, R.string.line_owner, true)
        "made_moderator" -> pick(R.string.line_moderator_you, R.string.line_moderator, true)
        "made_member" -> pick(R.string.line_member_you, R.string.line_member, true)
        else -> ""
    }
}

/** One history row's words, newest first on the History page. */
@Composable
private fun historyText(
    event: ChatEvent,
    type: Int,
    me: String?,
    people: Map<String, AccountBrief>,
    topics: Map<String, String>,
    inTopic: Boolean,
): String {
    val list = listOfNotNull(event.target)
    return when (event.action) {
        0 ->
            if (type == TYPE_CHANNEL) {
                stringResource(
                    R.string.history_created_channel,
                    personName(event.actor, people),
                )
            } else {
                lineText("created", event.actor, list, me, people)
            }
        1 -> lineText("added", event.actor, list, me, people)
        2 -> lineText("removed", event.actor, list, me, people)
        3 -> lineText("left", event.actor, list, me, people)
        4 -> lineText("let_in", event.actor, list, me, people)
        5 -> lineText("joined", event.actor, list, me, people)
        6 -> lineText("made_owner", event.actor, list, me, people)
        7 -> lineText("made_moderator", event.actor, list, me, people)
        8 -> lineText("made_member", event.actor, list, me, people)
        9 ->
            if (event.target == event.actor) {
                stringResource(R.string.history_deleted_own, personName(event.actor, people))
            } else {
                stringResource(R.string.history_deleted, personName(event.actor, people), personName(event.target, people))
            }
        10 ->
            if (inTopic) {
                stringResource(R.string.history_topic_created, personName(event.actor, people))
            } else {
                stringResource(R.string.history_topic_added, personName(event.actor, people), event.target?.let { topics[it] }.orEmpty())
            }
        11 -> stringResource(R.string.history_topic_removed, personName(event.actor, people))
        12 -> stringResource(R.string.history_topics_on, personName(event.actor, people))
        13 -> stringResource(R.string.history_topics_off, personName(event.actor, people))
        14 -> stringResource(R.string.history_declined, personName(event.actor, people))
        16 -> stringResource(R.string.history_topic_edited, personName(event.actor, people))
        17 -> stringResource(R.string.history_topic_options, personName(event.actor, people))
        18 -> stringResource(R.string.history_topic_mod, personName(event.actor, people), personName(event.target, people))
        19 -> stringResource(R.string.history_topic_member, personName(event.actor, people), personName(event.target, people))
        20 ->
            if (event.target == event.actor) {
                stringResource(R.string.history_edited_own, personName(event.actor, people))
            } else {
                stringResource(R.string.history_edited, personName(event.actor, people), personName(event.target, people))
            }
        else -> ""
    }
}

/**
 * What a delete removed or an edit changed, under its history row: a delete's message as it last read,
 * an edit's before and after. Copies are core's to open, so until it has, the box waits; one it cannot
 * open says so.
 */
@Composable
private fun HistoryCopies(
    event: ChatEvent,
    opened: List<Shown>?,
    ink: Ink,
) {
    val kit = kit()
    val box =
        Modifier.padding(
            top = 6.dp,
        ).fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(kit.other).padding(horizontal = 12.dp, vertical = 8.dp)
    if (opened == null) return
    if (opened.isEmpty()) {
        Text(stringResource(R.string.history_cant_open), box, color = ink.muted, fontSize = 13.5.sp)
        return
    }
    val parts =
        if (event.action == 20 && opened.size >= 2) {
            listOf(
                stringResource(R.string.history_before) to opened[opened.size - 2],
                stringResource(R.string.history_after) to opened.last(),
            )
        } else {
            listOf(null to opened.last())
        }
    Column(box, verticalArrangement = Arrangement.spacedBy(6.dp)) {
        for ((label, copy) in parts) {
            Column {
                if (label != null) Text(label, color = ink.muted, fontSize = 12.sp, fontWeight = FontWeight.SemiBold)
                Text(
                    copyText(copy),
                    color = ink.text,
                    fontSize = 14.sp,
                    lineHeight = 19.sp,
                    style = TextStyle(textDirection = TextDirection.Content),
                )
            }
        }
    }
}

/** A copy's words: its text, a reaction's emoji, a sticker, and each file's name. */
@Composable
private fun copyText(copy: Shown): String {
    val parts = ArrayList<String>()
    copy.text?.takeIf { it.isNotBlank() }?.let(parts::add)
    copy.emoji?.let(parts::add)
    if (copy.sticker != null) parts.add(stringResource(R.string.kind_sticker))
    for (file in copy.files) parts.add("📎 ${file.name}")
    return parts.joinToString("\n")
}

/**
 * A group's or channel's round photo: the opened photo once kept, its BlurHash while it is fetched,
 * or the first letter of its name.
 */
@Composable
internal fun GroupAvatar(
    chat: String,
    groups: GroupState,
    engine: Engine,
    name: String,
    size: Dp,
    modifier: Modifier = Modifier,
    blurhash: String? = null,
) {
    val photo = groups.profiles[chat]?.photo
    Box(modifier.size(size).clip(CircleShape).background(tile), contentAlignment = Alignment.Center) {
        val image = photo?.let { groupPhoto(chat, it, groups, engine) }
        val blur = (photo?.blurhash ?: blurhash)?.let { blurImage(it) }
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

@Composable
private fun groupPhoto(
    chat: String,
    photo: ShownChatPhoto,
    groups: GroupState,
    engine: Engine,
): ImageBitmap? {
    val version = groups.kept[photo.id] ?: 0
    var image by remember(photo.id) { mutableStateOf<ImageBitmap?>(null) }
    LaunchedEffect(photo.id, version) {
        val loaded =
            withContext(Dispatchers.IO) {
                val file = engine.photoFile(photo.id)
                if (file.exists()) BitmapFactory.decodeFile(file.path)?.asImageBitmap() else null
            }
        image = loaded
        if (loaded == null) engine.chatPhoto(chat, photo.id)
    }
    return image
}

@Composable
internal fun blurImage(hash: String): ImageBitmap? =
    remember(hash) {
        Core.blurhash(hash, 32, 32)?.let { rgba ->
            Bitmap
                .createBitmap(32, 32, Bitmap.Config.ARGB_8888)
                .apply { copyPixelsFromBuffer(ByteBuffer.wrap(rgba)) }
                .asImageBitmap()
        }
    }

/** The + menu: New group and New channel, centred in the screen's height on the + side, chosen by the author. */
@Composable
internal fun NewMenu(
    onClose: () -> Unit,
    onPick: (Int) -> Unit,
) {
    val dark = isDark()
    val ink = ink()
    BackHandler(onBack = onClose)
    val scrim = remember { MutableInteractionSource() }
    Box(
        Modifier
            .fillMaxSize()
            .background(Color(0x6B0A0F16))
            .clickable(scrim, null, onClick = onClose),
        // Centred both ways, as the design's menu is, chosen by the author.
        contentAlignment = Alignment.Center,
    ) {
        Column(
            Modifier
                .width(IntrinsicSize.Max)
                .widthIn(min = 200.dp)
                .shadow(14.dp, RoundedCornerShape(16.dp))
                .clip(RoundedCornerShape(16.dp))
                .background(if (dark) Color(0xFF1A2330) else Color.White)
                .clickable(remember { MutableInteractionSource() }, null) {}
                .padding(vertical = 6.dp),
        ) {
            val tone = if (dark) Color(0xFFC5D4E8) else accent
            MenuLine(peopleGlyph, stringResource(R.string.group_new), ink, tone) { onPick(TYPE_GROUP) }
            MenuLine(hornGlyph, stringResource(R.string.channel_new), ink, tone) { onPick(TYPE_CHANNEL) }
        }
    }
}

internal val peopleGlyph =
    listOf(
        "M9 4.8a3.2 3.2 0 1 0 0.01 0z",
        "M3.5 19c0-3 2.5-5 5.5-5s5.5 2 5.5 5",
        "M17 6.4a2.6 2.6 0 1 0 0.01 0z",
        "M15.5 14.2c2.6.2 5 1.9 5 4.8",
    )

/** A group's two people or a channel's horn before a name, as the chat list's row has. */
@Composable
internal fun ChatTypeMark(
    type: Int?,
    ink: Ink,
    size: Dp,
) {
    val glyph =
        when (type) {
            TYPE_GROUP -> peopleGlyph
            TYPE_CHANNEL -> hornGlyph
            else -> return
        }
    Box(Modifier.padding(end = 4.dp)) { Glyph(glyph, ink.muted, size) }
}

internal val hornGlyph = listOf("M4 10v4h3l8 4.5V5.5L7 10z", "M18.5 9a4 4 0 0 1 0 6")
internal val threadGlyph = listOf("M5 5h14v10H10l-5 4z")

/** The design's icons for the group screens, from `tmp/design/groups.html`, on a 24-unit grid. */
internal object GroupGlyphs {
    private fun circle(
        x: Float,
        y: Float,
        r: Float,
    ) = "M${x - r} ${y}a$r $r 0 1 0 ${2 * r} 0a$r $r 0 1 0 ${-2 * r} 0"

    val search = listOf(circle(11f, 11f, 6.5f), "M16 16l4 4")
    val pencil = listOf("M4 20h4L19 9l-4-4L4 16z", "M13.5 6.5l4 4")
    val hash = listOf("M9.5 4L7.5 20M16.5 4l-2 16M4.5 9h16M3.5 15h16")
    val mail = listOf("M4 6h16v12H4z", "M4 7l8 6 8-6")
    val shield = listOf("M12 3l7 3v5c0 5-3.5 8-7 10-3.5-2-7-5-7-10V6z")
    val crown = listOf("M4 18h16M5 15l-1-8 5 4 3-6 3 6 5-4-1 8z")
    val trash = listOf("M5 7h14M9 7V4h6v3M7 7l1 13h8l1-13")
    val userPlus = listOf(circle(10f, 8f, 3.5f), "M3.5 20c0-3.5 3-6 6.5-6", "M17 13v7M13.5 16.5h7")
    val userCross = listOf(circle(10f, 8f, 3.5f), "M3.5 20c0-3.5 3-6 6.5-6", "M14.5 14.5l5 5M19.5 14.5l-5 5")
    val user = listOf(circle(12f, 8f, 3.8f), "M4.5 20c0-4 3.4-6.5 7.5-6.5s7.5 2.5 7.5 6.5")
    val out = listOf("M14 4h5v16h-5", "M10 8l-4 4 4 4M6 12h10")
    val sliders = listOf("M4 7h10M18 7h2M4 17h4M12 17h8", circle(16f, 7f, 2f), circle(10f, 17f, 2f))
    val smile = listOf(circle(12f, 12f, 9f), "M8.5 14.5a4.5 4.5 0 0 0 7 0", "M9 9.5v.5M15 9.5v.5")
    val file = listOf("M6 3h8l4 4v14H6z", "M14 3v4h4")
    val sticker = listOf("M5 4h14v9l-6 7H5z", "M13 20v-5a2 2 0 0 1 2-2h4")
    val hour = listOf("M7 3h10M7 21h10M8 3c0 5 8 5 8 9s-8 4-8 9M16 3c0 5-8 5-8 9")
    val clock = listOf(circle(12f, 12f, 9f), "M12 7v5l3 2")
    val bubble = listOf("M5 5h14v10H10l-5 4z")
    val cross = listOf("M7 7l10 10M17 7L7 17")
    val up = listOf("M6 15l6-6 6 6")
    val down = listOf("M6 9l6 6 6-6")
}

@Composable
private fun MenuLine(
    icon: List<String>,
    label: String,
    ink: Ink,
    tone: Color,
    onClick: () -> Unit,
) {
    Row(
        Modifier.fillMaxWidth().clickable(role = Role.Button, onClick = onClick).padding(horizontal = 18.dp, vertical = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        Glyph(icon, tone)
        Text(label, color = ink.text, fontSize = 15.5.sp, fontWeight = FontWeight.SemiBold)
    }
}

/** The small blue dot at the edge where an unread tab is out of sight, chosen by the author. */
@Composable
internal fun EdgeDot(modifier: Modifier) {
    Box(modifier.padding(horizontal = 2.dp).size(7.dp).clip(CircleShape).background(accent))
}

/**
 * The colours `tmp/design/groups.html` draws the group and channel screens in, approved as drawn by the
 * author: the accent for icons and words that act, the ground under a page of cards, a card, a tint for
 * a chosen tab, a bubble's colour, a switch that is off, and red.
 */
internal data class Kit(
    val accent: Color,
    val ground: Color,
    val card: Color,
    val tint: Color,
    val other: Color,
    val off: Color,
    val red: Color,
)

@Composable
internal fun kit(): Kit =
    if (isDark()) {
        Kit(
            // The app's night blue for words and icons, as the chat list's, chosen by the author over the drawing's.
            Color(0xFFC5D4E8),
            Color(0xFF0F151D),
            Color(0xFF1A2330),
            Color(0xFF1E2833),
            Color(0xFF1E2833),
            Color(0xFF3A4757),
            Color(0xFFE26767),
        )
    } else {
        Kit(accent, Color(0xFFF2F4F8), Color.White, Color(0xFFEEF2F8), Color(0xFFF1F3F6), Color(0xFFC9D1DC), Color(0xFFC0392B))
    }

/** The design's dimmed screen under a menu, a sheet or a dialog. */
internal val scrimTone = Color(0x6B0A0F16)

/** What a line inside a card paints under itself, so the card's 1 dp gaps show as dividers. */
private val LocalFill = staticCompositionLocalOf { Color.Transparent }

/** A plain page's bar: back, a title with an optional muted count or a line under it, and an action at the end. */
@Composable
internal fun PageBar(
    title: String,
    ink: Ink,
    onBack: () -> Unit,
    sub: String? = null,
    count: String? = null,
    action: (@Composable () -> Unit)? = null,
) {
    val rtl = LocalLayoutDirection.current == LayoutDirection.Rtl
    Row(Modifier.fillMaxWidth().height(64.dp).padding(start = 4.dp, end = 8.dp), verticalAlignment = Alignment.CenterVertically) {
        IconButton(stringResource(R.string.back), onBack, 44.dp) { BackMark(ink.text, rtl) }
        Column(Modifier.weight(1f).padding(horizontal = 6.dp)) {
            Text(
                buildAnnotatedString {
                    append(title)
                    if (count != null) {
                        withStyle(SpanStyle(color = ink.muted, fontWeight = FontWeight.Medium)) { append(" $count") }
                    }
                },
                color = ink.text,
                fontSize = if (sub != null) 17.sp else 19.sp,
                fontWeight = FontWeight.SemiBold,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            if (sub != null) Text(sub, color = ink.muted, fontSize = 13.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
        action?.invoke()
    }
}

/** A word at the bar's end, such as Next, Create or Add: the accent colour, or muted while it cannot act. */
@Composable
internal fun BarWord(
    label: String,
    enabled: Boolean,
    onClick: () -> Unit,
) {
    val ink = ink()
    Text(
        label,
        Modifier
            .clip(RoundedCornerShape(10.dp))
            .clickable(enabled = enabled, role = Role.Button, onClick = onClick)
            .padding(horizontal = 12.dp, vertical = 8.dp),
        color = if (enabled) kit().accent else ink.muted,
        fontSize = 15.sp,
        fontWeight = FontWeight.SemiBold,
    )
}

/** A page drawn over what opened it, taking every touch and the back button. */
@Composable
internal fun Over(
    onBack: () -> Unit,
    page: Color = ink().page,
    content: @Composable ColumnScope.() -> Unit,
) {
    BackHandler(onBack = onBack)
    Bars(page, isDark())
    Column(
        Modifier
            .fillMaxSize()
            .background(page)
            .pointerInput(Unit) { awaitPointerEventScope { while (true) awaitPointerEvent() } }
            .windowInsetsPadding(WindowInsets.statusBars)
            .windowInsetsPadding(WindowInsets.navigationBars)
            .imePadding(),
        content = content,
    )
}

/**
 * A text box with a hint, as the design draws it: a 1.5 dp border, the accent's while focused, an
 * optional icon before the text, and the count under its end when [count] is set.
 */
@Composable
internal fun Field(
    value: String,
    onValue: (String) -> Unit,
    hint: String,
    ink: Ink,
    modifier: Modifier = Modifier,
    single: Boolean = true,
    max: Int? = null,
    count: Boolean = false,
    lead: List<String>? = null,
    height: Dp = 52.dp,
    enabled: Boolean = true,
    keyboardOptions: KeyboardOptions = KeyboardOptions.Default,
) {
    val interaction = remember { MutableInteractionSource() }
    val focused by interaction.collectIsFocusedAsState()
    val shape = RoundedCornerShape(12.dp)
    Column(modifier.fillMaxWidth()) {
        Row(
            Modifier
                .fillMaxWidth()
                .heightIn(min = height)
                .border(1.5.dp, if (focused) accent else ink.line, shape)
                .padding(horizontal = 14.dp, vertical = 10.dp),
            verticalAlignment = if (single) Alignment.CenterVertically else Alignment.Top,
            horizontalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            if (lead != null) Glyph(lead, ink.muted, 18.dp)
            Box(Modifier.weight(1f)) {
                if (value.isEmpty()) Text(hint, color = ink.muted, fontSize = 16.sp)
                BasicTextField(
                    value,
                    { next -> onValue(if (max != null) next.take(max) else next) },
                    // A box of several lines takes a tap anywhere in it, not only on its first line.
                    if (single) Modifier.fillMaxWidth() else Modifier.fillMaxWidth().heightIn(min = height - 20.dp),
                    singleLine = single,
                    enabled = enabled,
                    keyboardOptions = keyboardOptions,
                    textStyle = TextStyle(color = ink.text, fontSize = 16.sp, textDirection = TextDirection.Content),
                    cursorBrush = SolidColor(accent),
                    interactionSource = interaction,
                )
            }
        }
        if (count && max != null) {
            Text(
                "${exact(value.length.toLong())}/${exact(max.toLong())}",
                Modifier.align(Alignment.End).padding(top = 2.dp, end = 4.dp),
                color = ink.muted,
                fontSize = 11.5.sp,
            )
        }
    }
}

/** A switch line: an optional icon, a title, a line under it, and the switch. */
@Composable
internal fun SwitchLine(
    title: String,
    help: String?,
    on: Boolean,
    ink: Ink,
    enabled: Boolean = true,
    icon: List<String>? = null,
    onChange: (Boolean) -> Unit,
) {
    Row(
        Modifier
            .fillMaxWidth()
            .background(LocalFill.current)
            .clickable(enabled = enabled, role = Role.Switch) { onChange(!on) }
            .heightIn(min = 52.dp)
            .padding(horizontal = 14.dp, vertical = 11.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        if (icon != null) Glyph(icon, kit().accent, 20.dp)
        Column(Modifier.weight(1f)) {
            Text(title, color = if (enabled) ink.text else ink.muted, fontSize = 15.5.sp, fontWeight = FontWeight.SemiBold)
            if (help != null) Text(help, color = ink.muted, fontSize = 12.5.sp, lineHeight = 17.sp)
        }
        Toggle(on, enabled)
    }
}

@Composable
private fun Toggle(
    on: Boolean,
    enabled: Boolean,
) {
    Box(
        Modifier
            .size(width = 44.dp, height = 26.dp)
            .clip(RoundedCornerShape(13.dp))
            .background(if (on) accent.copy(alpha = if (enabled) 1f else 0.5f) else kit().off),
        contentAlignment = if (on) Alignment.CenterEnd else Alignment.CenterStart,
    ) {
        Box(Modifier.padding(3.dp).size(20.dp).clip(CircleShape).background(Color.White))
    }
}

/**
 * A tappable line, as the design's cards draw theirs: an icon in the accent colour, or in [tone] with
 * the title, then a value, a count and a chevron at its end.
 */
@Composable
internal fun ValueLine(
    title: String,
    value: String?,
    ink: Ink,
    tone: Color? = null,
    icon: List<String>? = null,
    help: String? = null,
    badge: Long? = null,
    chevron: Boolean = false,
    trailing: (@Composable () -> Unit)? = null,
    onClick: (() -> Unit)?,
) {
    Row(
        Modifier
            .fillMaxWidth()
            .background(LocalFill.current)
            .then(if (onClick != null) Modifier.clickable(role = Role.Button, onClick = onClick) else Modifier)
            .heightIn(min = 52.dp)
            .padding(horizontal = 14.dp, vertical = 11.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        if (icon != null) Glyph(icon, tone ?: kit().accent, 20.dp)
        Column(Modifier.weight(1f)) {
            Text(title, color = tone ?: ink.text, fontSize = 15.5.sp, fontWeight = FontWeight.SemiBold)
            if (help != null) Text(help, color = ink.muted, fontSize = 12.5.sp, lineHeight = 17.sp)
        }
        if (value != null) Text(value, color = ink.muted, fontSize = 14.sp)
        if (badge != null && badge > 0) CountPill(badge.toInt(), false)
        trailing?.invoke()
        if (chevron) Glyph(chevronGlyph(), ink.muted, 18.dp)
    }
}

/** The chevron pointing along the reading direction. */
@Composable
internal fun chevronGlyph() = if (LocalLayoutDirection.current == LayoutDirection.Rtl) listOf("M15 6l-6 6 6 6") else Glyphs.chevron

/**
 * Rounded lines with an optional caption over them, the design's cards: each line paints [fill] and the
 * 1 dp between them shows the divider's colour.
 */
@Composable
internal fun GroupCard(
    title: String?,
    ink: Ink,
    fill: Color = kit().card,
    modifier: Modifier = Modifier.padding(horizontal = 12.dp, vertical = 6.dp),
    content: @Composable ColumnScope.() -> Unit,
) {
    Column(modifier.fillMaxWidth()) {
        if (title != null) {
            Text(
                title,
                Modifier.padding(start = 4.dp, end = 4.dp, top = 4.dp, bottom = 10.dp),
                color = ink.muted,
                fontSize = 12.5.sp,
                fontWeight = FontWeight.Bold,
                letterSpacing = 0.4.sp,
            )
        }
        CompositionLocalProvider(LocalFill provides fill) {
            Column(
                Modifier.fillMaxWidth().clip(RoundedCornerShape(16.dp)).background(ink.line),
                verticalArrangement = Arrangement.spacedBy(1.dp),
                content = content,
            )
        }
    }
}

/** What a custom line inside a [GroupCard] paints under itself. */
@Composable
internal fun cardFill() = LocalFill.current

/**
 * A button filling its width: the accent colour, or outlined. [big] is the design's 48 dp one with
 * rounder corners; the others are 44 dp, as a pair of Decline and Let in.
 */
@Composable
internal fun WideButton(
    label: String,
    ink: Ink,
    modifier: Modifier = Modifier,
    strong: Boolean = true,
    enabled: Boolean = true,
    busy: Boolean = false,
    big: Boolean = false,
    tone: Color? = null,
    onClick: () -> Unit,
) {
    val shape = RoundedCornerShape(if (big) 14.dp else 12.dp)
    Box(
        modifier
            .fillMaxWidth()
            .height(if (big) 48.dp else 44.dp)
            .clip(shape)
            .then(if (strong) Modifier.background(accent) else Modifier.border(1.5.dp, ink.line, shape))
            .graphicsLayer { alpha = if (enabled) 1f else 0.5f }
            .clickable(enabled = enabled && !busy, role = Role.Button, onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        if (busy) {
            Spinner(if (strong) onAccent else ink.text)
        } else {
            Text(
                label,
                color = tone ?: if (strong) onAccent else ink.text,
                fontSize = if (big) 15.5.sp else 15.sp,
                fontWeight = FontWeight.SemiBold,
            )
        }
    }
}

/** A sheet at the screen's foot, over a dimmed screen, with the design's handle. */
@Composable
internal fun FootSheet(
    onClose: () -> Unit,
    divided: Boolean = true,
    content: @Composable ColumnScope.() -> Unit,
) {
    BackHandler(onBack = onClose)
    val scrim = remember { MutableInteractionSource() }
    val kit = kit()
    Box(
        Modifier.fillMaxSize().background(scrimTone).clickable(scrim, null, onClick = onClose),
        contentAlignment = Alignment.BottomCenter,
    ) {
        Column(
            Modifier
                .fillMaxWidth()
                .shadow(16.dp, RoundedCornerShape(topStart = 24.dp, topEnd = 24.dp))
                .clip(RoundedCornerShape(topStart = 24.dp, topEnd = 24.dp))
                .background(kit.card)
                .clickable(remember { MutableInteractionSource() }, null) {}
                .windowInsetsPadding(WindowInsets.navigationBars)
                .imePadding()
                .padding(bottom = 12.dp),
        ) {
            Box(
                Modifier
                    .padding(top = 10.dp, bottom = 4.dp)
                    .align(Alignment.CenterHorizontally)
                    .size(36.dp, 4.dp)
                    .clip(RoundedCornerShape(2.dp))
                    .background(kit.off),
            )
            CompositionLocalProvider(LocalFill provides kit.card) {
                Column(
                    Modifier.verticalScroll(rememberScrollState()).background(if (divided) ink().line else kit.card),
                    verticalArrangement = Arrangement.spacedBy(if (divided) 1.dp else 0.dp),
                    content = content,
                )
            }
        }
    }
}

/** One of a dialog's words at its foot: the accent colour, or red for one that destroys. */
internal class DialogAct(
    val label: String,
    val red: Boolean = false,
    val enabled: Boolean = true,
    val onClick: () -> Unit,
)

/**
 * A dialog as the design draws one: a title, an optional line under it, anything [content] adds, then
 * its words at the end, wrapping when they do not fit on one row.
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
internal fun AskDialog(
    title: String,
    body: String?,
    onCancel: () -> Unit,
    acts: List<DialogAct>,
    content: (@Composable ColumnScope.() -> Unit)? = null,
) {
    BackHandler(onBack = onCancel)
    val ink = ink()
    val kit = kit()
    val scrim = remember { MutableInteractionSource() }
    Box(
        Modifier
            .fillMaxSize()
            .background(scrimTone)
            .clickable(scrim, null, onClick = onCancel)
            .imePadding()
            .padding(20.dp),
        contentAlignment = Alignment.Center,
    ) {
        Column(
            Modifier
                .fillMaxWidth()
                .shadow(20.dp, RoundedCornerShape(24.dp))
                .clip(RoundedCornerShape(24.dp))
                .background(kit.card)
                .clickable(remember { MutableInteractionSource() }, null) {}
                .padding(start = 22.dp, end = 22.dp, top = 22.dp, bottom = 14.dp),
        ) {
            Text(title, color = ink.text, fontSize = 19.sp, fontWeight = FontWeight.SemiBold, lineHeight = 25.sp)
            Spacer(Modifier.height(10.dp))
            if (body != null) Text(body, Modifier.padding(bottom = 16.dp), color = ink.muted, fontSize = 14.5.sp, lineHeight = 21.sp)
            content?.invoke(this)
            FlowRow(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(4.dp, Alignment.End)) {
                for (act in acts) {
                    Box(
                        Modifier
                            .height(40.dp)
                            .clip(RoundedCornerShape(20.dp))
                            .clickable(enabled = act.enabled, role = Role.Button, onClick = act.onClick)
                            .padding(horizontal = 12.dp),
                        contentAlignment = Alignment.Center,
                    ) {
                        Text(
                            act.label,
                            color = (if (act.red) kit.red else kit.accent).copy(alpha = if (act.enabled) 1f else 0.4f),
                            fontSize = 14.5.sp,
                            fontWeight = FontWeight.SemiBold,
                        )
                    }
                }
            }
        }
    }
}

/** A turning ring, for a button whose work is under way. */
@Composable
internal fun Spinner(color: Color) {
    var turn by remember { mutableStateOf(0f) }
    LaunchedEffect(Unit) {
        while (true) {
            delay(16)
            turn = (turn + 8f) % 360f
        }
    }
    androidx.compose.foundation.Canvas(Modifier.size(22.dp)) {
        drawArc(
            color,
            turn,
            270f,
            false,
            style =
                androidx.compose.ui.graphics.drawscope.Stroke(
                    width = 2.5.dp.toPx(),
                    cap = androidx.compose.ui.graphics.StrokeCap.Round,
                ),
        )
    }
}

internal fun shortTime(at: Long): String =
    DateTimeFormatter.ofLocalizedTime(FormatStyle.SHORT).format(Instant.ofEpochMilli(at).atZone(ZoneId.systemDefault()))

/** What step 1 of making a group or channel set, kept while step 2 is open, so Back keeps it. */
internal data class NewChat(
    val type: Int,
    val name: String = "",
    val bio: String = "",
    val links: List<ProfileLink> = emptyList(),
    val topics: Boolean = false,
    val findable: Boolean = false,
    val switches: Switches = Switches(),
    val photo: Bitmap? = null,
)

/**
 * Making a group or channel, in two steps as drawn: the name first with everything else optional,
 * then people if you want, Create there even with nobody picked, chosen by the author.
 */
@Composable
internal fun CreateSheet(
    type: Int,
    topicsDefault: Boolean,
    me: String?,
    people: Map<String, AccountBrief>,
    known: List<String>,
    found: List<AccountBrief>,
    onSearch: (String) -> Unit,
    onClose: () -> Unit,
    onCreate: (NewChat, List<String>) -> Unit,
) {
    val ink = ink()
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var draft by remember { mutableStateOf(NewChat(type, topics = topicsDefault)) }
    var step by remember { mutableIntStateOf(1) }
    var more by remember { mutableStateOf(false) }
    var cropping by remember { mutableStateOf<Bitmap?>(null) }
    var slowPicking by remember { mutableStateOf(false) }
    val picked = remember { mutableStateListOf<String>() }
    var query by remember { mutableStateOf("") }
    val picker =
        rememberLauncherForActivityResult(ActivityResultContracts.PickVisualMedia()) { uri ->
            if (uri != null) scope.launch { cropping = withContext(Dispatchers.IO) { forCrop(context, uri) } }
        }
    LaunchedEffect(query) {
        delay(300)
        onSearch(query.trim())
    }
    val source = cropping
    if (source != null) {
        BackHandler { cropping = null }
        Crop(source, { cropping = null }) { square ->
            cropping = null
            draft = draft.copy(photo = square)
        }
        return
    }
    val channel = type == TYPE_CHANNEL
    val kit = kit()
    if (step == 1) {
        Over(onClose) {
            val ready = draft.name.trim().isNotEmpty()
            PageBar(stringResource(if (channel) R.string.channel_new else R.string.group_new), ink, onClose) {
                BarWord(stringResource(R.string.group_next), ready) { step = 2 }
            }
            Column(
                Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(start = 16.dp, end = 16.dp, top = 4.dp, bottom = 24.dp),
                verticalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                Row(Modifier.fillMaxWidth().padding(top = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                    Box(
                        Modifier
                            .size(64.dp)
                            .clip(CircleShape)
                            .background(kit.tint)
                            .clickable(role = Role.Button) {
                                picker.launch(PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageOnly))
                            },
                        contentAlignment = Alignment.Center,
                    ) {
                        val photo = draft.photo
                        if (photo != null) {
                            Image(remember(photo) { photo.asImageBitmap() }, null, Modifier.fillMaxSize(), contentScale = ContentScale.Crop)
                        } else {
                            Glyph(Glyphs.cam, kit.accent, 26.dp)
                        }
                    }
                    Field(
                        draft.name,
                        { draft = draft.copy(name = it) },
                        stringResource(if (channel) R.string.channel_name else R.string.group_name),
                        ink,
                        Modifier.padding(start = 12.dp).weight(1f),
                        max = GROUP_NAME_MAX,
                        count = true,
                    )
                }
                Field(
                    draft.bio,
                    { draft = draft.copy(bio = it) },
                    stringResource(R.string.group_bio),
                    ink,
                    single = false,
                    max = GROUP_BIO_MAX,
                )
                Links(draft.links, ink, kit.ground) { draft = draft.copy(links = it) }
                GroupCard(null, ink, kit.ground, Modifier) {
                    SwitchLine(stringResource(R.string.group_topics), stringResource(R.string.group_topics_help), draft.topics, ink) {
                        draft = draft.copy(topics = it)
                    }
                    SwitchLine(stringResource(R.string.group_findable), stringResource(R.string.group_findable_help), draft.findable, ink) {
                        draft = draft.copy(findable = it)
                    }
                    val moreLine =
                        @Composable {
                            ValueLine(
                                stringResource(R.string.group_more),
                                null,
                                ink,
                                help = if (more) null else stringResource(R.string.group_more_help),
                                trailing = { Glyph(if (more) GroupGlyphs.up else GroupGlyphs.down, ink.muted, 18.dp) },
                            ) { more = !more }
                        }
                    if (more) {
                        OptionLines(type, draft.switches, ink, head = moreLine, onChange = { draft = draft.copy(switches = it) }) {
                            slowPicking = true
                        }
                    } else {
                        moreLine()
                    }
                }
            }
        }
    } else {
        Over({ step = 1 }) {
            PageBar(
                stringResource(R.string.group_add_people),
                ink,
                { step = 1 },
                sub =
                    if (picked.isEmpty()) {
                        stringResource(R.string.group_just_you)
                    } else {
                        pluralStringResource(R.plurals.group_picked, picked.size, picked.size)
                    },
            ) {
                BarWord(stringResource(R.string.group_create), true) { onCreate(draft, picked.toList()) }
            }
            PeoplePicker(me, people, known, found, picked, query, { query = it }, ink, Modifier.weight(1f))
        }
    }
    if (slowPicking) {
        SlowPicker(draft.switches.slow, ink, { slowPicking = false }) {
            slowPicking = false
            draft = draft.copy(switches = draft.switches.copy(slow = it))
        }
    }
}

/** A group's or channel's name, as an account's display name: 1 to 64 characters after trimming. */
internal const val GROUP_NAME_MAX = 64
internal const val GROUP_BIO_MAX = 140
private const val LINKS_MAX = 3
private const val LINK_NAME_MAX = 32
private const val LINK_URL_MAX = 512

/** Up to 3 links, each a name and an address starting with https:// or http://, then Add a link in its card. */
@Composable
internal fun Links(
    links: List<ProfileLink>,
    ink: Ink,
    fill: Color,
    onChange: (List<ProfileLink>) -> Unit,
) {
    links.forEachIndexed { at, link ->
        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Field(
                    link.name,
                    { name -> onChange(links.mapIndexed { i, l -> if (i == at) l.copy(name = name) else l }) },
                    stringResource(R.string.group_link_name),
                    ink,
                    max = LINK_NAME_MAX,
                )
                Field(
                    link.url,
                    { url -> onChange(links.mapIndexed { i, l -> if (i == at) l.copy(url = url) else l }) },
                    "https://",
                    ink,
                    max = LINK_URL_MAX,
                )
            }
            IconButton(stringResource(R.string.group_remove), { onChange(links.filterIndexed { i, _ -> i != at }) }) {
                Glyph(GroupGlyphs.cross, ink.muted, 18.dp)
            }
        }
    }
    if (links.size < LINKS_MAX) {
        GroupCard(null, ink, fill, Modifier) {
            ValueLine(stringResource(R.string.group_add_link), null, ink, tone = kit().accent, icon = Glyphs.link) {
                onChange(links + ProfileLink("", ""))
            }
        }
    }
}

/** Links worth sending: both parts filled and the address one a browser can open. */
internal fun usable(links: List<ProfileLink>) =
    links
        .map { ProfileLink(it.name.trim(), it.url.trim()) }
        .filter { it.name.isNotEmpty() && (it.url.startsWith("https://") || it.url.startsWith("http://")) }

/** The options' lines inside a card: reactions, stickers, files and media, may add people, and slow mode. */
@Composable
internal fun OptionLines(
    type: Int,
    switches: Switches,
    ink: Ink,
    enabled: Boolean = true,
    help: Boolean = true,
    head: (@Composable () -> Unit)? = null,
    onChange: (Switches) -> Unit,
    onSlow: () -> Unit,
) {
    val reactions =
        @Composable {
            SwitchLine(stringResource(R.string.group_reactions), null, switches.reactions, ink, enabled, GroupGlyphs.smile) {
                onChange(switches.copy(reactions = it))
            }
        }
    if (help) {
        // [head], the help line and the first option have no divider between them, as drawn.
        Column(Modifier.fillMaxWidth().background(cardFill())) {
            head?.invoke()
            Text(
                stringResource(R.string.group_options_help),
                Modifier.fillMaxWidth().padding(start = 16.dp, end = 16.dp, top = 2.dp, bottom = 4.dp),
                color = ink.muted,
                fontSize = 12.5.sp,
            )
            reactions()
        }
    } else {
        reactions()
    }
    SwitchLine(stringResource(R.string.group_stickers), null, switches.stickers, ink, enabled, GroupGlyphs.sticker) {
        onChange(switches.copy(stickers = it))
    }
    SwitchLine(stringResource(R.string.group_files), null, switches.files, ink, enabled, GroupGlyphs.file) {
        onChange(switches.copy(files = it))
    }
    SwitchLine(
        stringResource(if (type == TYPE_CHANNEL) R.string.channel_may_add else R.string.group_may_add),
        null,
        switches.mayAdd,
        ink,
        enabled,
        GroupGlyphs.userPlus,
    ) { onChange(switches.copy(mayAdd = it)) }
    ValueLine(
        stringResource(R.string.group_slow_mode),
        slowLabel(switches.slow),
        ink,
        icon = GroupGlyphs.hour,
        chevron = true,
        onClick = if (enabled) onSlow else null,
    )
}

/**
 * Slow mode's choices in a small dialog. A topic's, given [sameAs], starts with "Same as group", its none, and its
 * Off is 0, which is off even while the group has slow mode on, chosen by the author.
 */
@Composable
internal fun SlowPicker(
    now: Long?,
    ink: Ink,
    onClose: () -> Unit,
    sameAs: String? = null,
    onPick: (Long?) -> Unit,
) {
    val choices = if (sameAs == null) SLOW_CHOICES else listOf(null, 0L) + SLOW_CHOICES.filterNotNull()
    AskDialog(
        stringResource(R.string.group_slow_mode),
        null,
        onClose,
        listOf(DialogAct(stringResource(R.string.group_cancel), onClick = onClose)),
    ) {
        for (choice in choices) {
            Row(
                Modifier.fillMaxWidth().clip(RoundedCornerShape(10.dp)).clickable(role = Role.RadioButton) {
                    onPick(choice)
                }.padding(vertical = 10.dp, horizontal = 4.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text(
                    if (choice == null && sameAs != null) sameAs else slowLabel(choice),
                    Modifier.weight(1f),
                    color = ink.text,
                    fontSize = 16.sp,
                )
                if (choice == now) Glyph(Glyphs.check, kit().accent, 20.dp)
            }
        }
        Spacer(Modifier.height(8.dp))
    }
}

/**
 * Picking people: a search box, the picked as chips that unpick, then your chats and, while searching,
 * the server's directory under its own caption. Tapping a person picks or unpicks them.
 */
@Composable
internal fun PeoplePicker(
    me: String?,
    people: Map<String, AccountBrief>,
    known: List<String>,
    found: List<AccountBrief>,
    picked: MutableList<String>,
    query: String,
    onQuery: (String) -> Unit,
    ink: Ink,
    modifier: Modifier = Modifier,
    leaveOut: Set<String> = emptySet(),
) {
    val wanted = query.trim()
    val mine =
        known
            .filter { it != me && it !in leaveOut }
            .filter { wanted.isEmpty() || matches(people[it], wanted) }
    val strangers = if (wanted.isEmpty()) emptyList() else found.filter { it.id != me && it.id !in known && it.id !in leaveOut }
    val names = remember { mutableStateMapOf<String, String>() }
    for (brief in found) names[brief.id] = brief.display_name?.takeIf { it.isNotBlank() } ?: brief.username
    val kit = kit()
    Column(modifier) {
        Field(
            query,
            onQuery,
            stringResource(R.string.search),
            ink,
            Modifier.padding(start = 12.dp, end = 12.dp, bottom = 6.dp),
            lead = GroupGlyphs.search,
            height = 46.dp,
        )
        if (picked.isNotEmpty()) {
            Row(
                Modifier.fillMaxWidth().horizontalScroll(
                    rememberScrollState(),
                ).padding(start = 12.dp, end = 12.dp, top = 6.dp, bottom = 8.dp),
                horizontalArrangement = Arrangement.spacedBy(6.dp),
            ) {
                for (account in picked.toList()) {
                    Row(
                        Modifier
                            .height(34.dp)
                            .clip(RoundedCornerShape(17.dp))
                            .background(kit.tint)
                            .clickable(role = Role.Button) { picked.remove(account) }
                            .padding(horizontal = 12.dp),
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(6.dp),
                    ) {
                        Text(
                            people[account]?.let { personName(account, people) } ?: names[account].orEmpty(),
                            color = ink.text,
                            fontSize = 14.sp,
                            fontWeight = FontWeight.SemiBold,
                            maxLines = 1,
                        )
                        Glyph(GroupGlyphs.cross, ink.text, 14.dp)
                    }
                }
            }
        }
        LazyColumn(Modifier.fillMaxWidth().weight(1f)) {
            if (mine.isNotEmpty() && wanted.isNotEmpty()) item { SectionTitle(stringResource(R.string.group_your_chats), ink) }
            items(mine, key = { "k$it" }) { account ->
                PickRow(account, personName(account, people), people[account]?.username, account in picked, ink, account != mine.first()) {
                    if (account in picked) picked.remove(account) else picked.add(account)
                }
            }
            if (strangers.isNotEmpty()) {
                item { SectionTitle(stringResource(R.string.group_on_server), ink) }
                items(strangers, key = { "f${it.id}" }) { brief ->
                    PickRow(
                        brief.id,
                        brief.display_name?.takeIf { it.isNotBlank() } ?: brief.username,
                        brief.username,
                        brief.id in picked,
                        ink,
                        brief != strangers.first(),
                    ) {
                        if (brief.id in picked) picked.remove(brief.id) else picked.add(brief.id)
                    }
                }
            }
        }
    }
}

private fun matches(
    brief: AccountBrief?,
    wanted: String,
): Boolean {
    brief ?: return false
    return brief.username.contains(wanted, ignoreCase = true) || brief.display_name?.contains(wanted, ignoreCase = true) == true
}

/** A caption over a list's part, such as Your chats and On this server. */
@Composable
internal fun SectionTitle(
    title: String,
    ink: Ink,
) {
    Text(
        title,
        Modifier.padding(start = 16.dp, end = 16.dp, top = 8.dp, bottom = 4.dp),
        color = ink.muted,
        fontSize = 12.5.sp,
        fontWeight = FontWeight.Bold,
        letterSpacing = 0.4.sp,
    )
}

@Composable
private fun PickRow(
    account: String,
    name: String,
    username: String?,
    chosen: Boolean,
    ink: Ink,
    divider: Boolean,
    onClick: () -> Unit,
) {
    val kit = kit()
    if (divider) Box(Modifier.fillMaxWidth().height(1.dp).background(ink.line))
    Row(
        Modifier.fillMaxWidth().clickable(
            role = Role.Checkbox,
            onClick = onClick,
        ).heightIn(min = 52.dp).padding(horizontal = 14.dp, vertical = 11.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Avatar(account, name, 40.dp, username = username)
        Column(Modifier.weight(1f)) {
            Text(
                name,
                color = ink.text,
                fontSize = 15.5.sp,
                fontWeight = FontWeight.SemiBold,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            if (username != null) Text("@$username", color = ink.muted, fontSize = 12.5.sp, maxLines = 1)
        }
        Box(
            Modifier
                .size(24.dp)
                .clip(CircleShape)
                .background(if (chosen) accent else Color.Transparent)
                .border(2.dp, if (chosen) accent else kit.off, CircleShape),
            contentAlignment = Alignment.Center,
        ) { if (chosen) Glyph(Glyphs.check, onAccent, 14.dp, 3f) }
    }
}

/** What every group screen reads: the groups' state, the engine, and who this account is. */
internal class GroupHost(
    val state: GroupState,
    val engine: Engine,
    val me: () -> String?,
)

internal val LocalGroups = staticCompositionLocalOf<GroupHost?> { null }

/** A group's members and roles, asked when a screen showing them opens, and again after a membership change. */
@Composable
internal fun membersOf(
    chat: String,
    host: GroupHost,
): List<MemberItem>? {
    val list = host.state.members[chat]
    LaunchedEffect(chat, list == null) {
        if (list == null) host.engine.send(Command.Members(chat))
    }
    return list
}

/** A count as people read it, with its digit groups: 1,204. */
internal fun exact(count: Long): String = java.text.NumberFormat.getIntegerInstance().format(count)

@Composable
internal fun memberCountText(
    type: Int,
    count: Long,
): String =
    if (type == TYPE_CHANNEL) {
        pluralStringResource(R.plurals.channel_subscriber_count, count.toInt(), exact(count))
    } else {
        pluralStringResource(R.plurals.group_member_count, count.toInt(), exact(count))
    }

@Composable
internal fun roleText(
    type: Int,
    role: String,
): String =
    stringResource(
        when (role) {
            "owner" -> R.string.role_owner
            "moderator" -> R.string.role_moderator
            else -> if (type == TYPE_CHANNEL) R.string.role_subscriber else R.string.role_member
        },
    )

/** Who is typing in one place: "Sara is typing", or "Sara and 2 others are typing". */
@Composable
internal fun typingText(
    typists: List<Typist>,
    people: Map<String, AccountBrief>,
): String? {
    val names = typists.map { personName(it.account, people) }.filter { it.isNotEmpty() }
    return when (names.size) {
        0 -> null
        1 -> stringResource(R.string.group_typing_one, names[0])
        else -> pluralStringResource(R.plurals.group_typing_others, names.size - 1, names[0], names.size - 1)
    }
}

/** Which part of a group's page opens first: the page itself, or straight to its Requests. */
internal enum class GroupSub { Page, Requests, Members, History, Options, Edit, Add }

/**
 * A group's or channel's page, like a person's profile: the big photo, then rounded cards with the
 * bio and links, what this account may manage, and the members under tabs All, Moderators and
 * Members (Subscribers), each online first, with a search inside the open tab.
 */
@Composable
internal fun GroupPage(
    chat: String,
    start: GroupSub,
    people: Map<String, AccountBrief>,
    known: List<String>,
    found: List<AccountBrief>,
    onSearch: (String) -> Unit,
    onBack: () -> Unit,
    onProfile: (String) -> Unit,
    onMessage: (String) -> Unit,
) {
    val host = LocalGroups.current ?: return
    val groups = host.state
    val me = host.me()
    val ink = ink()
    val info = groups.info(chat)
    var sub by remember(chat) { mutableStateOf(start) }
    if (info == null) {
        Over(onBack) { PageBar("", ink, onBack) }
        return
    }
    val type = info.type
    val back: () -> Unit = { if (sub == GroupSub.Page || sub == start) onBack() else sub = GroupSub.Page }
    when (sub) {
        GroupSub.Requests -> RequestsPage(chat, people, ink, back)
        GroupSub.History -> HistoryPage(chat, type, me, people, ink, back)
        GroupSub.Options -> OptionsPage(chat, info, ink, back)
        GroupSub.Edit -> EditGroupPage(chat, type, ink, back)
        GroupSub.Add -> AddPeoplePage(chat, me, people, known, found, onSearch, ink, back)
        GroupSub.Page, GroupSub.Members -> MainGroupPage(chat, info, people, ink, onBack, onProfile, onMessage) { sub = it }
    }
}

@Composable
private fun MainGroupPage(
    chat: String,
    info: GroupInfo,
    people: Map<String, AccountBrief>,
    ink: Ink,
    onBack: () -> Unit,
    onProfile: (String) -> Unit,
    onMessage: (String) -> Unit,
    onSub: (GroupSub) -> Unit,
) {
    val host = LocalGroups.current ?: return
    val groups = host.state
    val me = host.me()
    val type = info.type
    val channel = type == TYPE_CHANNEL
    val members = membersOf(chat, host)
    val profile = groups.profiles[chat]
    val name = groups.name(chat).orEmpty()
    val presence = LocalProfiles.current?.presence
    val kit = kit()
    var tab by remember { mutableIntStateOf(0) }
    var query by remember { mutableStateOf("") }
    var acting by remember { mutableStateOf<MemberItem?>(null) }
    var asking by remember { mutableStateOf<String?>(null) }
    var picking by remember { mutableStateOf(false) }
    val context = LocalContext.current
    Over(onBack, kit.ground) {
        Column(Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(bottom = 12.dp)) {
            GroupCover(chat, name, info, onBack, if (groups.managing(chat)) ({ onSub(GroupSub.Edit) }) else null)
            Spacer(Modifier.height(6.dp))
            val bio = profile?.bio?.takeIf { it.isNotBlank() }
            val links = profile?.link_list.orEmpty()
            if (bio != null || links.isNotEmpty()) {
                GroupCard(stringResource(R.string.group_about), ink) {
                    if (bio != null) {
                        Text(
                            bio,
                            Modifier.fillMaxWidth().background(cardFill()).padding(horizontal = 14.dp, vertical = 14.dp),
                            color = ink.text,
                            fontSize = 15.sp,
                        )
                    }
                    for (link in links) {
                        ValueLine(link.name, null, ink, icon = Glyphs.link) {
                            runCatching {
                                context.startActivity(
                                    android.content.Intent(android.content.Intent.ACTION_VIEW, android.net.Uri.parse(link.url)),
                                )
                            }
                        }
                    }
                }
            }
            profile?.rules?.takeIf { it.isNotBlank() }?.let { RulesCard(it, ink) }
            val mayAdd = groups.mayAdd(chat)
            val managing = groups.managing(chat)
            if (mayAdd || managing) {
                GroupCard(null, ink) {
                    if (mayAdd) {
                        ValueLine(
                            stringResource(R.string.group_requests),
                            null,
                            ink,
                            icon = GroupGlyphs.mail,
                            badge = info.requestCount,
                            chevron = true,
                        ) { onSub(GroupSub.Requests) }
                    }
                    if (managing) {
                        ValueLine(stringResource(R.string.group_history), null, ink, icon = GroupGlyphs.clock, chevron = true) {
                            onSub(GroupSub.History)
                        }
                        ValueLine(stringResource(R.string.group_options), null, ink, icon = GroupGlyphs.sliders, chevron = true) {
                            onSub(GroupSub.Options)
                        }
                    }
                }
            }
            val all = members.orEmpty()
            val ranked =
                all.sortedWith(
                    compareBy<MemberItem>(
                        { stateRank(presence?.get(it.account)?.state) },
                        { -(presence?.get(it.account)?.in_app_at ?: 0L) },
                        { personName(it.account, people).lowercase() },
                    ),
                )
            val moderators = ranked.filter { it.role == "owner" || it.role == "moderator" }
            val plain = ranked.filter { it.role == "member" }
            val lists = listOf(ranked, moderators, plain)
            val titles =
                listOf(
                    stringResource(R.string.group_tab_all),
                    stringResource(R.string.group_tab_moderators),
                    stringResource(if (channel) R.string.channel_tab_subscribers else R.string.group_tab_members),
                )
            val count = info.memberCount ?: all.size.toLong()
            GroupCard(memberCountText(type, count), ink) {
                Column(Modifier.fillMaxWidth().background(cardFill())) {
                    if (mayAdd) {
                        ValueLine(stringResource(R.string.group_add_people), null, ink, tone = kit.accent, icon = GroupGlyphs.userPlus) {
                            onSub(GroupSub.Add)
                        }
                    }
                    Field(
                        query,
                        { query = it },
                        stringResource(R.string.group_search_members),
                        ink,
                        Modifier.padding(start = 12.dp, end = 12.dp, top = 10.dp, bottom = 2.dp),
                        lead = GroupGlyphs.search,
                        height = 44.dp,
                    )
                    Row(
                        Modifier.fillMaxWidth().padding(horizontal = 6.dp, vertical = 8.dp),
                        horizontalArrangement = Arrangement.spacedBy(4.dp),
                    ) {
                        for (at in 0..2) {
                            val chosen = tab == at
                            Column(
                                Modifier
                                    .weight(1f)
                                    .height(48.dp)
                                    .clip(RoundedCornerShape(18.dp))
                                    .background(if (chosen) kit.tint else Color.Transparent)
                                    .clickable(role = Role.Tab) { tab = at }
                                    .padding(horizontal = 4.dp),
                                horizontalAlignment = Alignment.CenterHorizontally,
                                verticalArrangement = Arrangement.spacedBy(3.dp, Alignment.CenterVertically),
                            ) {
                                Text(
                                    titles[at],
                                    color = if (chosen) ink.text else ink.muted,
                                    fontSize = 13.sp,
                                    fontWeight = FontWeight.SemiBold,
                                    lineHeight = 14.sp,
                                    maxLines = 1,
                                )
                                Box(
                                    Modifier
                                        .height(18.dp)
                                        .widthIn(min = 18.dp)
                                        .clip(RoundedCornerShape(9.dp))
                                        .background(if (chosen) ink.text else ink.muted)
                                        .padding(horizontal = 6.dp),
                                    contentAlignment = Alignment.Center,
                                ) {
                                    Text(
                                        exact(lists[at].size.toLong()),
                                        color = if (chosen) kit.tint else ink.page,
                                        fontSize = 11.sp,
                                        fontWeight = FontWeight.Bold,
                                        lineHeight = 12.sp,
                                    )
                                }
                            }
                        }
                    }
                }
                if (members == null) {
                    Box(
                        Modifier.fillMaxWidth().background(cardFill()).padding(20.dp),
                        contentAlignment = Alignment.Center,
                    ) { Spinner(ink.muted) }
                }
                val wanted = query.trim()
                for (member in lists[tab].filter { wanted.isEmpty() || matches(people[it.account], wanted) }) {
                    MemberRow(member, type, me, people, presence?.get(member.account), ink) {
                        if (member.account == me) onProfile(member.account) else acting = member
                    }
                }
            }
            GroupCard(null, ink) {
                ValueLine(
                    stringResource(if (channel) R.string.channel_leave else R.string.group_leave),
                    null,
                    ink,
                    tone = kit.red,
                    icon = GroupGlyphs.out,
                ) { asking = "leave" }
                if (groups.owning(chat)) {
                    ValueLine(
                        stringResource(if (channel) R.string.channel_delete else R.string.group_delete),
                        null,
                        ink,
                        tone = kit.red,
                        icon = GroupGlyphs.trash,
                    ) { asking = "delete" }
                }
            }
        }
    }
    acting?.let { member ->
        MemberSheet(chat, type, member, people, ink, { acting = null }, { onProfile(member.account) }) { onMessage(member.account) }
    }
    if (picking) {
        OwnerPicker(members.orEmpty(), me, people, ink, { picking = false }) { account ->
            picking = false
            host.engine.send(Command.SetRole(chat, account, "owner"))
        }
    }
    asking?.let { what ->
        val owners = members.orEmpty().count { it.role == "owner" }
        val last = what == "leave" && groups.owning(chat) && owners <= 1
        val close = { asking = null }
        val act = {
            asking = null
            host.engine.send(if (what == "delete") Command.RemoveChat(chat) else Command.Leave(chat))
            onBack()
        }
        val cancel = DialogAct(stringResource(R.string.group_cancel), onClick = close)
        when {
            what == "delete" ->
                AskDialog(
                    stringResource(R.string.group_delete_ask, name),
                    stringResource(R.string.group_delete_body),
                    close,
                    listOf(cancel, DialogAct(stringResource(R.string.group_delete_button), red = true, onClick = act)),
                )
            last ->
                AskDialog(
                    stringResource(R.string.group_leave_last_title),
                    stringResource(R.string.group_leave_last, name),
                    close,
                    listOf(
                        cancel,
                        DialogAct(stringResource(R.string.group_pick_owner)) {
                            asking = null
                            picking = true
                        },
                        DialogAct(stringResource(R.string.group_delete_leave), red = true, onClick = act),
                    ),
                )
            else ->
                AskDialog(
                    stringResource(R.string.group_leave_ask, name),
                    null,
                    close,
                    listOf(cancel, DialogAct(stringResource(R.string.group_leave_button), red = true, onClick = act)),
                )
        }
    }
}

/**
 * The only owner's Pick an owner: the members as a list, tapping one making them owner, chosen by the author. The
 * person stays in the group and taps Leave again afterwards.
 */
@Composable
private fun OwnerPicker(
    members: List<MemberItem>,
    me: String?,
    people: Map<String, AccountBrief>,
    ink: Ink,
    onClose: () -> Unit,
    onPick: (String) -> Unit,
) {
    val list = members.filter { it.account != me && it.role != "owner" }.sortedBy { personName(it.account, people).lowercase() }
    AskDialog(
        stringResource(R.string.group_pick_owner),
        null,
        onClose,
        listOf(DialogAct(stringResource(R.string.group_cancel), onClick = onClose)),
    ) {
        Column(Modifier.fillMaxWidth().heightIn(max = 360.dp).verticalScroll(rememberScrollState())) {
            for (member in list) {
                Row(
                    Modifier.fillMaxWidth().clip(RoundedCornerShape(10.dp)).clickable(role = Role.Button) {
                        onPick(member.account)
                    }.padding(vertical = 10.dp, horizontal = 4.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Text(
                        personName(member.account, people),
                        Modifier.weight(1f),
                        color = ink.text,
                        fontSize = 16.sp,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                }
            }
        }
        Spacer(Modifier.height(8.dp))
    }
}

/** Online first, then in a call, then away, then offline. */
private fun stateRank(state: String?) =
    when (state) {
        "online" -> 0
        "call" -> 1
        "away" -> 2
        else -> 3
    }

/** The big photo, filling the width, with the name and the member count over its foot, back and the pencil over its top. */
@Composable
private fun GroupCover(
    chat: String,
    name: String,
    info: GroupInfo,
    onBack: () -> Unit,
    onEdit: (() -> Unit)?,
) {
    val host = LocalGroups.current ?: return
    val photo = host.state.profiles[chat]?.photo
    val rtl = LocalLayoutDirection.current == LayoutDirection.Rtl
    Box(
        Modifier
            .fillMaxWidth()
            .height(300.dp)
            .background(
                Brush.linearGradient(
                    listOf(accent, tile),
                    start = Offset(0f, 0f),
                    end = Offset(Float.POSITIVE_INFINITY, Float.POSITIVE_INFINITY),
                ),
            ),
    ) {
        val image = photo?.let { groupPhoto(chat, it, host.state, host.engine) }
        val blur = photo?.blurhash?.let { blurImage(it) }
        when {
            image != null -> Image(image, null, Modifier.fillMaxSize(), contentScale = ContentScale.Crop)
            blur != null -> Image(blur, null, Modifier.fillMaxSize(), contentScale = ContentScale.Crop)
            else ->
                Text(
                    name.firstOrNull()?.uppercaseChar()?.toString() ?: "",
                    Modifier.align(Alignment.Center),
                    color = Color.White,
                    fontSize = 96.sp,
                    fontWeight = FontWeight.SemiBold,
                )
        }
        Box(Modifier.fillMaxWidth().height(80.dp).background(Brush.verticalGradient(listOf(Color(0x66000000), Color.Transparent))))
        Box(
            Modifier
                .align(Alignment.BottomStart)
                .fillMaxWidth()
                .height(110.dp)
                .background(Brush.verticalGradient(listOf(Color.Transparent, Color(0x73000000)))),
        )
        Row(Modifier.fillMaxWidth().padding(6.dp), verticalAlignment = Alignment.CenterVertically) {
            IconButton(stringResource(R.string.back), onBack, 44.dp) { BackMark(Color.White, rtl) }
            Spacer(Modifier.weight(1f))
            if (onEdit != null) IconButton(stringResource(R.string.group_edit), onEdit, 44.dp) { Glyph(GroupGlyphs.pencil, Color.White) }
        }
        Column(Modifier.align(Alignment.BottomStart).padding(start = 18.dp, end = 18.dp, bottom = 14.dp)) {
            Text(
                name,
                color = Color.White,
                fontSize = 24.sp,
                fontWeight = FontWeight.SemiBold,
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
            )
            info.memberCount?.let {
                Text(
                    memberCountText(info.type, it),
                    color = Color.White,
                    fontSize = 14.sp,
                    fontWeight = FontWeight.Medium,
                )
            }
        }
    }
}

/** A member's row: photo with its dot, name, and under it an owner's or moderator's role and, while offline, when they were last here. */
@Composable
private fun MemberRow(
    member: MemberItem,
    type: Int,
    me: String?,
    people: Map<String, AccountBrief>,
    presence: PresenceItem?,
    ink: Ink,
    onClick: () -> Unit,
) {
    val name = if (member.account == me) stringResource(R.string.chat_you) else personName(member.account, people)
    val role = if (member.role == "member") null else roleText(type, member.role)
    val here = presence?.state.let { it == "online" || it == "call" || it == "away" }
    val last = if (here || member.account == me) null else presenceLine(presence)
    val line = listOfNotNull(role, last).joinToString(" · ").ifEmpty { null }
    val fill = cardFill()
    Row(
        Modifier
            .fillMaxWidth()
            .background(fill)
            .clickable(role = Role.Button, onClick = onClick)
            .heightIn(min = 62.dp)
            .padding(horizontal = 14.dp, vertical = 11.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Box {
            Avatar(member.account, name, 40.dp, badgeRing = fill, badgeSize = 11.dp)
            PresenceDot(presence?.state, 7.dp, 2.dp, fill, Modifier.align(Alignment.BottomEnd))
        }
        Column(Modifier.weight(1f)) {
            Text(
                name,
                color = ink.text,
                fontSize = 15.5.sp,
                fontWeight = FontWeight.SemiBold,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            if (line != null) Text(line, color = ink.muted, fontSize = 12.5.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
    }
}

/**
 * A member's actions, by this account's role and theirs: View profile, whose Back returns here, Message,
 * then the role changes only owners make, and Remove for owners and moderators, never on an owner by a moderator.
 */
@Composable
private fun MemberSheet(
    chat: String,
    type: Int,
    member: MemberItem,
    people: Map<String, AccountBrief>,
    ink: Ink,
    onClose: () -> Unit,
    onProfile: () -> Unit,
    onMessage: () -> Unit,
) {
    val host = LocalGroups.current ?: return
    val groups = host.state
    val kit = kit()
    val name = personName(member.account, people)
    val presence = LocalProfiles.current?.presence?.get(member.account)
    var removing by remember { mutableStateOf(false) }
    FootSheet(onClose) {
        Row(
            Modifier.fillMaxWidth().background(kit.card).padding(start = 18.dp, end = 18.dp, top = 6.dp, bottom = 12.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Avatar(member.account, name, 48.dp)
            Column(Modifier.padding(start = 12.dp)) {
                Text(name, color = ink.text, fontSize = 17.sp, fontWeight = FontWeight.SemiBold)
                val sub = listOfNotNull(people[member.account]?.username?.let { "@$it" }, presenceLine(presence)).joinToString(" · ")
                if (sub.isNotEmpty()) Text(sub, color = ink.muted, fontSize = 13.sp)
            }
        }
        ValueLine(stringResource(R.string.group_view_profile), null, ink, icon = GroupGlyphs.user, onClick = onProfile)
        ValueLine(stringResource(R.string.profile_message), null, ink, icon = GroupGlyphs.bubble, onClick = onMessage)

        fun role(to: String) {
            host.engine.send(Command.SetRole(chat, member.account, to))
            onClose()
        }
        if (groups.owning(chat)) {
            if (member.role != "moderator") {
                ValueLine(stringResource(R.string.group_make_moderator), null, ink, icon = GroupGlyphs.shield) { role("moderator") }
            }
            if (member.role != "owner") {
                ValueLine(stringResource(R.string.group_make_owner), null, ink, icon = GroupGlyphs.crown) { role("owner") }
            }
            if (member.role != "member") {
                ValueLine(
                    stringResource(if (type == TYPE_CHANNEL) R.string.channel_make_member else R.string.group_make_member),
                    null,
                    ink,
                    icon = GroupGlyphs.user,
                ) { role("member") }
            }
        }
        val mayRemove = groups.owning(chat) || (groups.managing(chat) && member.role == "member")
        if (mayRemove) {
            ValueLine(
                stringResource(if (type == TYPE_CHANNEL) R.string.channel_remove_member else R.string.group_remove_member),
                null,
                ink,
                tone = kit.red,
                icon = GroupGlyphs.userCross,
            ) { removing = true }
        }
    }
    if (removing) {
        AskDialog(
            stringResource(R.string.group_remove_ask, name),
            null,
            { removing = false },
            listOf(
                DialogAct(stringResource(R.string.group_cancel)) { removing = false },
                DialogAct(stringResource(R.string.group_remove), red = true) {
                    removing = false
                    host.engine.send(Command.RemoveMembers(chat, listOf(member.account)))
                    onClose()
                },
            ),
        )
    }
}

/** The waiting join requests, each with its note, Decline and Let in. */
@Composable
private fun RequestsPage(
    chat: String,
    people: Map<String, AccountBrief>,
    ink: Ink,
    onBack: () -> Unit,
) {
    val host = LocalGroups.current ?: return
    LaunchedEffect(chat) { host.engine.send(Command.Requests(chat)) }
    val list = host.state.requests[chat]
    val kit = kit()
    Over(onBack) {
        PageBar(stringResource(R.string.group_requests), ink, onBack, count = list?.size?.takeIf { it > 0 }?.let { exact(it.toLong()) })
        LazyColumn(Modifier.fillMaxWidth().weight(1f)) {
            if (list == null) {
                item { Box(Modifier.fillMaxWidth().padding(24.dp), contentAlignment = Alignment.Center) { Spinner(ink.muted) } }
            } else if (list.isEmpty()) {
                item { Text(stringResource(R.string.group_no_requests), Modifier.padding(20.dp), color = ink.muted, fontSize = 15.sp) }
            }
            items(list.orEmpty(), key = { it.account }) { request ->
                val name = request.name?.takeIf { it.isNotBlank() } ?: personName(request.account, people)
                Column(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 12.dp)) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Avatar(request.account, name, 44.dp)
                        Column(Modifier.weight(1f).padding(start = 12.dp)) {
                            Text(name, color = ink.text, fontSize = 16.sp, fontWeight = FontWeight.SemiBold)
                            Text(
                                listOfNotNull(people[request.account]?.username?.let { "@$it" }, shortTime(request.at)).joinToString(" · "),
                                color = ink.muted,
                                fontSize = 13.sp,
                            )
                        }
                    }
                    request.note?.takeIf { it.isNotBlank() }?.let {
                        Text(
                            it,
                            Modifier.padding(
                                top = 8.dp,
                            ).fillMaxWidth().clip(
                                RoundedCornerShape(12.dp),
                            ).background(kit.other).padding(horizontal = 12.dp, vertical = 8.dp),
                            color = ink.text,
                            fontSize = 14.5.sp,
                        )
                    }
                    Row(Modifier.padding(top = 10.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        WideButton(stringResource(R.string.group_decline), ink, strong = false, modifier = Modifier.weight(1f)) {
                            host.engine.send(Command.DeclineAsk(chat, request.account))
                        }
                        WideButton(stringResource(R.string.group_let_in), ink, modifier = Modifier.weight(1f)) {
                            host.engine.send(Command.LetIn(chat, request.account))
                        }
                    }
                }
                Box(Modifier.fillMaxWidth().height(1.dp).background(ink.line))
            }
        }
    }
}

/**
 * Who did what, newest first, from this device's archive: owners and moderators only. The group's page
 * shows its own rows and a topic's page that topic's, as the author chose; a delete or edit shows what it
 * removed or changed, opened by core.
 */
@Composable
private fun HistoryPage(
    chat: String,
    type: Int,
    me: String?,
    people: Map<String, AccountBrief>,
    ink: Ink,
    onBack: () -> Unit,
    topic: String? = null,
) {
    val host = LocalGroups.current ?: return
    var rows by remember { mutableStateOf<List<ChatEvent>?>(null) }
    val view = androidx.compose.ui.platform.LocalView.current

    // A topic's page also shows the group's row that added it, as "created the topic", chosen by the author.
    fun mine(event: ChatEvent) = event.topic == topic || (topic != null && event.action == 10 && event.target == topic)
    LaunchedEffect(chat, topic) { host.engine.history(chat) { list -> view.post { rows = list.filter(::mine) } } }
    val topics = host.state.topicNames[chat].orEmpty()
    val kit = kit()
    val today = stringResource(R.string.chat_today)
    val yesterday = stringResource(R.string.yesterday)
    Over(onBack) {
        PageBar(stringResource(R.string.group_history), ink, onBack)
        LazyColumn(Modifier.fillMaxWidth().weight(1f)) {
            val list = rows
            if (list == null) {
                item { Box(Modifier.fillMaxWidth().padding(24.dp), contentAlignment = Alignment.Center) { Spinner(ink.muted) } }
            } else if (list.isEmpty()) {
                item { Text(stringResource(R.string.group_no_history), Modifier.padding(20.dp), color = ink.muted, fontSize = 15.sp) }
            }
            items(list.orEmpty(), key = { it.n }) { event ->
                Row(
                    Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 10.dp),
                    horizontalArrangement = Arrangement.spacedBy(12.dp),
                ) {
                    Box(Modifier.size(34.dp).clip(CircleShape).background(kit.tint), contentAlignment = Alignment.Center) {
                        Glyph(historyGlyph(event.action), kit.accent, 18.dp)
                    }
                    Column(Modifier.weight(1f).padding(top = 2.dp)) {
                        Text(
                            historyText(event, type, me, people, topics, topic != null),
                            color = ink.text,
                            fontSize = 14.5.sp,
                            lineHeight = 20.sp,
                        )
                        if (event.copy_list.isNotEmpty()) HistoryCopies(event, host.state.copies[chat to event.n], ink)
                        Text(
                            historyWhen(event.server_at, today, yesterday),
                            Modifier.padding(top = 2.dp),
                            color = ink.muted,
                            fontSize = 12.5.sp,
                        )
                    }
                }
            }
        }
    }
}

/** Each history line's icon, as the design draws them: roles a shield, deletes a bin, joins a person with a plus. */
private fun historyGlyph(action: Int): List<String> =
    when (action) {
        0 -> peopleGlyph
        1, 4, 5 -> GroupGlyphs.userPlus
        2 -> GroupGlyphs.userCross
        3 -> GroupGlyphs.out
        6 -> GroupGlyphs.crown
        7, 8 -> GroupGlyphs.shield
        9 -> GroupGlyphs.trash
        10, 11, 12, 13, 16 -> GroupGlyphs.hash
        17 -> GroupGlyphs.sliders
        18, 19 -> GroupGlyphs.shield
        20 -> GroupGlyphs.pencil
        else -> GroupGlyphs.mail
    }

/** "Today 12:20", "Yesterday 18:05", or "3 Nov" for anything older. */
private fun historyWhen(
    at: Long,
    today: String,
    yesterday: String,
): String {
    val zone = ZoneId.systemDefault()
    val day = Instant.ofEpochMilli(at).atZone(zone).toLocalDate()
    val now = java.time.LocalDate.now(zone)
    return when (day) {
        now -> "$today ${shortTime(at)}"
        now.minusDays(1) -> "$yesterday ${shortTime(at)}"
        else -> DateTimeFormatter.ofPattern(if (day.year == now.year) "d MMM" else "d MMM yyyy").format(day)
    }
}

/**
 * The chat's options on the ground under a card: topics on or off for owners and moderators, findable
 * and the member switches and slow mode for owners, each saved as it changes.
 */
@Composable
private fun OptionsPage(
    chat: String,
    info: GroupInfo,
    ink: Ink,
    onBack: () -> Unit,
) {
    val host = LocalGroups.current ?: return
    val groups = host.state
    val owning = groups.owning(chat)
    val switches = Switches.of(info.type, info.options)
    var slowPicking by remember { mutableStateOf(false) }

    fun save(next: Switches) = host.engine.send(Command.SetOptions(chat, next.options(info.type)))
    Over(onBack, kit().ground) {
        PageBar(stringResource(R.string.group_options), ink, onBack)
        Column(Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(top = 6.dp)) {
            Text(
                stringResource(R.string.group_options_help),
                Modifier.padding(horizontal = 16.dp, vertical = 6.dp),
                color = ink.muted,
                fontSize = 13.5.sp,
            )
            GroupCard(null, ink) {
                SwitchLine(
                    stringResource(R.string.group_topics),
                    stringResource(R.string.group_topics_help),
                    info.topicsOn,
                    ink,
                    groups.managing(chat),
                    GroupGlyphs.hash,
                ) { on ->
                    host.engine.send(Command.Topics(chat, on, groups.topics(chat)))
                }
                SwitchLine(
                    stringResource(R.string.group_findable),
                    stringResource(R.string.group_findable_help),
                    info.findable != null,
                    ink,
                    owning,
                    GroupGlyphs.search,
                ) { on ->
                    val profile = groups.profiles[chat]
                    host.engine.send(Command.Findable(chat, on, if (on) profile?.name else null, if (on) profile?.bio else null))
                }
                OptionLines(info.type, switches, ink, owning, help = false, onChange = ::save) { slowPicking = true }
            }
        }
    }
    if (slowPicking) {
        SlowPicker(switches.slow, ink, { slowPicking = false }) {
            slowPicking = false
            save(switches.copy(slow = it))
        }
    }
}

/** The name, bio, links and photo, saved with the check; the photo goes up at once once cropped. */
@Composable
private fun EditGroupPage(
    chat: String,
    type: Int,
    ink: Ink,
    onBack: () -> Unit,
) {
    val host = LocalGroups.current ?: return
    val profile = host.state.profiles[chat]
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val kit = kit()
    var name by remember { mutableStateOf(profile?.name.orEmpty()) }
    var bio by remember { mutableStateOf(profile?.bio.orEmpty()) }
    var rules by remember { mutableStateOf(profile?.rules.orEmpty()) }
    var links by remember { mutableStateOf(profile?.link_list.orEmpty()) }
    var cropping by remember { mutableStateOf<Bitmap?>(null) }
    var sent by remember { mutableStateOf<Bitmap?>(null) }
    val picker =
        rememberLauncherForActivityResult(ActivityResultContracts.PickVisualMedia()) { uri ->
            if (uri != null) scope.launch { cropping = withContext(Dispatchers.IO) { forCrop(context, uri) } }
        }
    val source = cropping
    if (source != null) {
        BackHandler { cropping = null }
        Crop(source, { cropping = null }) { square ->
            cropping = null
            sent = square
            scope.launch {
                val (jpeg, thumbnail) = withContext(Dispatchers.Default) { encode(square) }
                if (jpeg != null) host.engine.send(Command.ChatPhoto(chat, jpeg, thumbnail))
            }
        }
        return
    }
    Over(onBack) {
        val ready = name.trim().isNotEmpty()
        PageBar(stringResource(R.string.group_edit), ink, onBack) {
            IconButton(stringResource(R.string.group_save), {
                if (ready) {
                    val said = rules.trim().ifEmpty { null }
                    host.engine.send(Command.ChatProfile(chat, name.trim(), bio.trim().ifEmpty { null }, usable(links), said))
                    // Their writer has seen them, chosen by the author.
                    if (said != null) markRulesShown(context, host.me(), chat, null)
                    onBack()
                }
            }, 44.dp) { Glyph(Glyphs.check, if (ready) kit.accent else ink.muted, 26.dp) }
        }
        Column(
            Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(start = 16.dp, end = 16.dp, top = 4.dp, bottom = 24.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Row(Modifier.fillMaxWidth().padding(top = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                // Only the photo is clipped to its circle, so the camera badge on its edge shows whole.
                Box(Modifier.size(64.dp)) {
                    Box(
                        Modifier.matchParentSize().clip(CircleShape).clickable(role = Role.Button) {
                            picker.launch(PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageOnly))
                        },
                    ) {
                        val shown = sent
                        if (shown != null) {
                            Image(remember(shown) { shown.asImageBitmap() }, null, Modifier.fillMaxSize(), contentScale = ContentScale.Crop)
                        } else {
                            GroupAvatar(chat, host.state, host.engine, name, 64.dp)
                        }
                    }
                    Box(
                        Modifier.align(Alignment.BottomEnd).size(22.dp).clip(CircleShape).background(accent),
                        contentAlignment = Alignment.Center,
                    ) {
                        Glyph(Glyphs.cam, onAccent, 13.dp)
                    }
                }
                Field(
                    name,
                    { name = it },
                    stringResource(if (type == TYPE_CHANNEL) R.string.channel_name else R.string.group_name),
                    ink,
                    Modifier.padding(start = 12.dp).weight(1f),
                    max = GROUP_NAME_MAX,
                    count = true,
                )
            }
            Field(bio, { bio = it }, stringResource(R.string.group_bio), ink, single = false, max = GROUP_BIO_MAX)
            Field(
                rules,
                { rules = it },
                stringResource(R.string.topic_rules_hint),
                ink,
                single = false,
                max = RULES_MAX,
                count = true,
                height = 118.dp,
            )
            Links(links, ink, kit.ground) { links = it }
        }
    }
}

/** Adding people: the people picker without those already in, then Add. */
@Composable
private fun AddPeoplePage(
    chat: String,
    me: String?,
    people: Map<String, AccountBrief>,
    known: List<String>,
    found: List<AccountBrief>,
    onSearch: (String) -> Unit,
    ink: Ink,
    onBack: () -> Unit,
) {
    val host = LocalGroups.current ?: return
    val members = membersOf(chat, host)
    val picked = remember { mutableStateListOf<String>() }
    var query by remember { mutableStateOf("") }
    LaunchedEffect(query) {
        delay(300)
        onSearch(query.trim())
    }
    Over(onBack) {
        PageBar(
            stringResource(R.string.group_add_people),
            ink,
            onBack,
            sub = if (picked.isEmpty()) null else pluralStringResource(R.plurals.group_picked, picked.size, picked.size),
        ) {
            BarWord(stringResource(R.string.group_add), picked.isNotEmpty()) {
                host.engine.send(Command.AddMembers(chat, picked.toList()))
                onBack()
            }
        }
        PeoplePicker(me, people, known, found, picked, query, {
            query = it
        }, ink, Modifier.weight(1f), members.orEmpty().map { it.account }.toSet())
    }
}

/**
 * Editing topics as one session in a sheet over the chat, saved with the check: drag to reorder, rename,
 * remove after a warning, and New topic, whose id is asked of the server first.
 */
@Composable
internal fun TopicsSheet(
    chat: String,
    ink: Ink,
    onNew: () -> Unit,
    onClose: () -> Unit,
) {
    val host = LocalGroups.current ?: return
    val groups = host.state
    val kit = kit()
    val start = remember(chat) { groups.topics(chat) }
    var list by remember(chat) { mutableStateOf(start) }
    var naming by remember { mutableStateOf<TopicName?>(null) }
    var removing by remember { mutableStateOf<TopicName?>(null) }
    var dragId by remember { mutableStateOf<String?>(null) }
    var dragBy by remember { mutableStateOf(0f) }
    var rowHeight by remember { mutableIntStateOf(0) }
    val changed = list != start
    FootSheet(onClose) {
        Row(
            Modifier.fillMaxWidth().background(kit.card).padding(start = 18.dp, end = 8.dp, bottom = 6.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text(
                stringResource(R.string.group_topics_title),
                Modifier.weight(1f),
                color = ink.text,
                fontSize = 17.sp,
                fontWeight = FontWeight.SemiBold,
            )
            IconButton(stringResource(R.string.group_save), {
                if (changed) {
                    host.engine.send(
                        Command.Topics(chat, groups.info(chat)?.topicsOn ?: true, list.mapIndexed { at, t -> t.copy(place = at.toLong()) }),
                    )
                }
                onClose()
            }, 44.dp) { Glyph(Glyphs.check, if (changed) kit.accent else ink.muted, 24.dp) }
        }
        Row(
            Modifier.fillMaxWidth().background(kit.card).heightIn(min = 52.dp).padding(horizontal = 14.dp, vertical = 11.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Spacer(Modifier.width(20.dp))
            Column {
                Text(stringResource(R.string.topic_general), color = ink.text, fontSize = 15.5.sp, fontWeight = FontWeight.SemiBold)
                Text(stringResource(R.string.topic_always_first), color = ink.muted, fontSize = 12.5.sp)
            }
        }
        for (topic in list) {
            val dragging = topic.id == dragId
            Row(
                Modifier
                    .fillMaxWidth()
                    .zIndex(if (dragging) 1f else 0f)
                    .graphicsLayer { translationY = if (dragging) dragBy else 0f }
                    .background(kit.card)
                    .onSizeChanged { if (rowHeight == 0) rowHeight = it.height }
                    .padding(start = 4.dp, end = 4.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Box(
                    Modifier.size(44.dp).pointerInput(topic.id) {
                        detectVerticalDragGestures(
                            onDragStart = {
                                dragId = topic.id
                                dragBy = 0f
                            },
                            onDragEnd = {
                                dragId = null
                                dragBy = 0f
                            },
                            onDragCancel = {
                                dragId = null
                                dragBy = 0f
                            },
                        ) { change, amount ->
                            change.consume()
                            dragBy += amount
                            val height = rowHeight.toFloat().takeIf { it > 0f } ?: return@detectVerticalDragGestures
                            val at = list.indexOfFirst { it.id == topic.id }
                            val to =
                                when {
                                    dragBy > height / 2 && at < list.lastIndex -> at + 1
                                    dragBy < -height / 2 && at > 0 -> at - 1
                                    else -> return@detectVerticalDragGestures
                                }
                            list = list.toMutableList().apply { add(to, removeAt(at)) }
                            dragBy += if (to > at) -height else height
                        }
                    },
                    contentAlignment = Alignment.Center,
                ) { Glyph(Glyphs.handle, ink.muted, 20.dp) }
                Text(
                    topic.name,
                    Modifier.weight(1f).padding(horizontal = 6.dp),
                    color = ink.text,
                    fontSize = 15.5.sp,
                    fontWeight = FontWeight.SemiBold,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                IconButton(stringResource(R.string.group_rename_topic), { naming = topic }) { Glyph(GroupGlyphs.pencil, ink.muted, 19.dp) }
                IconButton(stringResource(R.string.group_remove), { removing = topic }) { Glyph(GroupGlyphs.trash, kit.red, 19.dp) }
            }
        }
        if (list.size < TOPICS_MAX) {
            ValueLine(stringResource(R.string.group_new_topic), null, ink, tone = kit.accent, icon = Glyphs.plus, onClick = onNew)
        }
    }
    naming?.let { asked ->
        var name by remember(asked.id) { mutableStateOf(asked.name) }
        val close = { naming = null }
        val trimmed = name.trim()
        AskDialog(
            stringResource(R.string.group_rename_topic),
            null,
            close,
            listOf(
                DialogAct(stringResource(R.string.group_cancel), onClick = close),
                DialogAct(stringResource(R.string.group_save), enabled = trimmed.isNotEmpty()) {
                    list = list.map { if (it.id == asked.id) it.copy(name = trimmed) else it }
                    close()
                },
            ),
        ) {
            Field(
                name,
                {
                    name = it
                },
                stringResource(
                    R.string.group_topic_name,
                ),
                ink,
                Modifier.padding(top = 2.dp, bottom = 12.dp),
                max = TOPIC_NAME_MAX,
                count = true,
            )
        }
    }
    removing?.let { topic ->
        AskDialog(
            stringResource(R.string.group_remove_topic_ask, topic.name),
            stringResource(R.string.group_remove_topic_body),
            { removing = null },
            listOf(
                DialogAct(stringResource(R.string.group_cancel)) { removing = null },
                DialogAct(stringResource(R.string.group_remove), red = true) {
                    list = list.filter { it.id != topic.id }
                    removing = null
                },
            ),
        )
    }
}

/** A group or channel has at most 100 topics, General not counted, each named in 1 to 32 characters. */
private const val TOPICS_MAX = 100
private const val TOPIC_NAME_MAX = 32

/** A bar with the chat's round photo, its name and a line under it, as the chat's own header draws them. */
@Composable
private fun CardBar(
    chat: String,
    name: String,
    sub: String,
    ink: Ink,
    onBack: () -> Unit,
    blurhash: String? = null,
) {
    val host = LocalGroups.current ?: return
    val rtl = LocalLayoutDirection.current == LayoutDirection.Rtl
    Row(Modifier.fillMaxWidth().height(64.dp).padding(start = 4.dp, end = 8.dp), verticalAlignment = Alignment.CenterVertically) {
        IconButton(stringResource(R.string.back), onBack, 44.dp) { BackMark(ink.text, rtl) }
        Box(Modifier.padding(start = 2.dp, end = 10.dp)) { GroupAvatar(chat, host.state, host.engine, name, 44.dp, blurhash = blurhash) }
        Column(Modifier.weight(1f)) {
            Text(name, color = ink.text, fontSize = 17.sp, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text(sub, color = ink.muted, fontSize = 13.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
    }
}

/** The design's card in the middle of an empty chat: a 72 dp photo, the name, a line under it, then [content]. */
@Composable
private fun MiddleCard(
    chat: String,
    name: String,
    line: String,
    ink: Ink,
    blurhash: String? = null,
    content: @Composable ColumnScope.() -> Unit,
) {
    val host = LocalGroups.current ?: return
    Box(
        Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(horizontal = 14.dp, vertical = 24.dp),
        contentAlignment = Alignment.Center,
    ) {
        Column(
            Modifier
                .fillMaxWidth()
                .clip(RoundedCornerShape(20.dp))
                .background(kit().other)
                .padding(start = 16.dp, end = 16.dp, top = 18.dp, bottom = 16.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            GroupAvatar(chat, host.state, host.engine, name, 72.dp, blurhash = blurhash)
            Text(
                name,
                Modifier.padding(top = 10.dp),
                color = ink.text,
                fontSize = 18.sp,
                fontWeight = FontWeight.SemiBold,
                textAlign = TextAlign.Center,
            )
            Text(line, Modifier.padding(top = 4.dp, bottom = 12.dp), color = ink.muted, fontSize = 14.sp, textAlign = TextAlign.Center)
            content()
        }
    }
}

/** A quoted line inside the middle card, such as an invitation's note or a group's bio, on the page's colour. */
@Composable
private fun Quote(
    text: String,
    ink: Ink,
) {
    Text(
        text,
        Modifier.padding(
            bottom = 14.dp,
        ).fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(ink.page).padding(horizontal = 12.dp, vertical = 10.dp),
        color = ink.text,
        fontSize = 14.5.sp,
    )
}

/**
 * An invitation, as a card in the middle of the empty chat it is for: who invited, the name, the
 * photo's BlurHash and the note, with Decline, which declines at once, and Join, or Subscribe for a channel.
 */
@Composable
internal fun InvitationPage(
    chat: String,
    people: Map<String, AccountBrief>,
    onBack: () -> Unit,
) {
    val host = LocalGroups.current ?: return
    val groups = host.state
    val ink = ink()
    val invitation = groups.invitations[chat]
    LaunchedEffect(invitation == null) { if (invitation == null) onBack() }
    invitation ?: return
    val name = invitation.name.orEmpty()
    val channel = invitation.type == TYPE_CHANNEL
    val joining = groups.joining[chat] == true
    val inviter = personName(invitation.inviter, people)
    Over(onBack) {
        CardBar(chat, name, stringResource(R.string.group_invitation), ink, onBack, invitation.blurhash)
        Box(Modifier.fillMaxWidth().weight(1f)) {
            MiddleCard(
                chat,
                name,
                stringResource(if (channel) R.string.channel_invited_to else R.string.group_invited_to, inviter, name),
                ink,
                invitation.blurhash,
            ) {
                invitation.note?.takeIf { it.isNotBlank() }?.let { Quote(it, ink) }
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    WideButton(
                        stringResource(R.string.group_decline),
                        ink,
                        strong = false,
                        enabled = !joining,
                        modifier = Modifier.weight(1f),
                    ) {
                        host.engine.send(Command.AnswerInvite(chat, false))
                        onBack()
                    }
                    WideButton(
                        stringResource(if (channel) R.string.channel_subscribe else R.string.group_join),
                        ink,
                        busy = joining,
                        modifier = Modifier.weight(1f),
                    ) {
                        groups.joining[chat] = true
                        host.engine.send(Command.AnswerInvite(chat, true))
                    }
                }
                Text(
                    stringResource(R.string.group_join_help),
                    Modifier.padding(top = 12.dp),
                    color = ink.muted,
                    fontSize = 12.5.sp,
                    textAlign = TextAlign.Center,
                )
            }
        }
    }
}

/**
 * A findable group or channel from search, or one this account asked to join: its letter, name and
 * bio, then Ask to join, Request sent with Cancel request, or Declined with Ask again.
 */
@Composable
internal fun FoundPage(
    chat: String,
    type: Int,
    name: String,
    bio: String?,
    onBack: () -> Unit,
) {
    val host = LocalGroups.current ?: return
    val groups = host.state
    val ink = ink()
    val kit = kit()
    val asked = groups.asked[chat]
    var noting by remember { mutableStateOf(false) }
    val findable = stringResource(if (type == TYPE_CHANNEL) R.string.channel_findable_channel else R.string.group_findable_group)
    val sub =
        when (asked?.state) {
            "waiting" -> stringResource(R.string.group_request_sent)
            "declined" -> stringResource(R.string.group_declined_state)
            else -> findable
        }
    Over(onBack) {
        CardBar(chat, name, sub, ink, onBack)
        Box(Modifier.fillMaxWidth().weight(1f)) {
            MiddleCard(chat, name, findable, ink) {
                if (!bio.isNullOrBlank()) Quote(bio, ink)
                when (asked?.state) {
                    "waiting" -> {
                        Row(
                            Modifier.padding(bottom = 12.dp),
                            verticalAlignment = Alignment.CenterVertically,
                            horizontalArrangement = Arrangement.spacedBy(6.dp),
                        ) {
                            Glyph(GroupGlyphs.clock, ink.muted, 16.dp)
                            Text(stringResource(R.string.group_request_sent), color = ink.muted, fontSize = 14.sp)
                        }
                        Text(
                            stringResource(R.string.group_sent_help),
                            Modifier.padding(bottom = 12.dp),
                            color = ink.muted,
                            fontSize = 13.sp,
                            textAlign = TextAlign.Center,
                        )
                        WideButton(stringResource(R.string.group_cancel_request), ink, strong = false, big = true, tone = kit.red) {
                            host.engine.send(Command.CancelAsk(chat))
                        }
                    }
                    "declined" -> {
                        Row(
                            Modifier.padding(bottom = 12.dp),
                            verticalAlignment = Alignment.CenterVertically,
                            horizontalArrangement = Arrangement.spacedBy(6.dp),
                        ) {
                            Glyph(GroupGlyphs.cross, kit.red, 16.dp)
                            Text(
                                stringResource(R.string.group_declined_state),
                                color = kit.red,
                                fontSize = 14.sp,
                                fontWeight = FontWeight.SemiBold,
                            )
                        }
                        WideButton(stringResource(R.string.group_ask_again), ink, big = true) { noting = true }
                    }
                    else -> WideButton(stringResource(R.string.group_ask_join), ink, big = true) { noting = true }
                }
            }
        }
    }
    if (noting) {
        var note by remember { mutableStateOf("") }
        FootSheet({ noting = false }, divided = false) {
            Column(
                Modifier.padding(start = 18.dp, end = 18.dp, top = 4.dp, bottom = 6.dp),
                verticalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                Text(stringResource(R.string.group_ask_join), color = ink.text, fontSize = 17.sp, fontWeight = FontWeight.SemiBold)
                Field(note, { note = it }, stringResource(R.string.group_ask_note), ink, single = false, max = NOTE_MAX, count = true)
                Text(stringResource(R.string.group_ask_note_help), color = ink.muted, fontSize = 12.5.sp)
                WideButton(stringResource(R.string.group_ask_send), ink, big = true) {
                    host.engine.send(Command.AskToJoin(chat, note.trim().ifEmpty { null }))
                    noting = false
                }
            }
        }
    }
}

/** A join request's note: plain text the server reads, at most 140 characters, chosen by the author. */
private const val NOTE_MAX = 140

/** General's place in the app: the empty topic, as messages without a topic are General's. */
internal fun wireTopic(topic: String) = topic.ifEmpty { GENERAL_TOPIC }

/** A topic's name: General's own word, or the name its group's profile gives it. */
@Composable
internal fun topicName(
    groups: GroupState,
    chat: String,
    topic: String,
): String =
    if (topic.isEmpty() || topic == GENERAL_TOPIC) {
        stringResource(R.string.topic_general)
    } else {
        groups.topicNames[chat]?.get(topic).orEmpty()
    }

/** A topic's icon colours, one taken from its id, as the design draws them. */
private val TOPIC_TONES =
    listOf(
        Color(0xFFC0567A),
        Color(0xFF2F8F6B),
        Color(0xFFB7791F),
        Color(0xFF7B5CC4),
        Color(0xFF2D7FB8),
        Color(0xFFC4573A),
        Color(0xFF3F8F9E),
    )

internal fun topicTone(topic: String) = TOPIC_TONES[Math.floorMod(topic.hashCode(), TOPIC_TONES.size)]

/** The design's lock, crossed eye and megaphone. */
internal val topicLockGlyph =
    listOf(
        "M7 11h10a2 2 0 0 1 2 2v6a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2v-6a2 2 0 0 1 2-2z",
        "M8 11V8a4 4 0 0 1 8 0v3",
    )
private val eyeOffGlyph =
    listOf(
        "M3 3l18 18",
        "M10.6 6.1A9.8 9.8 0 0 1 12 6c5 0 8.5 4 9.5 6a13 13 0 0 1-2.7 3.4",
        "M6.6 7.6A13.4 13.4 0 0 0 2.5 12c1 2 4.5 6 9.5 6a9.6 9.6 0 0 0 4.4-1.1",
        "M9.9 9.9a3 3 0 0 0 4.2 4.2",
    )

/**
 * A topic's round icon: its photo once kept, its BlurHash while fetched, or a circle in a colour from
 * its id with its name's first letter; General's is a # on the tint, as the author chose.
 */
@Composable
internal fun TopicIcon(
    chat: String,
    topic: String,
    name: String,
    size: Dp,
    modifier: Modifier = Modifier,
) {
    val host = LocalGroups.current ?: return
    val kit = kit()
    val general = topic.isEmpty() || topic == GENERAL_TOPIC
    val photo = host.state.topicProfile(chat, topic)?.photo
    Box(
        modifier.size(size).clip(CircleShape).background(if (general) kit.tint else topicTone(topic)),
        contentAlignment = Alignment.Center,
    ) {
        val image = photo?.let { topicPhoto(chat, wireTopic(topic), it, host.state, host.engine) }
        val blur = photo?.blurhash?.let { blurImage(it) }
        when {
            image != null -> Image(image, null, Modifier.fillMaxSize(), contentScale = ContentScale.Crop)
            blur != null -> Image(blur, null, Modifier.fillMaxSize(), contentScale = ContentScale.Crop)
            general -> Glyph(GroupGlyphs.hash, kit.accent, size * 0.46f)
            else ->
                Text(
                    name.firstOrNull()?.uppercaseChar()?.toString() ?: "",
                    color = Color.White,
                    fontSize = (size.value * 0.4f).sp,
                    fontWeight = FontWeight.SemiBold,
                )
        }
    }
}

@Composable
private fun topicPhoto(
    chat: String,
    topic: String,
    photo: ShownChatPhoto,
    groups: GroupState,
    engine: Engine,
): ImageBitmap? {
    val version = groups.kept[photo.id] ?: 0
    var image by remember(photo.id) { mutableStateOf<ImageBitmap?>(null) }
    LaunchedEffect(photo.id, version) {
        val loaded =
            withContext(Dispatchers.IO) {
                val file = engine.photoFile(photo.id)
                if (file.exists()) BitmapFactory.decodeFile(file.path)?.asImageBitmap() else null
            }
        image = loaded
        if (loaded == null) engine.topicPhoto(chat, topic, photo.id)
    }
    return image
}

/** The permission names "Files and media" covers, without their chat type's prefix, as a topic names them. */
private val TOPIC_FILE_KINDS = listOf("voice", "image", "music", "video", "doc", "file", "media_bundle").map { "message.send.$it" }

/** Whether the chat's members may do [name], unprefixed: its options' override, else the built-in member role. */
internal fun chatMay(
    type: Int,
    options: ChatOptions?,
    name: String,
): Boolean {
    val full = prefixOf(type) + name
    if (options?.deny?.contains(full) == true) return false
    if (options?.allow?.contains(full) == true) return true
    return !name.endsWith(".others")
}

/** Whether a topic's members may do [name], unprefixed: the topic's own override, else the chat's. */
internal fun topicMay(
    type: Int,
    options: ChatOptions?,
    setting: TopicSetting,
    name: String,
): Boolean =
    when (name) {
        in setting.allow -> true
        in setting.deny -> false
        else -> chatMay(type, options, name)
    }

/**
 * A topic's options with [names] set to [on]: an override only where that differs from the chat's, so a
 * name the topic agrees on follows the group's later changes.
 */
private fun topicWith(
    type: Int,
    options: ChatOptions?,
    setting: TopicSetting,
    names: List<String>,
    on: Boolean,
): TopicOptionsBody {
    val allow = setting.allow.filter { it !in names }.toMutableList()
    val deny = setting.deny.filter { it !in names }.toMutableList()
    for (name in names) {
        if (chatMay(type, options, name) != on) {
            if (on) {
                allow.add(name)
            } else {
                deny.add(name)
            }
        }
    }
    return setting.options.copy(allow = allow, deny = deny)
}

/** Whether rules were shown on this device, kept in the app's own settings only, as the author chose. */
internal fun rulesShown(
    context: android.content.Context,
    me: String?,
    chat: String,
    topic: String?,
): Boolean =
    context.getSharedPreferences(
        RULES_SHOWN,
        android.content.Context.MODE_PRIVATE,
    ).getBoolean("$me $chat ${topic.orEmpty()}", false)

internal fun markRulesShown(
    context: android.content.Context,
    me: String?,
    chat: String,
    topic: String?,
) {
    context.getSharedPreferences(
        RULES_SHOWN,
        android.content.Context.MODE_PRIVATE,
    ).edit().putBoolean("$me $chat ${topic.orEmpty()}", true).apply()
}

private const val RULES_SHOWN = "rules_shown"

/** Rules shown once, in a sheet with OK: a group's on first opening it, a topic's on first opening that topic. */
@Composable
internal fun RulesSheet(
    title: String,
    rules: String,
    onOk: () -> Unit,
) {
    val ink = ink()
    FootSheet(onOk, divided = false) {
        Column(
            Modifier.fillMaxWidth().padding(start = 18.dp, end = 18.dp, top = 4.dp, bottom = 8.dp),
            verticalArrangement = Arrangement.spacedBy(14.dp),
        ) {
            Text(title, color = ink.text, fontSize = 17.sp, fontWeight = FontWeight.SemiBold)
            Text(rules, color = ink.text, fontSize = 15.sp, lineHeight = 22.sp, style = TextStyle(textDirection = TextDirection.Content))
            WideButton(stringResource(R.string.chat_ok), ink, big = true, onClick = onOk)
        }
    }
}

/**
 * A Rules card: cut after three lines, with Read all opening the rest only when the text is cut, chosen by the author,
 * on the group's and a topic's page.
 */
@Composable
internal fun RulesCard(
    rules: String,
    ink: Ink,
) {
    var all by remember(rules) { mutableStateOf(false) }
    var cut by remember(rules) { mutableStateOf(false) }
    GroupCard(stringResource(R.string.topic_rules), ink) {
        Column(
            Modifier
                .fillMaxWidth()
                .background(cardFill())
                .clickable(role = Role.Button, enabled = cut || all) { all = !all }
                .padding(horizontal = 14.dp, vertical = 14.dp),
        ) {
            Text(
                rules,
                color = ink.text,
                fontSize = 15.sp,
                lineHeight = 21.sp,
                maxLines = if (all) Int.MAX_VALUE else 3,
                overflow = TextOverflow.Ellipsis,
                style = TextStyle(textDirection = TextDirection.Content),
                onTextLayout = { if (!all) cut = it.hasVisualOverflow },
            )
            if (!all && cut) {
                Text(
                    stringResource(R.string.topic_read_all),
                    Modifier.padding(top = 6.dp),
                    color = kit().accent,
                    fontSize = 14.sp,
                    fontWeight = FontWeight.SemiBold,
                )
            }
        }
    }
}

/**
 * A group's or channel's topics as a list, like the chat list, as the author chose: General first, then
 * the topics in order, each with its icon, name, a lock when closed and a crossed eye when hidden, the
 * newest message this device holds with its sender, and its unread count. A long press does nothing.
 */
@Composable
internal fun TopicRows(
    chat: String,
    bubbles: List<Bubble>,
    me: String?,
    people: Map<String, AccountBrief>,
    preview: (Bubble) -> String,
    ink: Ink,
    modifier: Modifier = Modifier,
    onTopic: (String) -> Unit,
) {
    val host = LocalGroups.current ?: return
    val groups = host.state
    val ids = listOf("") + groups.topics(chat).map { it.id }
    val newest = HashMap<String, Bubble>()
    for (bubble in bubbles) {
        if (bubble.thread != null || bubble.notice != null || bubble.kind == CONTENT_REACTION) continue
        newest[bubble.topic.orEmpty()] = bubble
    }
    LazyColumn(modifier.fillMaxWidth()) {
        items(ids, key = { it.ifEmpty { "general" } }) { topic ->
            val setting = groups.setting(chat, topic)
            val name = topicName(groups, chat, topic)
            val last = newest[topic]
            val count = groups.unread.topics[chat to topic] ?: 0
            val mentioned = (groups.unread.topicMentions[chat to topic] ?: 0) > 0
            Row(
                Modifier
                    .fillMaxWidth()
                    .clickable(role = Role.Button) { onTopic(topic) }
                    .graphicsLayer { alpha = if (setting.hidden) 0.62f else 1f }
                    .padding(horizontal = 14.dp, vertical = 9.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                TopicIcon(chat, topic, name, 52.dp)
                Column(Modifier.weight(1f).padding(start = 12.dp)) {
                    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                        Row(Modifier.weight(1f), verticalAlignment = Alignment.CenterVertically) {
                            ChatTypeMark(groups.infos[chat]?.type, ink, 15.dp)
                            Text(
                                name,
                                Modifier.weight(1f, fill = false),
                                color = ink.text,
                                fontSize = 16.sp,
                                fontWeight = FontWeight.SemiBold,
                                maxLines = 1,
                                overflow = TextOverflow.Ellipsis,
                            )
                            if (setting.closed) Box(Modifier.padding(start = 5.dp)) { Glyph(topicLockGlyph, ink.muted, 15.dp) }
                            if (setting.hidden) Box(Modifier.padding(start = 5.dp)) { Glyph(eyeOffGlyph, ink.muted, 15.dp) }
                        }
                        if (last != null) Text(chatWhen(last.at), Modifier.padding(start = 8.dp), color = ink.muted, fontSize = 12.5.sp)
                    }
                    Row(Modifier.padding(top = 3.dp), verticalAlignment = Alignment.CenterVertically) {
                        val line =
                            if (last == null) {
                                buildAnnotatedString {}
                            } else {
                                buildAnnotatedString {
                                    if (last.line == null) {
                                        val who = if (last.mine) stringResource(R.string.chat_you) else personName(last.account, people)
                                        if (who.isNotEmpty()) withStyle(SpanStyle(color = ink.text)) { append("$who: ") }
                                    }
                                    append(preview(last))
                                }
                            }
                        Text(
                            line,
                            Modifier.weight(1f),
                            color = ink.muted,
                            fontSize = 14.5.sp,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                        CountPill(count, mentioned, Modifier.padding(start = 8.dp))
                    }
                }
            }
        }
    }
}

/** Which part of a topic's page shows. */
internal enum class TopicSub { Page, Options, Mods, Edit, History }

/**
 * A topic's page, opened from its header, like the group's: its big photo, About and Rules for
 * everyone; Options, Moderators and History, Close and Hide for its moderators; Remove topic for those who
 * manage topics. Once the photo scrolls away, a bar with back and its name shows.
 */
@Composable
internal fun TopicPage(
    chat: String,
    topic: String,
    people: Map<String, AccountBrief>,
    onBack: () -> Unit,
    onRemoved: () -> Unit,
    onProfile: (String) -> Unit,
) {
    val host = LocalGroups.current ?: return
    val groups = host.state
    val me = host.me()
    val ink = ink()
    val kit = kit()
    val info = groups.info(chat)
    var sub by remember(chat, topic) { mutableStateOf(TopicSub.Page) }
    if (info == null) {
        Over(onBack) { PageBar("", ink, onBack) }
        return
    }
    val back = { sub = TopicSub.Page }
    when (sub) {
        TopicSub.Options -> return TopicOptionsPage(chat, topic, info, ink, back)
        TopicSub.Mods -> return TopicModsPage(chat, topic, info, people, ink, back, onProfile)
        TopicSub.Edit -> return TopicEditPage(chat, topic, ink, back)
        TopicSub.History -> return HistoryPage(chat, info.type, me, people, ink, back, wireTopic(topic))
        TopicSub.Page -> Unit
    }
    val general = topic.isEmpty()
    val name = topicName(groups, chat, topic)
    val setting = groups.setting(chat, topic)
    val profile = groups.topicProfile(chat, topic)
    val moderating = groups.moderating(chat, topic, me)
    val context = LocalContext.current
    val scroll = rememberScrollState()
    val density = androidx.compose.ui.platform.LocalDensity.current
    val shown = with(density) { scroll.value > (300.dp - 64.dp).toPx() }
    var removing by remember { mutableStateOf(false) }
    val rtl = LocalLayoutDirection.current == LayoutDirection.Rtl

    fun save(next: TopicOptionsBody) = host.engine.send(Command.TopicOptions(chat, wireTopic(topic), next))
    Over(onBack, kit.ground) {
        Box(Modifier.weight(1f)) {
            Column(Modifier.fillMaxSize().verticalScroll(scroll).padding(bottom = 12.dp)) {
                TopicCover(chat, topic, name, groups.name(chat).orEmpty(), onBack, if (moderating) ({ sub = TopicSub.Edit }) else null)
                Spacer(Modifier.height(6.dp))
                val bio = profile?.bio?.takeIf { it.isNotBlank() }
                val links = profile?.link_list.orEmpty()
                if (bio != null || links.isNotEmpty()) {
                    GroupCard(stringResource(R.string.group_about), ink) {
                        if (bio != null) {
                            Text(
                                bio,
                                Modifier.fillMaxWidth().background(cardFill()).padding(horizontal = 14.dp, vertical = 14.dp),
                                color = ink.text,
                                fontSize = 15.sp,
                            )
                        }
                        for (link in links) {
                            ValueLine(link.name, null, ink, icon = Glyphs.link) {
                                runCatching {
                                    context.startActivity(
                                        android.content.Intent(android.content.Intent.ACTION_VIEW, android.net.Uri.parse(link.url)),
                                    )
                                }
                            }
                        }
                    }
                }
                profile?.rules?.takeIf { it.isNotBlank() }?.let { RulesCard(it, ink) }
                if (moderating) {
                    GroupCard(null, ink) {
                        ValueLine(stringResource(R.string.group_options), null, ink, icon = GroupGlyphs.sliders, chevron = true) {
                            sub = TopicSub.Options
                        }
                        ValueLine(
                            stringResource(R.string.topic_moderators),
                            exact(setting.mod_list.size.toLong()),
                            ink,
                            icon = GroupGlyphs.shield,
                            chevron = true,
                        ) { sub = TopicSub.Mods }
                        ValueLine(stringResource(R.string.group_history), null, ink, icon = GroupGlyphs.clock, chevron = true) {
                            sub = TopicSub.History
                        }
                    }
                    GroupCard(null, ink) {
                        SwitchLine(
                            stringResource(R.string.topic_close),
                            stringResource(R.string.topic_close_help),
                            setting.closed,
                            ink,
                            icon = topicLockGlyph,
                        ) {
                            save(setting.options.copy(closed = it))
                        }
                        if (!general) {
                            SwitchLine(
                                stringResource(R.string.topic_hide),
                                stringResource(R.string.topic_hide_help),
                                setting.hidden,
                                ink,
                                icon = eyeOffGlyph,
                            ) {
                                save(setting.options.copy(hidden = it))
                            }
                        }
                    }
                }
                if (!general && groups.managing(chat)) {
                    GroupCard(null, ink) {
                        ValueLine(
                            stringResource(R.string.topic_remove),
                            null,
                            ink,
                            tone = kit.red,
                            icon = GroupGlyphs.trash,
                        ) { removing = true }
                    }
                }
            }
            if (shown) {
                Row(
                    Modifier.fillMaxWidth().height(64.dp).background(ink.page).padding(start = 4.dp, end = 8.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    IconButton(stringResource(R.string.back), onBack, 44.dp) { BackMark(ink.text, rtl) }
                    Text(
                        name,
                        Modifier.weight(1f).padding(horizontal = 6.dp),
                        color = ink.text,
                        fontSize = 19.sp,
                        fontWeight = FontWeight.SemiBold,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                    if (moderating) {
                        IconButton(
                            stringResource(R.string.topic_edit),
                            { sub = TopicSub.Edit },
                            44.dp,
                        ) { Glyph(GroupGlyphs.pencil, ink.text) }
                    }
                }
            }
        }
    }
    if (removing) {
        AskDialog(
            stringResource(R.string.group_remove_topic_ask, name),
            stringResource(R.string.group_remove_topic_body),
            { removing = false },
            listOf(
                DialogAct(stringResource(R.string.group_cancel)) { removing = false },
                DialogAct(stringResource(R.string.group_remove), red = true) {
                    removing = false
                    host.engine.send(Command.Topics(chat, info.topicsOn, groups.topics(chat).filter { it.id != topic }))
                    onRemoved()
                },
            ),
        )
    }
}

/** A topic's big photo with its name and "Topic in Apollo" over its foot, back and the pencil over its top. */
@Composable
private fun TopicCover(
    chat: String,
    topic: String,
    name: String,
    group: String,
    onBack: () -> Unit,
    onEdit: (() -> Unit)?,
) {
    val host = LocalGroups.current ?: return
    val photo = host.state.topicProfile(chat, topic)?.photo
    val rtl = LocalLayoutDirection.current == LayoutDirection.Rtl
    val general = topic.isEmpty()
    Box(
        Modifier
            .fillMaxWidth()
            .height(300.dp)
            .background(
                if (general) {
                    Brush.linearGradient(
                        listOf(accent, tile),
                        start = Offset(0f, 0f),
                        end = Offset(Float.POSITIVE_INFINITY, Float.POSITIVE_INFINITY),
                    )
                } else {
                    SolidColor(topicTone(topic))
                },
            ),
    ) {
        val image = photo?.let { topicPhoto(chat, wireTopic(topic), it, host.state, host.engine) }
        val blur = photo?.blurhash?.let { blurImage(it) }
        when {
            image != null -> Image(image, null, Modifier.fillMaxSize(), contentScale = ContentScale.Crop)
            blur != null -> Image(blur, null, Modifier.fillMaxSize(), contentScale = ContentScale.Crop)
            general -> Box(Modifier.align(Alignment.Center)) { Glyph(GroupGlyphs.hash, Color.White, 96.dp) }
            else ->
                Text(
                    name.firstOrNull()?.uppercaseChar()?.toString() ?: "",
                    Modifier.align(Alignment.Center),
                    color = Color.White,
                    fontSize = 96.sp,
                    fontWeight = FontWeight.SemiBold,
                )
        }
        Box(Modifier.fillMaxWidth().height(80.dp).background(Brush.verticalGradient(listOf(Color(0x66000000), Color.Transparent))))
        Box(
            Modifier
                .align(Alignment.BottomStart)
                .fillMaxWidth()
                .height(110.dp)
                .background(Brush.verticalGradient(listOf(Color.Transparent, Color(0x73000000)))),
        )
        Row(Modifier.fillMaxWidth().padding(6.dp), verticalAlignment = Alignment.CenterVertically) {
            IconButton(stringResource(R.string.back), onBack, 44.dp) { BackMark(Color.White, rtl) }
            Spacer(Modifier.weight(1f))
            if (onEdit != null) IconButton(stringResource(R.string.topic_edit), onEdit, 44.dp) { Glyph(GroupGlyphs.pencil, Color.White) }
        }
        Column(Modifier.align(Alignment.BottomStart).padding(start = 18.dp, end = 18.dp, bottom = 14.dp)) {
            Text(
                name,
                color = Color.White,
                fontSize = 24.sp,
                fontWeight = FontWeight.SemiBold,
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
            )
            Text(
                stringResource(R.string.topic_in, group),
                color = Color.White,
                fontSize = 14.sp,
                fontWeight = FontWeight.Medium,
                maxLines = 1,
            )
        }
    }
}

/** A topic's own options, starting from the group's, each saved as it changes. */
@Composable
private fun TopicOptionsPage(
    chat: String,
    topic: String,
    info: GroupInfo,
    ink: Ink,
    onBack: () -> Unit,
) {
    val host = LocalGroups.current ?: return
    val groups = host.state
    val setting = groups.setting(chat, topic)

    fun save(next: TopicOptionsBody) = host.engine.send(Command.TopicOptions(chat, wireTopic(topic), next))
    val pickers = remember { TopicPickers() }
    Over(onBack, kit().ground) {
        PageBar(stringResource(R.string.group_options), ink, onBack)
        Column(Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(top = 6.dp, bottom = 16.dp)) {
            Text(
                stringResource(R.string.topic_options_help, topicName(groups, chat, topic)),
                Modifier.padding(horizontal = 16.dp, vertical = 6.dp),
                color = ink.muted,
                fontSize = 13.5.sp,
            )
            TopicOptionCards(info.type, info.options, setting, ink, pickers, onChange = ::save)
        }
    }
    TopicOptionDialogs(info.options, setting, ink, pickers, ::save)
}

/**
 * A topic's option lines: who may write, reactions, stickers, files, slow mode, then what members see of
 * the times. [onChange] gets the whole options each time.
 */
@Composable
private fun TopicOptionCards(
    type: Int,
    options: ChatOptions?,
    setting: TopicSetting,
    ink: Ink,
    pickers: TopicPickers,
    times: Boolean = true,
    onChange: (TopicOptionsBody) -> Unit,
) {
    fun set(
        names: List<String>,
        on: Boolean,
    ) = onChange(topicWith(type, options, setting, names, on))

    fun may(name: String) = topicMay(type, options, setting, name)
    GroupCard(null, ink) {
        ValueLine(
            stringResource(R.string.topic_write),
            stringResource(if (setting.write == "moderators") R.string.topic_mods_only else R.string.topic_everyone),
            ink,
            icon = hornGlyph,
            chevron = true,
        ) { pickers.writing = true }
        SwitchLine(stringResource(R.string.group_reactions), null, may("message.send.reaction"), ink, icon = GroupGlyphs.smile) {
            set(listOf("message.send.reaction"), it)
        }
        SwitchLine(stringResource(R.string.group_stickers), null, may("message.send.sticker"), ink, icon = GroupGlyphs.sticker) {
            set(listOf("message.send.sticker"), it)
        }
        SwitchLine(stringResource(R.string.group_files), null, TOPIC_FILE_KINDS.all { may(it) }, ink, icon = GroupGlyphs.file) {
            set(TOPIC_FILE_KINDS, it)
        }
        ValueLine(
            stringResource(R.string.group_slow_mode),
            setting.slow_mode?.let { slowLabel(it) } ?: stringResource(R.string.topic_slow_same, slowLabel(options?.slow_mode)),
            ink,
            icon = GroupGlyphs.hour,
            chevron = true,
        ) { pickers.slow = true }
    }
    if (times) {
        GroupCard(stringResource(R.string.topic_members_see), ink) {
            for ((name, label) in listOf(
                "message.view_received.own" to R.string.topic_own_received,
                "message.view_seen.own" to R.string.topic_own_seen,
                "message.view_received.others" to R.string.topic_all_received,
                "message.view_seen.others" to R.string.topic_all_seen,
            )) {
                SwitchLine(stringResource(label), null, may(name), ink) { set(listOf(name), it) }
            }
        }
    }
}

/** Which of a topic's option pickers is open, kept by its page so they draw over the page, not inside its scroll. */
internal class TopicPickers {
    var writing by mutableStateOf(false)
    var slow by mutableStateOf(false)
}

/** The pickers [TopicOptionCards] opens: who may write, and slow mode. */
@Composable
private fun TopicOptionDialogs(
    options: ChatOptions?,
    setting: TopicSetting,
    ink: Ink,
    pickers: TopicPickers,
    onChange: (TopicOptionsBody) -> Unit,
) {
    if (pickers.writing) {
        AskDialog(
            stringResource(R.string.topic_write),
            null,
            { pickers.writing = false },
            listOf(DialogAct(stringResource(R.string.group_cancel)) { pickers.writing = false }),
        ) {
            for ((value, label, help) in listOf(
                Triple("everyone", R.string.topic_everyone, null),
                Triple("moderators", R.string.topic_mods_only, R.string.topic_mods_only_help),
            )) {
                Row(
                    Modifier
                        .fillMaxWidth()
                        .clip(RoundedCornerShape(10.dp))
                        .clickable(role = Role.RadioButton) {
                            pickers.writing = false
                            onChange(setting.options.copy(write = value))
                        }.padding(vertical = 10.dp, horizontal = 4.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(12.dp),
                ) {
                    Radio(setting.write == value)
                    Column {
                        Text(stringResource(label), color = ink.text, fontSize = 16.sp)
                        if (help != null) Text(stringResource(help), color = ink.muted, fontSize = 12.5.sp)
                    }
                }
            }
            Spacer(Modifier.height(8.dp))
        }
    }
    if (pickers.slow) {
        SlowPicker(
            setting.slow_mode,
            ink,
            { pickers.slow = false },
            stringResource(R.string.topic_slow_same, slowLabel(options?.slow_mode)),
        ) {
            pickers.slow = false
            onChange(setting.options.copy(slow_mode = it))
        }
    }
}

@Composable
private fun Radio(on: Boolean) {
    Box(
        Modifier.size(20.dp).clip(CircleShape).border(2.dp, if (on) accent else kit().off, CircleShape),
        contentAlignment = Alignment.Center,
    ) {
        if (on) Box(Modifier.size(10.dp).clip(CircleShape).background(accent))
    }
}

/**
 * A topic's moderators: its own, with Add a moderator for group owners and Remove as moderator on a tap,
 * then the group's owners and moderators, who moderate every topic.
 */
@Composable
private fun TopicModsPage(
    chat: String,
    topic: String,
    info: GroupInfo,
    people: Map<String, AccountBrief>,
    ink: Ink,
    onBack: () -> Unit,
    onProfile: (String) -> Unit,
) {
    val host = LocalGroups.current ?: return
    val groups = host.state
    val members = membersOf(chat, host)
    val setting = groups.setting(chat, topic)
    val owning = groups.owning(chat)
    val kit = kit()
    var adding by remember { mutableStateOf(false) }
    var acting by remember { mutableStateOf<String?>(null) }
    val name = topicName(groups, chat, topic)

    fun role(
        account: String,
        role: String,
    ) = host.engine.send(Command.TopicRole(chat, wireTopic(topic), account, role))
    Over(onBack, kit.ground) {
        PageBar(stringResource(R.string.topic_moderators), ink, onBack)
        Column(Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(top = 6.dp, bottom = 16.dp)) {
            GroupCard(stringResource(R.string.topic_mods_of, name), ink) {
                if (owning) {
                    ValueLine(
                        stringResource(R.string.topic_add_mod),
                        null,
                        ink,
                        tone = kit.accent,
                        icon = GroupGlyphs.userPlus,
                    ) { adding = true }
                }
                if (setting.mod_list.isEmpty() && !owning) {
                    Text(
                        stringResource(R.string.topic_no_mods),
                        Modifier.fillMaxWidth().background(cardFill()).padding(14.dp),
                        color = ink.muted,
                        fontSize = 14.5.sp,
                    )
                }
                for (account in setting.mod_list) {
                    PersonLine(
                        account,
                        stringResource(R.string.topic_mod),
                        people,
                        ink,
                    ) { if (owning) acting = account else onProfile(account) }
                }
            }
            val staff = members.orEmpty().filter { it.role == "owner" || it.role == "moderator" }
            if (staff.isNotEmpty()) {
                GroupCard(stringResource(R.string.topic_group_mods), ink) {
                    for (member in staff) PersonLine(
                        member.account,
                        roleText(info.type, member.role),
                        people,
                        ink,
                    ) { onProfile(member.account) }
                }
            }
            Text(
                stringResource(R.string.topic_group_mods_help),
                Modifier.padding(horizontal = 16.dp, vertical = 4.dp),
                color = ink.muted,
                fontSize = 12.5.sp,
            )
        }
    }
    acting?.let { account ->
        AskDialog(
            personName(account, people),
            null,
            { acting = null },
            listOf(
                DialogAct(stringResource(R.string.group_cancel)) { acting = null },
                DialogAct(stringResource(R.string.topic_remove_mod), red = true) {
                    acting = null
                    role(account, "member")
                },
            ),
        )
    }
    if (adding) {
        val choices = members.orEmpty().filter { it.role == "member" && it.account !in setting.mod_list }
        FootSheet({ adding = false }) {
            Text(
                stringResource(R.string.topic_add_mod),
                Modifier.fillMaxWidth().background(kit.card).padding(start = 18.dp, end = 18.dp, bottom = 8.dp),
                color = ink.text,
                fontSize = 17.sp,
                fontWeight = FontWeight.SemiBold,
            )
            for (member in choices) {
                PersonLine(member.account, people[member.account]?.username?.let { "@$it" }, people, ink) {
                    adding = false
                    role(member.account, "moderator")
                }
            }
        }
    }
}

/** A person's line in a card: their photo, name and a line under it. */
@Composable
private fun PersonLine(
    account: String,
    sub: String?,
    people: Map<String, AccountBrief>,
    ink: Ink,
    onClick: () -> Unit,
) {
    val name = personName(account, people)
    Row(
        Modifier
            .fillMaxWidth()
            .background(cardFill())
            .clickable(role = Role.Button, onClick = onClick)
            .padding(horizontal = 14.dp, vertical = 9.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Avatar(account, name, 40.dp)
        Column(Modifier.weight(1f).padding(start = 12.dp)) {
            Text(
                name,
                color = ink.text,
                fontSize = 15.5.sp,
                fontWeight = FontWeight.SemiBold,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            if (sub != null) Text(sub, color = ink.muted, fontSize = 13.sp, maxLines = 1)
        }
    }
}

/** A topic's photo, name, bio, rules and links, saved with the check; the photo goes up at once once cropped. */
@Composable
private fun TopicEditPage(
    chat: String,
    topic: String,
    ink: Ink,
    onBack: () -> Unit,
) {
    val host = LocalGroups.current ?: return
    val groups = host.state
    val profile = groups.topicProfile(chat, topic)
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val kit = kit()
    val general = topic.isEmpty()
    val start = topicName(groups, chat, topic)
    // A topic's name lives in the group's topic list, which only those who manage topics change.
    val naming = !general && groups.managing(chat)
    var name by remember { mutableStateOf(start) }
    var bio by remember { mutableStateOf(profile?.bio.orEmpty()) }
    var rules by remember { mutableStateOf(profile?.rules.orEmpty()) }
    var links by remember { mutableStateOf(profile?.link_list.orEmpty()) }
    var cropping by remember { mutableStateOf<Bitmap?>(null) }
    var sent by remember { mutableStateOf<Bitmap?>(null) }
    val picker =
        rememberLauncherForActivityResult(ActivityResultContracts.PickVisualMedia()) { uri ->
            if (uri != null) scope.launch { cropping = withContext(Dispatchers.IO) { forCrop(context, uri) } }
        }
    val source = cropping
    if (source != null) {
        BackHandler { cropping = null }
        Crop(source, { cropping = null }) { square ->
            cropping = null
            sent = square
            scope.launch {
                val (jpeg, thumbnail) = withContext(Dispatchers.Default) { encode(square) }
                if (jpeg != null) host.engine.send(Command.TopicPhoto(chat, wireTopic(topic), jpeg, thumbnail))
            }
        }
        return
    }
    Over(onBack) {
        val ready = !naming || name.trim().isNotEmpty()
        PageBar(stringResource(R.string.topic_edit), ink, onBack) {
            IconButton(stringResource(R.string.group_save), {
                if (ready) {
                    val info = groups.info(chat)
                    if (naming && info != null && name.trim() != start) {
                        host.engine.send(
                            Command.Topics(
                                chat,
                                info.topicsOn,
                                groups.topics(chat).map { if (it.id == topic) it.copy(name = name.trim()) else it },
                            ),
                        )
                    }
                    val said = rules.trim().ifEmpty { null }
                    host.engine.send(Command.TopicProfile(chat, wireTopic(topic), bio.trim().ifEmpty { null }, usable(links), said))
                    // Their writer has seen them, chosen by the author.
                    if (said != null) markRulesShown(context, host.me(), chat, topic)
                    onBack()
                }
            }, 44.dp) { Glyph(Glyphs.check, if (ready) kit.accent else ink.muted, 26.dp) }
        }
        Column(
            Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(start = 16.dp, end = 16.dp, top = 4.dp, bottom = 24.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Row(Modifier.fillMaxWidth().padding(top = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                // Only the photo is clipped to its circle, so the camera badge on its edge shows whole.
                Box(Modifier.size(64.dp)) {
                    Box(
                        Modifier.matchParentSize().clip(CircleShape).clickable(role = Role.Button) {
                            picker.launch(PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageOnly))
                        },
                    ) {
                        val shown = sent
                        if (shown != null) {
                            Image(remember(shown) { shown.asImageBitmap() }, null, Modifier.fillMaxSize(), contentScale = ContentScale.Crop)
                        } else {
                            TopicIcon(chat, topic, name, 64.dp)
                        }
                    }
                    Box(
                        Modifier.align(Alignment.BottomEnd).size(22.dp).clip(CircleShape).background(accent),
                        contentAlignment = Alignment.Center,
                    ) { Glyph(Glyphs.cam, onAccent, 13.dp) }
                }
                if (naming) {
                    Field(
                        name,
                        { name = it },
                        stringResource(R.string.group_topic_name),
                        ink,
                        Modifier.padding(start = 12.dp).weight(1f),
                        max = TOPIC_NAME_MAX,
                        count = true,
                    )
                } else {
                    Text(
                        name,
                        Modifier.padding(start = 12.dp).weight(1f),
                        color = ink.text,
                        fontSize = 18.sp,
                        fontWeight = FontWeight.SemiBold,
                    )
                }
            }
            Field(bio, { bio = it }, stringResource(R.string.group_bio), ink, single = false, max = GROUP_BIO_MAX, count = true)
            Field(
                rules,
                { rules = it },
                stringResource(R.string.topic_rules_hint),
                ink,
                single = false,
                max = RULES_MAX,
                count = true,
                height = 118.dp,
            )
            Links(links, ink, kit.ground) { links = it }
        }
    }
}

/** Rules are at most 2,000 characters, as the author chose, checked by core too. */
internal const val RULES_MAX = 2000

/**
 * A new topic, as a page like New group's first step, chosen by the author: a tinted box saying which
 * group it goes in, the photo and name, Bio, Rules, links, Hide until ready, and More options folding open
 * its own options. Create is faint until there is a name, and opens the new topic.
 */
@Composable
internal fun NewTopicPage(
    chat: String,
    onBack: () -> Unit,
    onMade: (String) -> Unit,
) {
    val host = LocalGroups.current ?: return
    val groups = host.state
    val info = groups.info(chat) ?: return
    val ink = ink()
    val kit = kit()
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val group = groups.name(chat).orEmpty()
    var name by remember { mutableStateOf("") }
    var bio by remember { mutableStateOf("") }
    var rules by remember { mutableStateOf("") }
    var links by remember { mutableStateOf(emptyList<ProfileLink>()) }
    var options by remember { mutableStateOf(TopicSetting("")) }
    val pickers = remember { TopicPickers() }

    fun change(next: TopicOptionsBody) {
        options = TopicSetting("", next.allow, next.deny, next.slow_mode, next.write, next.closed, next.hidden)
    }
    var more by remember { mutableStateOf(false) }
    var photo by remember { mutableStateOf<Bitmap?>(null) }
    var cropping by remember { mutableStateOf<Bitmap?>(null) }
    var making by remember { mutableStateOf(false) }
    val picker =
        rememberLauncherForActivityResult(ActivityResultContracts.PickVisualMedia()) { uri ->
            if (uri != null) scope.launch { cropping = withContext(Dispatchers.IO) { forCrop(context, uri) } }
        }
    val source = cropping
    if (source != null) {
        BackHandler { cropping = null }
        Crop(source, { cropping = null }) { square ->
            cropping = null
            photo = square
        }
        return
    }
    val ready = name.trim().isNotEmpty() && !making
    Over(onBack) {
        PageBar(stringResource(R.string.group_new_topic), ink, onBack) {
            BarWord(stringResource(R.string.group_create), ready) {
                making = true
                groups.askTopicId(chat, host.engine) { id ->
                    val trimmed = name.trim()
                    groups.topicNames[chat] = groups.topicNames[chat].orEmpty() + (id to trimmed)
                    host.engine.send(
                        Command.Topics(
                            chat,
                            info.topicsOn,
                            groups.topics(chat) + TopicName(id, trimmed, groups.topics(chat).size.toLong()),
                        ),
                    )
                    val body = bio.trim().ifEmpty { null }
                    val said = rules.trim().ifEmpty { null }
                    val kept = usable(links)
                    if (body != null || said != null || kept.isNotEmpty()) {
                        host.engine.send(
                            Command.TopicProfile(chat, id, body, kept, said),
                        )
                    }
                    // Their writer has seen them, chosen by the author.
                    if (said != null) markRulesShown(context, host.me(), chat, id)
                    if (options.options != TopicOptionsBody()) host.engine.send(Command.TopicOptions(chat, id, options.options))
                    photo?.let { square ->
                        scope.launch {
                            val (jpeg, thumbnail) = withContext(Dispatchers.Default) { encode(square) }
                            if (jpeg != null) host.engine.send(Command.TopicPhoto(chat, id, jpeg, thumbnail))
                        }
                    }
                    onMade(id)
                }
            }
        }
        Column(
            Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(start = 16.dp, end = 16.dp, top = 4.dp, bottom = 24.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Row(
                Modifier.fillMaxWidth().clip(RoundedCornerShape(14.dp)).background(kit.tint).padding(horizontal = 12.dp, vertical = 10.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                GroupAvatar(chat, groups, host.engine, group, 36.dp)
                Column {
                    Text(stringResource(R.string.topic_new_in, group), color = ink.text, fontSize = 14.5.sp)
                    Text(stringResource(R.string.topic_new_in_help, group), color = ink.muted, fontSize = 12.5.sp)
                }
            }
            Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                Box(
                    Modifier.size(64.dp).clip(CircleShape).background(kit.tint).clickable(role = Role.Button) {
                        picker.launch(PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageOnly))
                    },
                    contentAlignment = Alignment.Center,
                ) {
                    val shown = photo
                    if (shown != null) {
                        Image(remember(shown) { shown.asImageBitmap() }, null, Modifier.fillMaxSize(), contentScale = ContentScale.Crop)
                    } else {
                        Glyph(Glyphs.cam, kit.accent, 24.dp)
                    }
                }
                Field(
                    name,
                    { name = it },
                    stringResource(R.string.group_topic_name),
                    ink,
                    Modifier.padding(start = 12.dp).weight(1f),
                    max = TOPIC_NAME_MAX,
                    count = true,
                )
            }
            Field(bio, { bio = it }, stringResource(R.string.group_bio), ink, single = false, max = GROUP_BIO_MAX)
            Field(rules, { rules = it }, stringResource(R.string.topic_rules_hint), ink, single = false, max = RULES_MAX, height = 84.dp)
            Links(links, ink, kit.ground) { links = it }
            GroupCard(null, ink, modifier = Modifier) {
                SwitchLine(
                    stringResource(R.string.topic_hide_ready),
                    stringResource(R.string.topic_hide_ready_help),
                    options.hidden,
                    ink,
                    icon = eyeOffGlyph,
                ) {
                    options = options.copy(hidden = it)
                }
                ValueLine(
                    stringResource(R.string.group_more),
                    null,
                    ink,
                    help = if (more) null else stringResource(R.string.topic_more_help),
                    trailing = { Glyph(if (more) GroupGlyphs.up else GroupGlyphs.down, ink.muted, 18.dp) },
                ) { more = !more }
            }
            if (more) {
                Text(
                    stringResource(R.string.topic_options_help, name.trim().ifEmpty { stringResource(R.string.group_new_topic) }),
                    Modifier.padding(horizontal = 4.dp),
                    color = ink.muted,
                    fontSize = 12.5.sp,
                )
                TopicOptionCards(info.type, info.options, options, ink, pickers, onChange = ::change)
            }
        }
    }
    TopicOptionDialogs(info.options, options, ink, pickers, ::change)
}
