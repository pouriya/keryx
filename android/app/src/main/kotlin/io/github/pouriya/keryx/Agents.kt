package io.github.pouriya.keryx

import android.content.ClipData
import android.content.ClipDescription
import android.content.ClipboardManager
import android.content.Context
import android.graphics.Bitmap
import android.os.Build
import android.os.PersistableBundle
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.PickVisualMediaRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.asPaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalLayoutDirection
import androidx.compose.ui.platform.LocalUriHandler
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextDirection
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.LayoutDirection
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

private const val CONFIGURATION = "https://github.com/pouriya/keryx/blob/main/agent/README.md#configuration"
private val USERNAME_STEM = Regex("[a-z_][a-z0-9_]{0,25}")

/** Agent commands use Core's tags so another request's reply cannot close an editor or consume its failure. */
internal class AgentAccounts(private val engine: Engine) {
    var listing by mutableStateOf<Event.Agents?>(null)
        private set
    var credentials by mutableStateOf<AgentCredentials?>(null)
        private set
    var failure by mutableStateOf<Event.Answered?>(null)
        private set
    var busy by mutableStateOf(false)
        private set
    var loading by mutableStateOf(false)
        private set
    var loadFailure by mutableStateOf<Event.Answered?>(null)
        private set

    private var next = 0L
    private var loadTag: Long? = null
    private var writeTag: Long? = null
    private var onSaved: (() -> Unit)? = null
    private var creating = false
    private var renewing: String? = null

    fun load() {
        if (loading) return
        val tag = ++next
        loadTag = tag
        loading = true
        loadFailure = null
        engine.send(tag, Command.Agents)
    }

    fun write(
        command: Command,
        saved: () -> Unit = {},
    ) {
        if (busy) return
        val tag = ++next
        writeTag = tag
        failure = null
        busy = true
        onSaved = saved
        creating = command is Command.CreateAgent
        renewing = (command as? Command.RenewAgentToken)?.agent
        engine.send(tag, command)
    }

    fun clearFailure() {
        failure = null
    }

    fun closeCredentials() {
        credentials = null
    }

    fun clear() {
        listing = null
        credentials = null
        failure = null
        loadFailure = null
        loadTag = null
        writeTag = null
        onSaved = null
        creating = false
        renewing = null
        loading = false
        busy = false
    }

    fun apply(event: Event) {
        when (event) {
            is Event.Agents -> listing = event
            is Event.AgentCreated -> {
                if (creating) credentials = AgentCredentials(event.token, event.words)
            }
            is Event.AgentToken -> {
                if (renewing == event.agent) credentials = AgentCredentials(event.token)
            }
            is Event.Answered ->
                when (event.tag) {
                    loadTag -> {
                        loadTag = null
                        loading = false
                        loadFailure = event.takeIf { it.code != null }
                    }
                    writeTag -> {
                        val saved = onSaved
                        writeTag = null
                        onSaved = null
                        creating = false
                        renewing = null
                        busy = false
                        if (event.code == null) {
                            saved?.invoke()
                        } else {
                            failure = event
                            if (event.code in setOf("not_held", "no_permission", "too_many_agents", "agents_off", "bad_signature")) load()
                        }
                    }
                    else -> Unit
                }
            is Event.LoggedOut -> clear()
            else -> Unit
        }
    }
}

internal class AgentCredentials(val token: String, val words: String? = null)

private enum class AgentPage { List, Create, Manage, Permissions, Edit, Name, Bio, Links }

private enum class AgentTokenAction(val label: Int) {
    Renew(R.string.agent_renew),
    Revoke(R.string.agent_revoke),
    Disable(R.string.agent_disable),
}

private class AgentDraft {
    var name by mutableStateOf("")
    var username by mutableStateOf("")
    var grants by mutableStateOf<Set<String>>(emptySet())
    var admin by mutableStateOf(false)
}

