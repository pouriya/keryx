// The bare screen: a few plain fields and buttons, the chats, the open chat's messages, and a list
// of what happened, enough to log in, send an encrypted message or files and make a call. The account
// screens replace it. What happened shows each event's type only, never words, passwords, tokens
// or message text. A file shows as a row that downloads it and opens it in another app. A ringing call shows as a card, and a joined call takes the whole screen.

package io.github.pouriya.keryx

import android.Manifest
import android.content.ActivityNotFoundException
import android.content.Intent
import android.content.pm.PackageManager
import android.graphics.Bitmap
import android.os.Build
import android.os.Bundle
import android.view.WindowManager
import androidx.activity.ComponentActivity
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
    private val server = mutableStateOf("http://127.0.0.1:7778")
    private val serverFine = mutableStateOf<Boolean?>(null)
    private val words = mutableStateOf<String?>(null)
    private val chats = mutableStateListOf<ChatItem>()
    private val chosen = mutableStateOf<String?>(null)
    private val messages = mutableStateListOf<Shown>()
    private val happened = mutableStateListOf<String>()
    private var lookingFor: String? = null
    private lateinit var engine: Engine

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

    override fun onCreate(saved: Bundle?) {
        super.onCreate(saved)
        val vault = Vault(applicationContext)
        vault.note("server")?.let { server.value = it }
        val platform = Platform(platform = 1, os = "android", os_version = Build.VERSION.RELEASE)
        engine =
            Engine(
                applicationContext,
                vault,
                platform,
                { event -> runOnUiThread { apply(event) } },
                { output -> runOnUiThread { call(output) } },
            )
        if (vault.note("server") != null) {
            engine.use(server.value) { fine -> runOnUiThread { serverFine.value = fine } }
        }
        setContent {
            MaterialTheme(colorScheme = if (isSystemInDarkTheme()) darkColorScheme() else lightColorScheme()) {
                Surface(Modifier.fillMaxSize()) {
                    val joined = session.value
                    if (joined == null) {
                        Bare()
                    } else {
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
                }
            }
        }
    }

    /** Rotating keeps this activity, so this runs only when it really goes: nothing it started runs on. */
    override fun onDestroy() {
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
            is Event.SignedIn -> if (event.device != null) words.value = null
            is Event.Chats -> {
                chats.clear()
                chats.addAll(event.chat_list)
            }
            is Event.Message -> {
                val message = event.message
                if (message.chat == chosen.value) messages.add(message)
                if (message.call != null && !message.mine && message.call_outcome == null && message.call_ended_at == null &&
                    session.value == null
                ) {
                    ringing.value = message
                }
            }
            is Event.Pending ->
                if (event.chat == starting && event.text == null && event.handles.isEmpty()) {
                    startingLocal = event.local
                    starting = null
                }
            is Event.Sent -> {
                if (event.local == startingLocal) {
                    startingLocal = null
                    engine.send(Command.JoinCall(event.chat, event.seq))
                }
                if (event.chat == chosen.value) choose(event.chat)
            }
            is Event.Failed -> {
                happened[0] = "Failed: ${event.code}"
                if (event.local == startingLocal) startingLocal = null
            }
            is Event.CallEvent -> callEvent(event.event)
            is Event.Downloaded -> {
                transfers[event.handle] = "downloaded"
                opening.remove(event.handle)?.let { mime -> open(event.handle, mime) }
            }
            is Event.TransferFailed -> transfers[event.handle] = "failed: ${event.code}"
            is Event.SearchResults ->
                event.account_list.firstOrNull { it.username == lookingFor }?.let {
                    engine.send(Command.StartChat(it.id))
                }
            is Event.Started -> {
                choose(event.chat)
                engine.send(Command.ChatList)
            }
            is Event.Error -> happened[0] = "Error: ${event.code}"
            is Event.LoggedOut -> {
                chats.clear()
                messages.clear()
                chosen.value = null
            }
            else -> Unit
        }
    }

    private fun callEvent(event: ShownCallEvent) {
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
                    engine.use(server.value) { fine -> runOnUiThread { serverFine.value = fine } }
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
                Text(
                    "$mark${if (chat.type == 0) "Saved messages" else chat.chat}",
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
