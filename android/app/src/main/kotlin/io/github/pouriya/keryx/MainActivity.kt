// The bare screen: a few plain fields and buttons, the chats, the open chat's messages, and a list
// of what happened, enough to log in, send an encrypted message or files and make a call. The account
// screens replace it. What happened shows each event's type only, never words, passwords, tokens
// or message text. A file shows as a row that downloads it and opens it in another app. A ringing call shows as a card, and a joined call takes the whole screen.

package io.github.pouriya.keryx

import android.Manifest
import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.content.res.Configuration
import android.graphics.Bitmap
import android.net.ConnectivityManager
import android.net.Network
import android.os.Build
import android.os.Bundle
import android.view.WindowManager
import androidx.activity.ComponentActivity
import androidx.activity.compose.BackHandler
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.Image
import androidx.compose.foundation.clickable
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import androidx.core.content.FileProvider
import java.nio.ByteBuffer
import java.util.Locale

class MainActivity : ComponentActivity() {
    private lateinit var vault: Vault
    private val language = mutableStateOf("en")

    /** Welcome, then account setup, then words, then the chat list. The test screen stays in the code. */
    private enum class Place { Welcome, Setup, Words, Chats, Test }

    private enum class After { None, EnterSignUp, LogIn, SignUp, SetupLink }

    private val place = mutableStateOf(Place.Welcome)
    private val server = mutableStateOf("http://127.0.0.1:7778")
    private val serverFine = mutableStateOf<Boolean?>(null)
    private val address = mutableStateOf("http://127.0.0.1:7778")
    private val username = mutableStateOf("")
    private val password = mutableStateOf("")
    private val signingUp = mutableStateOf(false)
    private val addressError = mutableStateOf<Int?>(null)
    private val checking = mutableStateOf(false)
    private val reached = mutableStateOf(false)
    private val gapField = mutableStateOf("")
    private val focusNonce = mutableStateOf(0)
    private val linkError = mutableStateOf<Int?>(null)
    private val formError = mutableStateOf<String?>(null)
    private val registration = mutableStateOf<Boolean?>(null)
    private val captchaOn = mutableStateOf(false)
    private val captchaId = mutableStateOf<String?>(null)
    private val captchaImage = mutableStateOf<String?>(null)
    private val captchaAnswer = mutableStateOf("")
    private val busy = mutableStateOf(false)
    private val linkStage = mutableStateOf(false)
    private val setupPaste = mutableStateOf("")
    private val setupToken = mutableStateOf<String?>(null)
    private var linkBase: String? = null
    private var checkedAddress: String? = null
    private var pendingAddress: String? = null
    private var checkGeneration = 0
    private var afterCheck = After.None
    private val words = mutableStateOf<String?>(null)
    private val wordsRestore = mutableStateOf(false)
    private val wordsError = mutableStateOf<String?>(null)
    private var wordsBack: () -> Boolean = { false }
    private val chats = mutableStateListOf<ChatItem>()
    private val list = ChatRows()
    private val people = mutableStateMapOf<String, AccountBrief>()
    private val me = mutableStateOf<String?>(null)
    private val serverName = mutableStateOf<String?>(null)
    private val linkState = mutableStateOf(Link.Connecting)
    private val unreadCounts = mutableStateMapOf<String, Int>()
    private val found = mutableStateListOf<AccountBrief>()
    private val foundFor = mutableStateOf("")
    private var searches = 0
    private var latestSearch = ""
    private val chosen = mutableStateOf<String?>(null)
    private val messages = mutableStateListOf<Shown>()
    private val talk = Talk()
    private val timesAnswer = mutableStateOf<TimesAnswer?>(null)
    private var loadingOlder = false
    private val happened = mutableStateListOf<String>()
    private var lookingFor: String? = null
    private lateinit var engine: Engine
    private var netUp = true
    private var probeGen = 0
    private var leaving = false
    private var watchingNet = false
    private val connectivity by lazy { getSystemService(ConnectivityManager::class.java) }
    private val netCallback =
        object : ConnectivityManager.NetworkCallback() {
            override fun onAvailable(network: Network) {
                runOnUiThread {
                    val back = !netUp
                    netUp = true
                    if (back && place.value == Place.Chats && linkState.value == Link.Waiting) {
                        linkState.value = Link.Connecting
                        engine.send(Command.NetworkBack)
                        probe()
                    }
                }
            }

            override fun onLost(network: Network) {
                runOnUiThread {
                    netUp = false
                    if (place.value == Place.Chats) {
                        linkState.value = Link.Waiting
                        probeGen++
                    }
                }
            }
        }

    /** The call ringing here, as its start message, until it is answered, declined or ended. */
    private val ringing = mutableStateOf<Shown?>(null)
    private val session = mutableStateOf<CallSession?>(null)

    /** The chat a call was just started in, then its message's local id, until it has a seq. */
    private var starting: String? = null
    private var startingLocal: Long? = null
    private var wantVideo = false
    private var granted: (() -> Unit)? = null

    /** Each file asked for by its id, its download's handle, and each handle's progress as shown. */
    private val fetching = mutableStateMapOf<String, Long>()
    private val transfers = mutableStateMapOf<Long, String>()
    private val opening = HashMap<Long, String>()

    /** The chat and caption the picker was opened for. */
    private var picking: Pair<String, String>? = null
    private val picker =
        registerForActivityResult(ActivityResultContracts.OpenMultipleDocuments()) { uris ->
            val (chat, caption) = picking ?: return@registerForActivityResult
            picking = null
            if (uris.isNotEmpty()) {
                engine.sendFiles(chat, uris, caption)
            }
        }