@Composable
internal fun AgentsScreen(
    state: AgentAccounts,
    profiles: Profiles,
    server: String,
    link: Link,
    onClose: () -> Unit,
) {
    val ink = ink()
    val online = link == Link.Updating || link == Link.Connected
    var page by remember { mutableStateOf(AgentPage.List) }
    var selected by remember { mutableStateOf<String?>(null) }
    var creation by remember { mutableStateOf(AgentDraft()) }
    var fromCreate by remember { mutableStateOf(false) }
    var action by remember { mutableStateOf<AgentTokenAction?>(null) }
    val listing = state.listing
    val capabilities = listing?.capabilities
    val agent = listing?.agent_list?.firstOrNull { it.id == selected }
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var crop by remember(selected) { mutableStateOf<Bitmap?>(null) }
    val picker =
        rememberLauncherForActivityResult(ActivityResultContracts.PickVisualMedia()) { uri ->
            if (uri != null) scope.launch { crop = withContext(Dispatchers.IO) { forCrop(context, uri) } }
        }
    val choosePhoto = { picker.launch(PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageOnly)) }

    LaunchedEffect(online) { if (online) state.load() }
    val credentials = state.credentials
    if (credentials != null) {
        AgentSetup(credentials, server, ink) {
            state.closeCredentials()
            action = null
            page = if (credentials.words != null) AgentPage.List else AgentPage.Manage
            if (online) state.load()
        }
        return
    }
    val source = crop
    if (source != null && agent != null) {
        BackHandler { crop = null }
        Crop(source, { crop = null }) { square ->
            crop = null
            scope.launch {
                val (jpeg, thumbnail) = withContext(Dispatchers.Default) { encode(square) }
                if (jpeg != null) state.write(Command.AgentPhoto(agent.id, jpeg, thumbnail))
            }
        }
        return
    }

    fun open(next: AgentPage) {
        state.clearFailure()
        page = next
    }

    fun back() {
        if (state.busy) return
        if (action != null) {
            action = null
            state.clearFailure()
            return
        }
        when (page) {
            AgentPage.List -> onClose()
            AgentPage.Create, AgentPage.Manage -> open(AgentPage.List)
            AgentPage.Permissions -> open(if (fromCreate) AgentPage.Create else AgentPage.Manage)
            AgentPage.Edit -> open(AgentPage.Manage)
            AgentPage.Name, AgentPage.Bio, AgentPage.Links -> open(AgentPage.Edit)
        }
    }

    Box(
        Modifier.fillMaxSize().background(ink.page)
            .pointerInput(Unit) { awaitPointerEventScope { while (true) awaitPointerEvent() } },
    ) {
        when (page) {
            AgentPage.List ->
                AgentListPage(state, profiles, online, ::back, {
                    creation = AgentDraft()
                    open(AgentPage.Create)
                }) {
                    selected = it.id
                    open(AgentPage.Manage)
                }
            AgentPage.Create ->
                AgentCreatePage(state, creation, capabilities, online, ::back, {
                    fromCreate = true
                    open(AgentPage.Permissions)
                })
            AgentPage.Manage ->
                if (agent != null) {
                    key(agent.id) {
                        AgentManagePage(
                            state, profiles, agent, online, ::back,
                            choosePhoto,
                            onEdit = { open(AgentPage.Edit) },
                            onPermissions = {
                                fromCreate = false
                                open(AgentPage.Permissions)
                            },
                            onToken = {
                                state.clearFailure()
                                action = it
                            },
                        )
                    }
                } else {
                    Over(::back) { PageBar(stringResource(R.string.agents_title), ink, ::back) }
                }
            AgentPage.Permissions ->
                if (capabilities != null && (fromCreate || agent != null)) {
                    AgentPermissionsPage(state, capabilities, if (fromCreate) creation else null, agent, online, ::back) {
                        open(if (fromCreate) AgentPage.Create else AgentPage.Manage)
                    }
                }
            AgentPage.Edit, AgentPage.Name, AgentPage.Bio, AgentPage.Links ->
                if (agent != null) {
                    AgentEditPage(state, agent, profiles, page, online, ::back, ::open, choosePhoto)
                }
        }
        val ending = action
        if (ending != null && agent != null) {
            AgentEndDialog(state, ending, agent, online, ::back) { action = null }
        }
    }
}

@Composable
private fun AgentListPage(
    state: AgentAccounts,
    profiles: Profiles,
    online: Boolean,
    onBack: () -> Unit,
    onCreate: () -> Unit,
    onAgent: (AgentItem) -> Unit,
) {
    val ink = ink()
    val listing = state.listing
    val capabilities = listing?.capabilities
    val agents = listing?.agent_list.orEmpty()
    val atLimit = capabilities != null && agents.count { !it.disabled }.toLong() >= capabilities.limit
    val blocked =
        when {
            !online -> stringResource(R.string.agent_waiting_network)
            state.loadFailure != null -> agentError(state.loadFailure)
            capabilities == null -> null
            !capabilities.enabled -> stringResource(R.string.agent_server_off)
            !capabilities.can_create -> stringResource(R.string.agent_create_forbidden)
            atLimit -> stringResource(R.string.agent_limit)
            else -> null
        }
    val canCreate = capabilities != null && blocked == null && !state.loading
    Over(onBack) {
        PageBar(stringResource(R.string.agents_title), ink, onBack) {
            if (canCreate && agents.isNotEmpty()) {
                IconButton(stringResource(R.string.agent_new), onCreate) { Glyph(Glyphs.plus, kit().accent) }
            }
        }
        if (blocked != null) {
            Row(
                Modifier.fillMaxWidth().background(kit().tint).padding(horizontal = 16.dp, vertical = 10.dp),
                horizontalArrangement = Arrangement.spacedBy(10.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Glyph(Glyphs.info, kit().accent, 18.dp)
                Text(blocked, Modifier.weight(1f), color = ink.text, fontSize = 14.sp)
                if (online && state.loadFailure != null) {
                    BarWord(stringResource(R.string.agent_retry), !state.loading, state::load)
                }
            }
        }
        if (agents.isEmpty()) {
            Box(Modifier.weight(1f).fillMaxWidth().padding(16.dp), contentAlignment = Alignment.Center) {
                if (online && listing == null && state.loadFailure == null) {
                    Spinner(kit().accent)
                } else if (listing != null && capabilities?.enabled == true) {
                    Column(horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(8.dp)) {
                        Glyph(Glyphs.cpu, ink.muted, 42.dp)
                        Text(stringResource(R.string.agent_empty), fontSize = 17.sp, fontWeight = FontWeight.SemiBold, color = ink.text)
                        Text(stringResource(R.string.agent_empty_hint), fontSize = 14.sp, color = ink.muted)
                    }
                }
            }
        } else {
            Column(Modifier.weight(1f).verticalScroll(rememberScrollState())) {
                for (agent in agents) {
                    key(agent.id) {
                        LaunchedEffect(agent.id, agent.device, online) {
                            if (online && !agent.disabled && agent.device != null) {
                                profiles.open(agent.id, refreshPresence = true)
                            }
                        }
                        AgentListRow(agent, profiles, online, onAgent)
                    }
                }
            }
        }
        if (canCreate && agents.isEmpty()) {
            WideButton(stringResource(R.string.agent_new), ink, Modifier.padding(horizontal = 16.dp, vertical = 8.dp), onClick = onCreate)
        }
    }
}

@Composable
private fun AgentListRow(
    agent: AgentItem,
    profiles: Profiles,
    online: Boolean,
    onAgent: (AgentItem) -> Unit,
) {
    val ink = ink()
    val name = agent.display_name ?: agent.username
    val presence = profiles.presence[agent.id].takeIf { online && !agent.disabled }
    Row(
        Modifier.fillMaxWidth().clickable(role = Role.Button) { onAgent(agent) }.padding(horizontal = 16.dp, vertical = 10.dp),
        horizontalArrangement = Arrangement.spacedBy(12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box {
            Avatar(agent.id, name, 54.dp, username = agent.username)
            PresenceDot(presence?.state, 10.dp, 2.dp, ink.page, Modifier.align(Alignment.BottomEnd))
        }
        Column(Modifier.weight(1f)) {
            Text(name, color = ink.text, fontSize = 16.sp, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text(
                agentStatus(agent, presence),
                color = if (presence?.state == "trouble") troubleTone(isDark()) else ink.muted,
                fontSize = 14.sp,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
        Glyph(chevronGlyph(), ink.muted, 18.dp)
    }
}

@Composable
private fun agentStatus(
    agent: AgentItem,
    presence: PresenceItem?,
): String =
    when {
        agent.disabled -> stringResource(R.string.agent_disabled)
        agent.token_at == null -> stringResource(R.string.agent_revoked)
        agent.device == null -> stringResource(R.string.agent_waiting_device)
        else -> presenceLine(presence) ?: stringResource(R.string.agent_disconnected)
    }

@Composable
private fun AgentCreatePage(
    state: AgentAccounts,
    draft: AgentDraft,
    capabilities: AgentCapabilities?,
    online: Boolean,
    onBack: () -> Unit,
    onPermissions: () -> Unit,
) {
    val ink = ink()
    val available = capabilities?.grantable_permission_list.orEmpty().toSet()
    val canAdmin = capabilities?.can_grant_admin == true
    val stale = draft.grants.any { it !in available } || (draft.admin && !canAdmin)
    val atLimit = capabilities != null && state.listing?.agent_list.orEmpty().count { !it.disabled }.toLong() >= capabilities.limit
    val valid = draft.name.isNotBlank() && USERNAME_STEM.matches(draft.username)
    val canSubmit = valid && online && capabilities?.enabled == true && capabilities.can_create && !atLimit && !stale && !state.busy
    Over(onBack) {
        PageBar(stringResource(R.string.agent_new), ink, onBack) {
            BarWord(stringResource(R.string.group_create), canSubmit) {
                state.write(Command.CreateAgent(draft.username + AGENT_SUFFIX, draft.name.trim(), draft.grants.toList(), draft.admin))
            }
        }
        Column(
            Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(horizontal = 16.dp, vertical = 12.dp),
            verticalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            AgentHint(stringResource(R.string.profile_display_name), ink.text)
            Field(draft.name, {
                draft.name = it
                state.clearFailure()
            }, "", ink, max = 64, enabled = !state.busy)
            AgentHint(stringResource(R.string.username), ink.text)
            AgentUsername(draft.username, !state.busy) {
                draft.username = it
                state.clearFailure()
            }
            AgentHint(stringResource(R.string.agent_suffix_hint))
            if (state.busy) AgentHint(stringResource(R.string.agent_creating))
            if (stale) AgentHint(stringResource(R.string.agent_permissions_changed), kit().red)
            if (!online) AgentHint(stringResource(R.string.agent_waiting_network), kit().red)
            agentError(state.failure)?.let { AgentHint(it, kit().red) }
            GroupCard(null, ink, modifier = Modifier.padding(top = 6.dp)) {
                ValueLine(
                    stringResource(R.string.agent_permissions),
                    null,
                    ink,
                    icon = GroupGlyphs.shield,
                    chevron = true,
                    help =
                        if (draft.admin) {
                            stringResource(
                                R.string.agent_admin,
                            )
                        } else {
                            stringResource(R.string.agent_selected, exact(draft.grants.size.toLong()))
                        },
                    onClick = onPermissions.takeIf { !state.busy && capabilities != null },
                )
            }
            AgentHint(stringResource(R.string.agent_chat_roles))
        }
    }
}

@Composable
private fun AgentUsername(
    value: String,
    enabled: Boolean,
    onChange: (String) -> Unit,
) {
    val ink = ink()
    CompositionLocalProvider(LocalLayoutDirection provides LayoutDirection.Ltr) {
        Row(
            Modifier.fillMaxWidth().heightIn(min = 52.dp).border(1.5.dp, ink.line, RoundedCornerShape(12.dp))
                .padding(horizontal = 14.dp),
            horizontalArrangement = Arrangement.spacedBy(8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            BasicTextField(
                value,
                { onChange(it.take(26)) },
                Modifier.weight(1f),
                singleLine = true,
                enabled = enabled,
                keyboardOptions = KeyboardOptions(autoCorrectEnabled = false, keyboardType = KeyboardType.Ascii),
                textStyle = TextStyle(color = ink.text, fontSize = 16.sp, textDirection = TextDirection.Ltr),
                cursorBrush = SolidColor(kit().accent),
            )
            Text(AGENT_SUFFIX, color = ink.muted, fontSize = 15.sp, fontFamily = FontFamily.Monospace)
        }
    }
}

@Composable
private fun AgentPermissionsPage(
    state: AgentAccounts,
    capabilities: AgentCapabilities,
    draft: AgentDraft?,
    agent: AgentItem?,
    online: Boolean,
    onBack: () -> Unit,
    onSave: () -> Unit,
) {
    val ink = ink()
    var selected by remember { mutableStateOf(draft?.grants ?: agent?.permission_list.orEmpty().toSet()) }
    var admin by remember { mutableStateOf(draft?.admin ?: (agent?.admin == true)) }
    val available = capabilities.grantable_permission_list.toSet()
    val unavailable = (selected + agent?.permission_list.orEmpty()) - available
    val unavailableAdmin = (admin || agent?.admin == true) && !capabilities.can_grant_admin
    val maySave = !state.busy && !state.loading && (draft != null || (online && agent?.disabled == false))
    Over(onBack, kit().ground) {
        PageBar(stringResource(R.string.agent_permissions), ink, onBack) {
            IconButton(stringResource(R.string.agent_permissions_save), {
                if (maySave) {
                    val kept = selected.intersect(available)
                    val keepAdmin = admin && capabilities.can_grant_admin
                    if (draft != null) {
                        draft.grants = kept
                        draft.admin = keepAdmin
                        onSave()
                    } else if (agent != null) {
                        state.write(Command.UpdateAgent(agent.id, permission_list = kept.toList(), admin = keepAdmin), onSave)
                    }
                }
            }) {
                Glyph(Glyphs.check, if (maySave) kit().accent else ink.muted)
            }
        }
        Column(Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(vertical = 6.dp)) {
            Box(Modifier.padding(horizontal = 16.dp, vertical = 6.dp)) {
                AgentHint(stringResource(R.string.agent_permissions_hint))
            }
            if (unavailable.isNotEmpty() || unavailableAdmin) {
                Box(Modifier.padding(horizontal = 16.dp, vertical = 6.dp)) {
                    AgentHint(stringResource(R.string.agent_unavailable_hint), kit().red)
                }
            }
            agentError(state.failure)?.let {
                Box(Modifier.padding(horizontal = 16.dp, vertical = 6.dp)) { AgentHint(it, kit().red) }
            }
            GroupCard(null, ink) {
                val shown = available + unavailable
                for (name in (permissions.keys + shown).filter { it in shown }) {
                    val permission = permissions[name]
                    SwitchLine(
                        if (permission != null) stringResource(permission.label) else name,
                        if (name in unavailable) stringResource(R.string.agent_unavailable) else null,
                        name in selected || name in unavailable,
                        ink,
                        enabled = !state.busy && name in available,
                        icon = permission?.glyph ?: GroupGlyphs.shield,
                    ) { on ->
                        selected = if (on) selected + name else selected - name
                    }
                }
            }
            if (capabilities.can_grant_admin || unavailableAdmin) {
                GroupCard(null, ink) {
                    SwitchLine(
                        stringResource(R.string.agent_admin),
                        stringResource(if (unavailableAdmin) R.string.agent_unavailable else R.string.agent_admin_hint),
                        admin || unavailableAdmin,
                        ink,
                        enabled = !state.busy && capabilities.can_grant_admin,
                        icon = GroupGlyphs.shield,
                    ) { admin = it }
                }
            }
        }
    }
}

private class Permission(val label: Int, val glyph: List<String>)

private val permissions =
    linkedMapOf(
        "direct.create" to Permission(R.string.agent_p_direct, Glyphs.chat),
        "group.create" to Permission(R.string.agent_p_group, Glyphs.person),
        "channel.create" to Permission(R.string.agent_p_channel, Glyphs.plus),
        "directory.search" to Permission(R.string.agent_p_directory, GroupGlyphs.search),
        "profile.edit" to Permission(R.string.agent_p_profile, Glyphs.person),
        "account.rename" to Permission(R.string.agent_p_rename, Glyphs.at),
        "folder.create" to Permission(R.string.agent_p_folder_create, Glyphs.folderPlus),
        "folder.rename" to Permission(R.string.agent_p_folder_rename, Glyphs.edit),
        "folder.remove" to Permission(R.string.agent_p_folder_remove, Glyphs.folderOut),
        "folder.reorder" to Permission(R.string.agent_p_folder_reorder, Glyphs.handle),
        "folder.default" to Permission(R.string.agent_p_folder_default, Glyphs.check),
        "folder.chats" to Permission(R.string.agent_p_folder_chats, Glyphs.folder),
        "chat.pin" to Permission(R.string.agent_p_pin, Glyphs.pin),
        "presence.in_app" to Permission(R.string.agent_p_in_app, Glyphs.clock),
        "presence.connected" to Permission(R.string.agent_p_connected, Glyphs.devices),
        "account.create" to Permission(R.string.agent_p_account, Glyphs.person),
        "sticker.manage" to Permission(R.string.agent_p_stickers, GroupGlyphs.smile),
    )

@Composable
private fun AgentManagePage(
    state: AgentAccounts,
    profiles: Profiles,
    agent: AgentItem,
    online: Boolean,
    onBack: () -> Unit,
    choosePhoto: () -> Unit,
    onEdit: () -> Unit,
    onPermissions: () -> Unit,
    onToken: (AgentTokenAction) -> Unit,
) {
    val ink = ink()
    val canChange = online && !agent.disabled && !state.busy
    val canRenew = canChange && state.listing?.capabilities?.enabled == true
    BackHandler(onBack = onBack)
    LaunchedEffect(agent.id, online) { if (online) profiles.open(agent.id, refreshPresence = true) }
    val top = WindowInsets.statusBars.asPaddingValues().calculateTopPadding()
    val scroll = rememberScrollState()
    val name = agent.display_name ?: agent.username
    Box(Modifier.fillMaxSize().background(kit().ground)) {
        Column(Modifier.fillMaxSize().verticalScroll(scroll)) {
            Hero(profiles, agent.id, true, name, agent.username, top, choosePhoto, {}, agent = true, editable = canChange)
            if (!online) {
                Box(Modifier.padding(horizontal = 16.dp, vertical = 8.dp)) {
                    AgentHint(stringResource(R.string.agent_waiting_network))
                }
            }
            agentError(state.failure)?.let {
                Box(Modifier.padding(horizontal = 16.dp, vertical = 8.dp)) { AgentHint(it, kit().red) }
            }
            GroupCard(stringResource(R.string.agent_connection), ink) {
                val presence = profiles.presence[agent.id].takeIf { online }
                ValueLine(agentStatus(agent, presence), null, ink, icon = Glyphs.bolt, onClick = null)
                val connected = agent.last_connected_at
                ValueLine(
                    if (connected == null) stringResource(R.string.agent_no_device) else presenceWhen(connected),
                    null,
                    ink,
                    icon = Glyphs.clock,
                    help = if (connected == null) null else stringResource(R.string.presence_connected),
                    onClick = null,
                )
            }
            if (!agent.disabled) {
                GroupCard(stringResource(R.string.profile_account), ink) {
                    ValueLine(
                        stringResource(R.string.agent_edit),
                        null,
                        ink,
                        icon = Glyphs.edit,
                        chevron = true,
                        help = stringResource(R.string.agent_edit_hint),
                        onClick = onEdit.takeIf { canChange },
                    )
                    ValueLine(
                        stringResource(R.string.agent_permissions),
                        null,
                        ink,
                        icon = GroupGlyphs.shield,
                        chevron = true,
                        help =
                            if (agent.admin) {
                                stringResource(
                                    R.string.agent_admin,
                                )
                            } else {
                                stringResource(R.string.agent_granted, exact(agent.permission_list.size.toLong()))
                            },
                        onClick = onPermissions.takeIf { canChange },
                    )
                }
                val issued = agent.token_at
                GroupCard(stringResource(R.string.agent_token), ink) {
                    ValueLine(
                        stringResource(R.string.agent_renew),
                        null,
                        ink,
                        icon = AgentGlyphs.key,
                        help =
                            if (issued == null) {
                                stringResource(
                                    R.string.agent_issue,
                                )
                            } else {
                                stringResource(R.string.agent_issued, presenceWhen(issued))
                            },
                        onClick = if (canRenew) ({ onToken(AgentTokenAction.Renew) }) else null,
                    )
                    if (issued != null) {
                        ValueLine(
                            stringResource(R.string.agent_revoke),
                            null,
                            ink,
                            tone = kit().red,
                            icon = Glyphs.out,
                            onClick = if (canChange) ({ onToken(AgentTokenAction.Revoke) }) else null,
                        )
                    }
                }
                GroupCard(null, ink) {
                    ValueLine(
                        stringResource(R.string.agent_disable),
                        null,
                        ink,
                        tone = kit().red,
                        icon = GroupGlyphs.userCross,
                        onClick = if (canChange) ({ onToken(AgentTokenAction.Disable) }) else null,
                    )
                }
            }
            Spacer(Modifier.windowInsetsPadding(WindowInsets.navigationBars).padding(bottom = 6.dp))
        }
        ProfileBar(name, ink, scroll, onBack, choosePhoto.takeIf { canChange })
    }
}

@Composable
private fun AgentEndDialog(
    state: AgentAccounts,
    action: AgentTokenAction,
    agent: AgentItem,
    online: Boolean,
    onBack: () -> Unit,
    onSaved: () -> Unit,
) {
    val ink = ink()
    var words by remember(action, agent.id) { mutableStateOf("") }
    val required = agent.device != null
    val phrase = words.trim().split(Regex("\\s+")).filter(String::isNotEmpty)
    val label = stringResource(action.label)
    val body =
        when (action) {
            AgentTokenAction.Renew -> if (required) R.string.agent_renew_device_hint else R.string.agent_renew_hint
            AgentTokenAction.Revoke -> R.string.agent_revoke_hint
            AgentTokenAction.Disable -> R.string.agent_disable_hint
        }
    AskDialog(
        label,
        stringResource(body),
        onBack,
        listOf(
            DialogAct(stringResource(R.string.chat_cancel), enabled = !state.busy, onClick = onBack),
            DialogAct(label, red = action != AgentTokenAction.Renew, enabled = online && !state.busy && (!required || phrase.size == 12)) {
                val proof = if (required) phrase.joinToString(" ") else ""
                val command =
                    when (action) {
                        AgentTokenAction.Renew -> Command.RenewAgentToken(agent.id, proof)
                        AgentTokenAction.Revoke -> Command.RevokeAgentToken(agent.id, proof)
                        AgentTokenAction.Disable -> Command.DisableAgent(agent.id, proof)
                    }
                state.write(command, onSaved)
            },
        ),
    ) {
        if (required) {
            Field(
                words,
                {
                    words = it
                    state.clearFailure()
                },
                stringResource(R.string.agent_words),
                ink,
                single = false,
                enabled = !state.busy,
                keyboardOptions = KeyboardOptions(autoCorrectEnabled = false, keyboardType = KeyboardType.Password),
            )
        }
        if (state.busy) Box(Modifier.padding(vertical = 8.dp)) { Spinner(kit().accent) }
        agentError(state.failure)?.let {
            Box(Modifier.padding(vertical = 8.dp)) { AgentHint(it, kit().red) }
        }
    }
}

@Composable
private fun AgentEditPage(
    state: AgentAccounts,
    agent: AgentItem,
    profiles: Profiles,
    page: AgentPage,
    online: Boolean,
    onBack: () -> Unit,
    onPage: (AgentPage) -> Unit,
    choosePhoto: () -> Unit,
) {
    val ink = ink()
    val profile = profiles.shown[agent.id]
    val ready = online && !agent.disabled && !state.busy
    if (page == AgentPage.Edit) {
        Over(onBack, kit().ground) {
            PageBar(stringResource(R.string.agent_edit), ink, onBack)
            agentError(state.failure)?.let {
                Box(Modifier.padding(horizontal = 16.dp, vertical = 8.dp)) { AgentHint(it, kit().red) }
            }
            GroupCard(stringResource(R.string.profile_account), ink) {
                ValueLine(
                    stringResource(R.string.profile_change_photo),
                    null,
                    ink,
                    icon = Glyphs.cam,
                    onClick = choosePhoto.takeIf { ready },
                )
                ValueLine(
                    agent.display_name ?: agent.username,
                    null,
                    ink,
                    icon = Glyphs.person,
                    help = stringResource(R.string.profile_display_name),
                    chevron = true,
                    onClick = if (ready) ({ onPage(AgentPage.Name) }) else null,
                )
                ValueLine(
                    profile?.bio ?: stringResource(R.string.profile_bio_add),
                    null,
                    ink,
                    icon = Glyphs.info,
                    help = stringResource(R.string.profile_bio),
                    chevron = true,
                    onClick = if (ready && profile != null) ({ onPage(AgentPage.Bio) }) else null,
                )
                ValueLine(
                    stringResource(R.string.profile_links),
                    "${exact(profile?.link_list.orEmpty().size.toLong())} / ${exact(3)}",
                    ink,
                    icon = Glyphs.link,
                    chevron = true,
                    onClick = if (ready && profile != null) ({ onPage(AgentPage.Links) }) else null,
                )
            }
        }
        return
    }
    key(page) {
        val isName = page == AgentPage.Name
        val isLinks = page == AgentPage.Links
        var text by remember { mutableStateOf(if (isName) agent.display_name.orEmpty() else profile?.bio.orEmpty()) }
        var links by remember { mutableStateOf(profile?.link_list.orEmpty()) }
        val entered = links.map { ProfileLink(it.name.trim(), it.url.trim()) }.filter { it.name.isNotEmpty() || it.url.isNotEmpty() }
        val valid = if (isName) text.isNotBlank() else !isLinks || entered == usable(entered)
        val title =
            stringResource(
                if (isName) {
                    R.string.profile_display_name
                } else if (isLinks) {
                    R.string.profile_links
                } else {
                    R.string.profile_bio
                },
            )
        Over(onBack) {
            PageBar(title, ink, onBack) {
                IconButton(stringResource(R.string.group_save), {
                    if (ready && valid) {
                        val command =
                            if (isName) {
                                Command.UpdateAgent(agent.id, display_name = text.trim())
                            } else {
                                Command.AgentProfile(
                                    agent.id,
                                    if (isLinks) profile?.bio else text.trim().ifEmpty { null },
                                    if (isLinks) entered else links,
                                )
                            }
                        state.write(command) { onPage(AgentPage.Edit) }
                    }
                }) {
                    Glyph(Glyphs.check, if (ready && valid) kit().accent else ink.muted)
                }
            }
            Column(
                Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(horizontal = 16.dp, vertical = 10.dp),
                verticalArrangement = Arrangement.spacedBy(10.dp),
            ) {
                if (isLinks) {
                    AgentHint(stringResource(R.string.profile_links_up_to))
                    Links(links, ink, kit().card) { if (!state.busy) links = it }
                    if (!valid) AgentHint(stringResource(R.string.agent_links_invalid), kit().red)
                } else {
                    AgentHint(title, ink.text)
                    Field(text, { text = it }, "", ink, single = isName, max = if (isName) 64 else 140, enabled = !state.busy)
                }
                if (state.busy) Spinner(kit().accent)
                agentError(state.failure)?.let { AgentHint(it, kit().red) }
            }
        }
    }
}

@Composable
private fun AgentSetup(
    credentials: AgentCredentials,
    server: String,
    ink: Ink,
    onDone: () -> Unit,
) {
    val context = LocalContext.current
    val uri = LocalUriHandler.current
    var confirm by remember { mutableStateOf(false) }
    var linkFailed by remember { mutableStateOf(false) }
    val words = credentials.words
    Box(Modifier.fillMaxSize().background(ink.page)) {
        Over({ confirm = true }) {
            PageBar(stringResource(if (words == null) R.string.agent_new_token else R.string.agent_setup), ink, { confirm = true })
            Column(
                Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(horizontal = 16.dp, vertical = 10.dp),
                verticalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                AgentHint(stringResource(if (words == null) R.string.agent_keep_token else R.string.agent_keep_credentials))
                if (words != null) {
                    CredentialCard(stringResource(R.string.agent_server_url), server, secret = false, context = context)
                }
                CredentialCard(stringResource(R.string.agent_token), credentials.token, secret = true, context = context)
                if (words != null) {
                    CredentialCard(stringResource(R.string.agent_words), words, secret = true, context = context, showWords = true)
                }
                AgentHint(stringResource(R.string.agent_use_values))
                ValueLine(
                    stringResource(R.string.agent_configuration),
                    null,
                    ink,
                    tone = kit().accent,
                    icon = AgentGlyphs.external,
                    onClick = { linkFailed = runCatching { uri.openUri(CONFIGURATION) }.isFailure },
                )
                if (linkFailed) AgentHint(stringResource(R.string.agent_configuration_failed), kit().red)
            }
            WideButton(stringResource(R.string.profile_done), ink, Modifier.padding(horizontal = 16.dp, vertical = 8.dp), onClick = onDone)
        }
        if (confirm) {
            AskDialog(
                stringResource(R.string.agent_leave),
                stringResource(R.string.agent_leave_hint),
                { confirm = false },
                listOf(
                    DialogAct(stringResource(R.string.chat_cancel)) { confirm = false },
                    DialogAct(stringResource(R.string.agent_leave_button), onClick = onDone),
                ),
            )
        }
    }
}

@Composable
private fun CredentialCard(
    label: String,
    value: String,
    secret: Boolean,
    context: Context,
    showWords: Boolean = false,
) {
    val ink = ink()
    val shape = RoundedCornerShape(14.dp)
    Column(
        Modifier.fillMaxWidth().border(1.dp, ink.line, shape).background(kit().card, shape)
            .padding(start = 12.dp, end = 8.dp, top = 4.dp, bottom = 10.dp),
    ) {
        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
            Text(label, Modifier.weight(1f), color = ink.text, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
            IconButton(stringResource(R.string.agent_copy, label), {
                val clip = ClipData.newPlainText(label, value)
                if (secret && Build.VERSION.SDK_INT >= 33) {
                    clip.description.extras = PersistableBundle().apply { putBoolean(ClipDescription.EXTRA_IS_SENSITIVE, true) }
                }
                context.getSystemService(ClipboardManager::class.java)?.setPrimaryClip(clip)
                if (Build.VERSION.SDK_INT < 33) {
                    android.widget.Toast.makeText(context, R.string.chat_copied, android.widget.Toast.LENGTH_SHORT).show()
                }
            }) {
                Glyph(Glyphs.copy, kit().accent, 18.dp)
            }
        }
        CompositionLocalProvider(LocalLayoutDirection provides LayoutDirection.Ltr) {
            if (showWords) {
                val words = value.trim().split(Regex("\\s+"))
                for ((row, items) in words.chunked(3).withIndex()) {
                    Row(
                        Modifier.fillMaxWidth().padding(top = 4.dp),
                        horizontalArrangement = Arrangement.spacedBy(6.dp),
                    ) {
                        for ((column, word) in items.withIndex()) {
                            Row(Modifier.weight(1f).background(kit().ground, RoundedCornerShape(7.dp)).padding(5.dp)) {
                                Text(
                                    "${row * 3 + column + 1}",
                                    color = ink.muted,
                                    fontSize = 10.sp,
                                    modifier = Modifier.padding(end = 4.dp),
                                )
                                Text(word, color = ink.text, fontSize = 13.sp)
                            }
                        }
                    }
                }
            } else {
                Text(
                    value,
                    Modifier.fillMaxWidth(),
                    color = ink.text,
                    fontSize = 13.sp,
                    style = TextStyle(fontFamily = FontFamily.Monospace, textDirection = TextDirection.Ltr),
                )
            }
        }
    }
}

@Composable
private fun AgentHint(
    text: String,
    color: Color = ink().muted,
) {
    Text(text, color = color, fontSize = 13.5.sp, lineHeight = 19.sp)
}

@Composable
private fun agentError(error: Event.Answered?): String? {
    return when (error?.code) {
        null -> null
        "agents_off" -> stringResource(R.string.agent_server_off)
        "too_many_agents" -> stringResource(R.string.agent_limit)
        "username_taken" -> stringResource(R.string.agent_username_taken)
        "bad_username" -> stringResource(R.string.agent_username_invalid)
        "no_permission" -> stringResource(R.string.agent_action_forbidden)
        "not_held" -> stringResource(R.string.agent_permissions_changed)
        "agent_disabled" -> stringResource(R.string.agent_disabled)
        "bad_words" -> stringResource(R.string.agent_bad_words)
        "bad_signature" -> stringResource(R.string.agent_bad_signature)
        "offline", "network", "no_device" -> stringResource(R.string.agent_waiting_network)
        else -> error.message ?: stringResource(R.string.agent_failed)
    }
}

private object AgentGlyphs {
    val key = listOf("M14 9a4 4 0 1 0-4 4l-6 6v2h3v-3h3v-3l3-3a4 4 0 0 0 1-3z")
    val external = listOf("M14 4h6v6M20 4l-9 9M11 5H5v14h14v-6")
}