    private val asking =
        registerForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) {
            granted?.invoke()
            granted = null
        }

    override fun attachBaseContext(newBase: Context) {
        val code = Vault(newBase).note("lang")?.takeIf { it == "en" || it == "fa" } ?: "en"
        val locale = Locale.forLanguageTag(code)
        val config = Configuration(newBase.resources.configuration)
        config.setLocale(locale)
        config.setLayoutDirection(locale)
        super.attachBaseContext(newBase.createConfigurationContext(config))
    }

    override fun onCreate(saved: Bundle?) {
        super.onCreate(saved)
        vault = Vault(applicationContext)
        language.value = vault.note("lang")?.takeIf { it == "en" || it == "fa" } ?: "en"
        vault.note("server")?.let {
            server.value = it
            address.value = it
        }
        val platform = Platform(platform = 1, os = "android", os_version = Build.VERSION.RELEASE)
        engine =
            Engine(
                applicationContext,
                vault,
                platform,
                { event -> runOnUiThread { apply(event) } },
                { output -> runOnUiThread { call(output) } },
                { counts ->
                    runOnUiThread {
                        unreadCounts.clear()
                        for ((chat, count) in counts) {
                            if (count > 0) unreadCounts[chat] = count
                        }
                    }
                },
                { chat, seq, outcome ->
                    runOnUiThread {
                        val video = list.rows.firstOrNull { it.id == chat }?.callVideo ?: return@runOnUiThread
                        list.callStatus(chat, seq, callLabel(video, outcome == 0))
                    }
                },
            )
        netUp = connectivity.activeNetwork != null
        runCatching {
            connectivity.registerDefaultNetworkCallback(netCallback)
            watchingNet = true
        }
        openSaved()
        setContent {
            val joined = session.value
            BackHandler(enabled = joined == null && place.value != Place.Welcome && place.value != Place.Chats) {
                when (place.value) {
                    Place.Words -> if (!wordsBack()) place.value = Place.Setup
                    Place.Test -> place.value = Place.Setup
                    Place.Setup -> if (linkStage.value) closeLink() else place.value = Place.Welcome
                    Place.Welcome, Place.Chats -> Unit
                }
            }
            MaterialTheme(colorScheme = if (isSystemInDarkTheme()) darkColorScheme() else lightColorScheme()) {
                if (joined != null) {
                    Surface(Modifier.fillMaxSize()) {
                        CallScreen(
                            joined,
                            {
                                    on ->
                                if (on) {
                                    withPermissions(
                                        Manifest.permission.CAMERA,
                                    ) { joined.video(has(Manifest.permission.CAMERA)) }
                                } else {
                                    joined.video(false)
                                }
                            },
                            { said -> if (said.isNotBlank()) engine.send(Command.CallText(joined.chat, joined.callSeq, said)) },
                            { engine.send(Command.LeaveCall(joined.chat)) },
                        )
                    }
                } else {
                    when (place.value) {
                        Place.Welcome -> Welcome(language.value, ::chooseLanguage) { place.value = Place.Setup }
                        Place.Setup ->
                            Setup(
                                link = linkStage.value,
                                signingUp = signingUp.value,
                                address = address.value,
                                onAddress = ::onAddress,
                                onAddressBlur = { ensureServer(After.None) },
                                username = username.value,
                                onUsername = ::onUsername,
                                password = password.value,
                                onPassword = ::onPassword,
                                setupLink = setupPaste.value,
                                onSetupLink = ::onSetupPaste,
                                onSetupLinkBlur = { prepareLink(false) },
                                checking = checking.value,
                                reached = reached.value,
                                addressError = addressError.value?.let { getString(it) },
                                linkError = linkError.value?.let { getString(it) },
                                formError = formError.value,
                                showCaptcha = captchaOn.value,
                                captchaImage = captchaImage.value,
                                captchaAnswer = captchaAnswer.value,
                                onCaptcha = ::onCaptcha,
                                onRefreshCaptcha = ::refreshCaptcha,
                                gap = gapField.value,
                                focusNonce = focusNonce.value,
                                busy = busy.value,
                                onPrimary = ::onPrimary,
                                onQuiet = ::onQuiet,
                                onLink = ::openLink,
                            )
                        Place.Words ->
                            Words(
                                server = normalized(address.value),
                                username = username.value.trim().lowercase(),
                                phrase = words.value,
                                restore = wordsRestore.value,
                                error = wordsError.value,
                                busy = busy.value,
                                onGenerate = { count ->
                                    wordsError.value = null
                                    engine.send(Command.GenerateWords(count))
                                },
                                onSet = { phrase ->
                                    wordsError.value = null
                                    busy.value = true
                                    engine.send(Command.SetWords(phrase))
                                },
                                onRestore = { phrase ->
                                    wordsError.value = null
                                    busy.value = true
                                    engine.send(Command.Restore(phrase))
                                },
                                onClearError = { wordsError.value = null },
                                onBindBack = { wordsBack = it },
                            )
                        Place.Chats ->
                            if (talk.chat != null) {
                                OpenedChat()
                            } else {
                                Chats(
                                    link = linkState.value,
                                    serverName = serverName.value,
                                    host = hostOf(server.value),
                                    people = people,
                                    me = me.value,
                                    unread = unreadCounts,
                                    rows = list.rows,
                                    ringing = ringing.value,
                                    found = found,
                                    foundFor = foundFor.value,
                                    onSearch = ::searchPeople,
                                    onPerson = { account -> engine.send(Command.StartChat(account.id)) },
                                    onOpen = { row -> openChat(row.id) },
                                    onLogOut = ::logOut,
                                    onAnswer = ::answer,
                                    onDecline = { call ->
                                        engine.send(Command.DeclineCall(call.chat, call.seq))
                                        ringing.value = null
                                    },
                                )
                            }
                        Place.Test -> Surface(Modifier.fillMaxSize()) { Bare() }
                    }
                }
            }
        }
    }

    /** English until FA is chosen. Kept as a plain note, and the activity is built again so the screen mirrors. */
    private fun chooseLanguage(code: String) {
        if (code == language.value) {
            return
        }
        vault.setNote("lang", code)
        recreate()
    }

    private fun normalized(raw: String) = raw.trim().trimEnd('/')

    private fun hostOf(address: String): String {
        val host = android.net.Uri.parse(address).host
        return if (host.isNullOrEmpty()) address else host
    }

    /** The directory, for a name that is not already a chat. An empty query drops the last answer. */
    private fun searchPeople(text: String) {
        latestSearch = text
        if (text.isEmpty()) {
            searches = 0
            found.clear()
            foundFor.value = ""
            return
        }
        searches++
        engine.send(Command.Search(text))
    }

    private fun searchAnswered(accounts: List<AccountBrief>) {
        searches = (searches - 1).coerceAtLeast(0)
        if (searches > 0) return
        found.clear()
        found.addAll(accounts)
        for (account in accounts) people[account.id] = account
        foundFor.value = latestSearch
    }

    /**
     * A kept login opens the list. The token is what was saved: without one, the welcome screen
     * stays, and a rejected session still falls back to Log in.
     */
    private fun openSaved() {
        val base = vault.note("server") ?: return
        val last = vault.note("last") ?: return
        if (last !in vault.usernames(base) || vault.get(base, last, "token") == null) return
        username.value = last
        place.value = Place.Chats
        linkState.value = if (netUp) Link.Connecting else Link.Waiting
        engine.openLast(base)
        if (linkState.value == Link.Connecting) probe()
    }

    private fun logOut() {
        leaving = true
        engine.send(Command.LogOut)
    }

    /** The list closes with nothing said. The address stays. The name and the password do not. */
    private fun leaveForLogin() {
        probeGen++
        leaving = false
        username.value = ""
        password.value = ""
        formError.value = null
        addressError.value = null
        linkError.value = null
        wordsError.value = null
        signingUp.value = false
        busy.value = false
        place.value = Place.Setup
    }

    /** While the title is Connecting, ask features again. An answer that is not keryx leaves the list. */
    private fun probe() {
        if (place.value != Place.Chats || linkState.value != Link.Connecting || !netUp) return
        val gen = ++probeGen
        engine.look(server.value) { reach, name ->
            runOnUiThread {
                if (place.value != Place.Chats) {
                    return@runOnUiThread
                }
                if (reach == Reach.Answers) {
                    serverName.value = name
                }
                if (gen != probeGen || linkState.value != Link.Connecting) {
                    return@runOnUiThread
                }
                if (reach == Reach.NotKeryx) {
                    leaveForLogin()
                } else {
                    window.decorView.postDelayed({
                        if (gen == probeGen && linkState.value == Link.Connecting) probe()
                    }, 2000)
                }
            }
        }
    }

    private fun previewOfBubble(bubble: Bubble): String {
        val text = bubble.text?.takeIf { it.isNotBlank() }
        return when (bubble.kind) {
            1 -> getString(R.string.kind_voice)
            2 -> text?.let { getString(R.string.kind_reacted, it) } ?: getString(R.string.kind_reacted, "").trim()
            3 -> text ?: getString(R.string.kind_photo)
            4 -> text ?: getString(R.string.kind_music)
            5 -> text ?: getString(R.string.kind_video)
            6 -> text ?: getString(R.string.kind_doc)
            7 -> text ?: getString(R.string.kind_file)
            8 -> callLabel(bubble.call?.video == true, bubble.callOutcome == 0)
            9 -> text ?: getString(R.string.kind_bundle)
            else -> text.orEmpty()
        }
    }

    private fun previewOf(message: Shown): String {
        val text = message.text?.takeIf { it.isNotBlank() }
        return when (message.content_kind) {
            1 -> getString(R.string.kind_voice)
            2 -> text?.let { getString(R.string.kind_reacted, it) } ?: getString(R.string.kind_reacted, "").trim()
            3 -> text ?: getString(R.string.kind_photo)
            4 -> text ?: getString(R.string.kind_music)
            5 -> text ?: getString(R.string.kind_video)
            6 -> text ?: getString(R.string.kind_doc)
            7 -> text ?: getString(R.string.kind_file)
            8 -> callLabel(message.call?.video == true, message.call_outcome == 0)
            9 -> text ?: getString(R.string.kind_bundle)
            else -> text.orEmpty()
        }
    }

    /** A call's preview: missed or not, and video when the caller had it on. */
    private fun callLabel(
        video: Boolean,
        missed: Boolean,
    ): String =
        when {
            missed && video -> getString(R.string.call_missed_video)
            missed -> getString(R.string.call_missed_voice)
            video -> getString(R.string.call_video)
            else -> getString(R.string.call_voice)
        }

    private fun onAddress(value: String) {
        address.value = value
        addressError.value = null
        formError.value = null
        if (gapField.value == "address") gapField.value = ""
        if (normalized(value) != checkedAddress) {
            checkGeneration += 1
            pendingAddress = null
            checking.value = false
            reached.value = false
            checkedAddress = null
            registration.value = null
            captchaOn.value = false
            clearCaptcha()
            signingUp.value = false
            afterCheck = After.None
        }
    }

    private fun onUsername(value: String) {
        username.value = value
        formError.value = null
        if (gapField.value == "username") gapField.value = ""
    }

    private fun onPassword(value: String) {
        password.value = value
        formError.value = null
        if (gapField.value == "password") gapField.value = ""
    }

    private fun onCaptcha(value: String) {
        captchaAnswer.value = value
        formError.value = null
        if (gapField.value == "captcha") gapField.value = ""
    }

    private fun onSetupPaste(value: String) {
        setupPaste.value = value
        linkError.value = null
        addressError.value = null
        formError.value = null
        if (gapField.value == "link") gapField.value = ""
        setupToken.value = null
        linkBase = null
        checkGeneration += 1
        pendingAddress = null
        checking.value = false
        reached.value = false
        checkedAddress = null
        registration.value = null
        captchaOn.value = false
        clearCaptcha()
    }

    private fun clearCaptcha() {
        captchaId.value = null
        captchaImage.value = null
        captchaAnswer.value = ""
    }

    /** Asks this address what it allows. A check already running for it is left to finish. */
    private fun ensureServer(next: After) {
        val base = normalized(if (linkStage.value) linkBase ?: return else address.value)
        if (!linkStage.value && base.isEmpty()) {
            checking.value = false
            reached.value = false
            addressError.value = R.string.setup_need_address
            return
        }
        afterCheck = next
        if (base == checkedAddress || base == pendingAddress) {
            if (registration.value != null) {
                finishCheck(registration.value == true)
            }
            return
        }
        check(base)
    }

    private fun check(base: String) {
        val id = ++checkGeneration
        pendingAddress = base
        checking.value = true
        reached.value = false
        addressError.value = null
        engine.use(base) { reach ->
            runOnUiThread {
                if (id != checkGeneration) {
                    return@runOnUiThread
                }
                pendingAddress = null
                checking.value = false
                when (reach) {
                    Reach.NotAddress -> failAddress(R.string.setup_need_address)
                    Reach.Unreachable -> failAddress(R.string.setup_unreachable)
                    Reach.NotKeryx -> failAddress(R.string.setup_not_keryx)
                    Reach.Answers -> {
                        addressError.value = null
                        checkedAddress = base
                        server.value = base
                        reached.value = true
                    }
                }
            }
        }
    }

    private fun failAddress(message: Int) {
        addressError.value = message
        checkedAddress = null
        reached.value = false
        checking.value = false
        registration.value = null
        afterCheck = After.None
    }

    private fun finishCheck(open: Boolean) {
        val next = afterCheck
        afterCheck = After.None
        when (next) {
            After.EnterSignUp ->
                if (open) {
                    signingUp.value = true
                } else {
                    formError.value = getString(R.string.setup_registration_closed)
                }
            After.LogIn -> submit(false)
            After.SignUp ->
                if (open) {
                    submit(true)
                } else {
                    formError.value = getString(R.string.setup_registration_closed)
                }
            After.SetupLink -> submitLink()
            After.None -> Unit
        }
    }

    private fun captchaValue(): CaptchaAnswer? {
        val id = captchaId.value ?: return null
        val answer = captchaAnswer.value.trim()
        if (answer.isEmpty()) {
            return null
        }
        return CaptchaAnswer(id, answer)
    }

    private fun submit(signUp: Boolean) {
        val name = username.value.trim().lowercase()
        if (name.isEmpty() || password.value.isEmpty()) {
            return
        }
        val picture = captchaValue()
        if (captchaOn.value && picture == null) {
            return
        }
        formError.value = null
        busy.value = true
        engine.send(
            if (signUp) {
                Command.SignUp(name, password.value, captcha = picture)
            } else {
                Command.LogIn(name, password.value, captcha = picture)
            },
        )
    }

    /** The first box with nothing in it, in the order they are shown. */
    private fun emptyBox(): String? {
        if (linkStage.value) {
            if (setupPaste.value.isBlank()) return "link"
        } else if (normalized(address.value).isEmpty()) {
            return "address"
        }
        if (!linkStage.value && username.value.trim().isEmpty()) return "username"
        if (password.value.isEmpty()) return "password"
        if (captchaOn.value && captchaAnswer.value.trim().isEmpty()) return "captcha"
        return null
    }

    private fun showGap(field: String) {
        gapField.value = field
        focusNonce.value += 1
        formError.value = null
        if (field == "address") addressError.value = R.string.setup_need_address
    }

    private fun onPrimary() {
        if (busy.value) {
            return
        }
        val gap = emptyBox()
        if (gap != null) {
            showGap(gap)
            return
        }
        if (linkStage.value) {
            prepareLink(true)
        } else {
            ensureServer(if (signingUp.value) After.SignUp else After.LogIn)
        }
    }

    private fun onQuiet() {
        if (busy.value) {
            return
        }
        formError.value = null
        if (linkStage.value) {
            closeLink()
            return
        }
        if (signingUp.value) {
            signingUp.value = false
            return
        }
        when (registration.value) {
            false -> formError.value = getString(R.string.setup_registration_closed)
            true -> signingUp.value = true
            null -> ensureServer(After.EnterSignUp)
        }
    }

    private fun refreshCaptcha() {
        captchaAnswer.value = ""
        captchaId.value = null
        captchaImage.value = null
        engine.send(Command.Captcha)
    }

    private fun openLink() {
        linkStage.value = true
        setupPaste.value = ""
        setupToken.value = null
        linkBase = null
        linkError.value = null
        formError.value = null
        addressError.value = null
        password.value = ""
        checkedAddress = null
        pendingAddress = null
        checking.value = false
        reached.value = false
        registration.value = null
        captchaOn.value = false
        clearCaptcha()
        checkGeneration += 1
        afterCheck = After.None
        signingUp.value = false
    }

    private fun closeLink() {
        linkStage.value = false
        setupToken.value = null
        linkBase = null
        linkError.value = null
        formError.value = null
        addressError.value = null
        password.value = ""
        checkedAddress = null
        pendingAddress = null
        checking.value = false
        reached.value = false
        registration.value = null
        captchaOn.value = false
        clearCaptcha()
        checkGeneration += 1
        afterCheck = After.None
    }

    private fun onFeatures(event: Event.Features) {
        registration.value = event.registration
        captchaOn.value = event.captcha
        serverName.value = event.name?.trim()?.ifEmpty { null }
        if (event.captcha && captchaImage.value == null) {
            engine.send(Command.Captcha)
        }
        finishCheck(event.registration)
    }

    private fun accountMessage(
        code: String,
        message: String,
    ) = when (code) {
        "bad_link" -> getString(R.string.link_unknown)
        "link_used" -> getString(R.string.link_used)
        "link_expired" -> getString(R.string.link_expired)
        "wrong_words" -> getString(R.string.wrong_words)
        "bad_words" -> getString(R.string.bad_checksum)
        "keys_set" -> getString(R.string.keys_set)
        else -> message
    }

    /** A pasted `https://server/setup#token`. A missing token is reported apart from a link that is not one. */
    private data class ParsedLink(
        val base: String,
        val token: String?,
    )

    private fun parseSetupLink(raw: String): ParsedLink? {
        val uri = android.net.Uri.parse(raw.trim())
        val scheme = uri.scheme?.lowercase()
        val host = uri.host
        if ((scheme != "http" && scheme != "https") || host.isNullOrEmpty()) {
            return null
        }
        if (uri.path?.trimEnd('/') != "/setup") {
            return null
        }
        val port = if (uri.port == -1) "" else ":${uri.port}"
        val token =
            try {
                uri.fragment?.let { java.net.URLDecoder.decode(it, Charsets.UTF_8.name()) }?.takeIf { it.isNotEmpty() }
            } catch (_: IllegalArgumentException) {
                return null
            }
        return ParsedLink("$scheme://$host$port", token)
    }

    private fun prepareLink(submit: Boolean) {
        if (setupPaste.value.isBlank()) {
            return
        }
        val parsed = parseSetupLink(setupPaste.value)
        if (parsed == null) {
            linkError.value = R.string.link_unknown
            return
        }
        if (parsed.token == null) {
            linkError.value = R.string.setup_missing
            return
        }
        linkError.value = null
        setupToken.value = parsed.token
        linkBase = parsed.base
        if (submit) {
            afterCheck = After.SetupLink
        }
        if (parsed.base == checkedAddress && registration.value != null) {
            if (submit) {
                finishCheck(registration.value == true)
            }
            return
        }
        if (parsed.base != pendingAddress) {
            check(parsed.base)
        }
    }

    private fun submitLink() {
        val token = setupToken.value ?: return
        if (password.value.isEmpty()) {
            return
        }
        val picture = captchaValue()
        if (captchaOn.value && picture == null) {
            return
        }
        formError.value = null
        busy.value = true
        engine.send(Command.Setup(token, password.value, captcha = picture))
    }

    /** Rotating keeps this activity, so this runs only when it really goes: nothing it started runs on. */
    override fun onDestroy() {
        if (watchingNet) connectivity.unregisterNetworkCallback(netCallback)
        session.value?.close()
        session.value = null
        engine.close()
        super.onDestroy()
    }

    private fun apply(event: Event) {
        if (event is Event.Progress) {
            transfers[event.handle] = "${event.done} of ${event.total} chunks"
            return
        }
        happened.add(0, event::class.simpleName ?: "?")
        if (happened.size > 40) {
            happened.removeAt(happened.lastIndex)
        }
        when (event) {
            is Event.Words -> words.value = event.words
            is Event.Features -> onFeatures(event)
            is Event.Captcha -> {
                captchaId.value = event.id
                captchaImage.value = event.image
                captchaAnswer.value = ""
            }
            is Event.Registered -> {
                busy.value = false
                signingUp.value = false
                formError.value = null
                clearCaptcha()
                if (captchaOn.value) {
                    engine.send(Command.Captcha)
                }
            }
            is Event.SignedIn -> {
                me.value = event.account
                if (event.device != null) words.value = null
                busy.value = false
                afterCheck = After.None
                if (event.device != null && (place.value == Place.Words || place.value == Place.Setup)) {
                    place.value = Place.Chats
                    if (linkState.value != Link.Connected && linkState.value != Link.Updating) {
                        linkState.value = if (netUp) Link.Connecting else Link.Waiting
                        if (linkState.value == Link.Connecting) probe()
                    } else {
                        engine.look(server.value) { reach, name ->
                            runOnUiThread {
                                if (place.value == Place.Chats && reach == Reach.Answers) serverName.value = name
                            }
                        }
                    }
                }
            }
            is Event.NeedLogin -> if (place.value == Place.Chats) leaveForLogin()
            is Event.Online -> {
                if (netUp) linkState.value = Link.Updating
                probeGen++
            }
            is Event.Offline -> {
                linkState.value = if (netUp) Link.Connecting else Link.Waiting
                if (linkState.value == Link.Connecting && place.value == Place.Chats) probe()
            }
            is Event.Account -> people[event.account.id] = event.account
            is Event.NeedWords -> {
                busy.value = false
                afterCheck = After.None
                wordsRestore.value = event.restore
                wordsError.value = null
                words.value = null
                if (!event.restore) engine.send(Command.GenerateWords(12))
                place.value = Place.Words
            }
            is Event.Chats -> {
                chats.clear()
                chats.addAll(event.chat_list)
                list.replace(event.chat_list)
                // The first list comes from what this device kept, before any connection: only one
                // that follows `Online` says the server was reached.
                if (netUp && linkState.value == Link.Updating) {
                    linkState.value = Link.Connected
                    probeGen++
                }
            }
            is Event.Message -> {
                val message = event.message
                val known = list.rows.any { it.id == message.chat }
                list.message(message, previewOf(message))
                talk.message(message)
                if (!known) engine.send(Command.ChatList)
                if (message.chat == chosen.value) messages.add(message)
                // A call kept from the last run is not ringing: only one that arrives live is.
                val live = linkState.value == Link.Updating || linkState.value == Link.Connected
                if (live && message.call != null && !message.mine && message.call_outcome == null &&
                    message.call_ended_at == null && session.value == null
                ) {
                    ringing.value = message
                }
            }
            is Event.Pending -> {
                talk.pending(event)
                if (event.chat == starting && event.text == null && event.handles.isEmpty()) {
                    startingLocal = event.local
                    starting = null
                }
                if (event.text != null || event.handles.isNotEmpty()) {
                    list.pending(event.chat, event.text ?: getString(R.string.kind_file))
                }
            }
            is Event.Sent -> {
                talk.sent(event)
                list.sent(event.chat, event.seq, event.saved_at)
                if (event.local == startingLocal) {
                    startingLocal = null
                    engine.send(Command.JoinCall(event.chat, event.seq))
                }
                if (event.chat == chosen.value && place.value == Place.Test) choose(event.chat)
                list.walkBack(event.chat, talk, ::previewOfBubble)
            }
            is Event.Marks -> list.marks(event.chat, event.mark)
            is Event.Edited -> talk.edited(event)
            is Event.Deleted -> {
                talk.deleted(event.target_seq)
                talk.chat?.let { list.walkBack(it, talk, ::previewOfBubble) }
            }
            is Event.Removed -> {
                talk.removed(event.seq)
                talk.chat?.let { list.walkBack(it, talk, ::previewOfBubble) }
            }
            is Event.PageLoaded -> {
                if (event.chat == talk.chat) talk.page(event.has_older)
                loadingOlder = false
            }
            is Event.Draft -> talk.drafted(event)
            is Event.Times -> timesAnswer.value = TimesAnswer(event.seq, event.member_list)
            is Event.Failed -> {
                talk.failed(event)
                happened[0] = "Failed: ${event.code}"
                if (event.local == startingLocal) startingLocal = null
            }
            is Event.CallEvent -> callEvent(event.event)
            is Event.Downloaded -> {
                transfers[event.handle] = "downloaded"
                opening.remove(event.handle)?.let { mime -> open(event.handle, mime) }
            }
            is Event.TransferFailed -> transfers[event.handle] = "failed: ${event.code}"
            is Event.SearchResults -> {
                if (place.value == Place.Chats) searchAnswered(event.account_list)
                event.account_list.firstOrNull { it.username == lookingFor }?.let {
                    engine.send(Command.StartChat(it.id))
                }
            }
            is Event.Started -> {
                if (place.value == Place.Chats) openChat(event.chat) else choose(event.chat)
                engine.send(Command.ChatList)
            }
            is Event.Error -> {
                happened[0] = "Error: ${event.code}"
                timesAnswer.value?.let { if (it.member_list == null) timesAnswer.value = it.copy(member_list = emptyList()) }
                busy.value = false
                if (place.value == Place.Chats && searches > 0) searchAnswered(emptyList())
                val shown = accountMessage(event.code, event.message)
                if (place.value == Place.Words) wordsError.value = shown
                if (place.value == Place.Setup) formError.value = shown
            }
            is Event.DeviceLimit -> {
                busy.value = false
                if (place.value == Place.Chats) {
                    leaveForLogin()
                } else {
                    if (place.value == Place.Words) wordsError.value = event.message
                    if (place.value == Place.Setup) formError.value = event.message
                }
            }
            is Event.LoggedOut -> {
                chats.clear()
                list.clear()
                people.clear()
                messages.clear()
                talk.close()
                unreadCounts.clear()
                chosen.value = null
                me.value = null
                if (place.value != Place.Welcome) {
                    val own = leaving
                    leaving = false
                    password.value = ""
                    formError.value = null
                    addressError.value = null
                    signingUp.value = false
                    busy.value = false
                    if (!own) username.value = ""
                    probeGen++
                    place.value = Place.Setup
                }
            }
            else -> Unit
        }
    }

    private fun callEvent(event: ShownCallEvent) {
        val outcome = event.outcome
        if (event.kind == 4 && outcome != null) {
            val video = list.rows.firstOrNull { it.id == event.chat }?.callVideo
            if (video != null) list.callStatus(event.chat, event.call_seq, callLabel(video, outcome == 0))
        }
        ringing.value?.let {
            // An end or a decline stops the ring, and so does a join from this account's other device.
            val stops = event.kind == 4 || event.kind == 2 || (event.kind == 0 && event.mine)
            if (event.chat == it.chat && event.call_seq == it.seq && stops) {
                ringing.value = null
            }
        }
        val joined = session.value ?: return
        if (event.chat != joined.chat || event.call_seq != joined.callSeq) {
            return
        }
        val who = if (event.mine) "me" else "them"
        when (event.kind) {
            0 -> {
                joined.texts.add("$who joined")
                joined.keyframe()
            }
            1 -> joined.texts.add("$who left")
            3 -> joined.texts.add("$who: ${event.text ?: "(${event.problem ?: "no key"})"}")
            4 -> joined.texts.add("the call ended")
        }
    }

    private fun call(output: Output) {
        when (output) {
            is Output.JoinCall -> {
                session.value?.close()
                ringing.value = null
                val degrees = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) (display?.rotation ?: 0) * 90 else 0
                session.value =
                    CallSession(applicationContext, engine, output.chat, output.call_seq, wantVideo, degrees) { reason ->
                        runOnUiThread { happened.add(0, "Call closed: $reason") }
                    }.also { it.start() }
                window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
            }
            is Output.LeaveCall -> {
                session.value?.close()
                session.value = null
                window.clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
            }
            else -> Unit
        }
    }

    /** Downloads a file the first time, and opens it in another app once it is here. */
    private fun fetch(file: FileInfo) {
        val handle = fetching[file.id]
        if (handle != null && transfers[handle] == "downloaded") {
            open(handle, file.mime)
            return
        }
        if (handle != null && transfers[handle]?.startsWith("failed") != true) {
            return
        }
        val started = engine.download(file)
        fetching[file.id] = started
        transfers[started] = "asked"
        opening[started] = file.mime
    }

    private fun open(
        handle: Long,
        mime: String,
    ) {
        val file = engine.downloadedFile(handle) ?: return
        val uri = FileProvider.getUriForFile(this, "$packageName.files", file)
        val view =
            Intent(Intent.ACTION_VIEW)
                .setDataAndType(uri, mime)
                .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        try {
            startActivity(view)
        } catch (_: ActivityNotFoundException) {
            happened.add(0, "No app opens $mime")
        }
    }

    private fun size(bytes: Long): String =
        when {
            bytes >= 1 shl 20 -> String.format(Locale.ROOT, "%.1f MB", bytes / 1048576.0)
            bytes >= 1 shl 10 -> String.format(Locale.ROOT, "%.1f KB", bytes / 1024.0)
            else -> "$bytes B"
        }

    @Composable
    private fun FileRow(file: FileInfo) {
        val kind =
            when (file.kind) {
                3 -> "image"
                4 -> "music"
                5 -> "video"
                6 -> "doc"
                else -> "file"
            }
        val picture =
            remember(file.blurhash) {
                file.blurhash?.let { hash ->
                    val width = 32
                    val height = 32
                    Core.blurhash(hash, width, height)?.let { rgba ->
                        Bitmap
                            .createBitmap(width, height, Bitmap.Config.ARGB_8888)
                            .apply { copyPixelsFromBuffer(ByteBuffer.wrap(rgba)) }
                            .asImageBitmap()
                    }
                }
            }
        Row(
            Modifier.fillMaxWidth().clickable { fetch(file) }.padding(vertical = 4.dp),
            horizontalArrangement = Arrangement.spacedBy(8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            picture?.let { Image(it, null, Modifier.size(40.dp)) }
            Column(Modifier.weight(1f)) {
                Text(file.name)
                val tags =
                    listOfNotNull(
                        file.title,
                        file.artist,
                        file.duration?.let { "%d:%02d".format(Locale.ROOT, it / 60000, it / 1000 % 60) },
                    )
                if (tags.isNotEmpty()) {
                    Text(tags.joinToString(" · "), style = MaterialTheme.typography.bodySmall)
                }
                val state = fetching[file.id]?.let { transfers[it] }
                Text("$kind, ${size(file.size)}${state?.let { ", $it" } ?: ""}", style = MaterialTheme.typography.bodySmall)
            }
            Text("Open")
        }
    }

    private fun has(permission: String) = checkSelfPermission(permission) == PackageManager.PERMISSION_GRANTED

    /** Asks for what is missing, then goes on whatever the answer, as a refused sense stays off. */
    private fun withPermissions(
        vararg names: String,
        then: () -> Unit,
    ) {
        if (names.all(::has)) {
            then()
        } else {
            granted = then
            asking.launch(arrayOf(*names))
        }
    }

    private fun startCall(
        chat: String,
        video: Boolean,
    ) {
        val needs =
            if (video) {
                arrayOf(
                    Manifest.permission.RECORD_AUDIO,
                    Manifest.permission.CAMERA,
                )
            } else {
                arrayOf(Manifest.permission.RECORD_AUDIO)
            }
        withPermissions(*needs) {
            wantVideo = video && has(Manifest.permission.CAMERA)
            starting = chat
            engine.send(Command.StartCall(chat, wantVideo, has(Manifest.permission.RECORD_AUDIO)))
        }
    }

    private fun answer(call: Shown) {
        val video = call.call?.video == true
        val needs =
            if (video) {
                arrayOf(
                    Manifest.permission.RECORD_AUDIO,
                    Manifest.permission.CAMERA,
                )
            } else {
                arrayOf(Manifest.permission.RECORD_AUDIO)
            }
        withPermissions(*needs) {
            wantVideo = video && has(Manifest.permission.CAMERA)
            engine.send(Command.JoinCall(call.chat, call.seq))
        }
    }

    private fun openChat(chat: String) {
        timesAnswer.value = null
        loadingOlder = false
        talk.open(chat)
        engine.send(Command.OpenChat(chat))
    }

    private fun pointer(bubble: Bubble) = Pointer(seq = bubble.seq, local = bubble.local)

    @Composable
    private fun OpenedChat() {
        val chat = talk.chat ?: return
        val row = list.rows.firstOrNull { it.id == chat }
        val saved = getString(R.string.saved_messages)
        ChatScreen(
            talk = talk,
            title = row?.let { label(it, people, saved) }.orEmpty(),
            link = linkState.value,
            type = row?.type ?: 1,
            me = me.value,
            people = people,
            marks = row?.marks ?: emptyList(),
            times = timesAnswer.value,
            onBack = { talk.close() },
            onSend = { text, reply -> engine.send(Command.SendText(chat, text, reply?.let(::pointer))) },
            onEdit = { bubble, text -> engine.send(Command.Edit(chat, pointer(bubble), text)) },
            onDelete = { list ->
                for (bubble in list) engine.send(Command.Delete(chat, pointer(bubble), bubble.files.map { it.id }))
            },
            onTimes = { seq ->
                timesAnswer.value = TimesAnswer(seq, null)
                engine.send(Command.Times(chat, seq))
            },
            onOlder = {
                if (!loadingOlder) {
                    loadingOlder = true
                    engine.send(Command.LoadOlder(chat))
                }
            },
            onSeen = { seq -> engine.send(Command.Seen(chat, seq)) },
            onDraft = { text ->
                talk.keep(chat, text)
                engine.send(Command.Draft(chat, text))
            },
            onFile = ::fetch,
        )
    }

    private fun choose(chat: String) {
        chosen.value = chat
        messages.clear()
        engine.send(Command.OpenChat(chat))
    }

    @Composable
    private fun Bare() {
        val username = remember { mutableStateOf("") }
        val password = remember { mutableStateOf("") }
        val restore = remember { mutableStateOf("") }
        val other = remember { mutableStateOf("") }
        val text = remember { mutableStateOf("") }
        Column(
            Modifier.safeDrawingPadding().padding(12.dp).verticalScroll(rememberScrollState()),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedTextField(server.value, { server.value = it }, Modifier.weight(1f), label = { Text("Server") })
                Button({
                    engine.use(server.value) { reach -> runOnUiThread { serverFine.value = reach == Reach.Answers } }
                }) { Text("Use") }
            }
            serverFine.value?.let { Text(if (it) "Server answers" else "Not a keryx server") }
            OutlinedTextField(
                username.value,
                { username.value = it },
                Modifier.fillMaxWidth(),
                label = { Text("Username") },
                singleLine = true,
            )
            OutlinedTextField(
                password.value,
                { password.value = it },
                Modifier.fillMaxWidth(),
                label = { Text("Password") },
                singleLine = true,
                visualTransformation = PasswordVisualTransformation(),
                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password),
            )
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Button({ engine.send(Command.SignUp(username.value.trim().lowercase(), password.value)) }) { Text("Sign up") }
                Button({ engine.send(Command.LogIn(username.value.trim().lowercase(), password.value)) }) { Text("Log in") }
                Button({ engine.send(Command.LogOut) }) { Text("Log out") }
            }
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Button({ engine.send(Command.GenerateWords(12)) }) { Text("New words") }
                words.value?.let { shown -> Button({ engine.send(Command.SetWords(shown)) }) { Text("Set them") } }
            }
            words.value?.let { Text(it) }
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedTextField(restore.value, { restore.value = it }, Modifier.weight(1f), label = { Text("Words to restore") })
                Button({ engine.send(Command.Restore(restore.value.trim())) }) { Text("Restore") }
            }
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedTextField(other.value, { other.value = it }, Modifier.weight(1f), label = { Text("Chat with username") })
                Button({
                    lookingFor = other.value.trim().lowercase()
                    engine.send(Command.Search(other.value.trim()))
                }) { Text("Start") }
            }
            ringing.value?.let { call ->
                Card(Modifier.fillMaxWidth()) {
                    Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                        val kind = if (call.call?.video == true) "video" else "voice"
                        Text("Incoming $kind call from ${call.account ?: "someone"}")
                        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            Button({ answer(call) }) { Text("Answer") }
                            Button({
                                engine.send(Command.DeclineCall(call.chat, call.seq))
                                ringing.value = null
                            }) { Text("Decline") }
                        }
                    }
                }
            }
            Text("Chats", style = MaterialTheme.typography.titleMedium)
            for (chat in chats) {
                val mark = if (chat.chat == chosen.value) "▶ " else ""
                val name = if (chat.type == 0) "Saved messages" else chat.chat
                val count = unreadCounts[chat.chat]?.let { " ($it)" } ?: ""
                Text(
                    "$mark$name$count",
                    Modifier.fillMaxWidth().clickable { choose(chat.chat) }.padding(vertical = 4.dp),
                )
            }
            chosen.value?.let { chat ->
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    OutlinedTextField(text.value, { text.value = it }, Modifier.weight(1f), label = { Text("Message") })
                    Button({
                        engine.send(Command.SendText(chat, text.value))
                        text.value = ""
                    }) { Text("Send") }
                }
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Button({
                        picking = chat to text.value
                        text.value = ""
                        picker.launch(arrayOf("*/*"))
                    }) { Text("File") }
                }
                for ((handle, state) in transfers) {
                    if (handle !in fetching.values) Text("Sending $handle: $state", style = MaterialTheme.typography.bodySmall)
                }
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Button({ startCall(chat, true) }) { Text("Video call") }
                    Button({ startCall(chat, false) }) { Text("Voice call") }
                }
                for (message in messages) {
                    val who = if (message.mine) "me" else "them"
                    val shown =
                        message.call?.let { "${if (it.video) "video" else "voice"} call" }
                            ?: message.text
                            ?: if (message.files.isNotEmpty()) "" else "(${message.problem ?: "no key"})"
                    Text("${message.seq} $who: $shown")
                    for (file in message.files) {
                        FileRow(file)
                    }
                }
            }
            Text("What happened", style = MaterialTheme.typography.titleMedium)
            for (line in happened) {
                Text(line, style = MaterialTheme.typography.bodySmall)
            }
        }
    }
}
