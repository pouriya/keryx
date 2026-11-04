// The bare screen: a few plain fields and buttons, the chats, the open chat's messages, and a list
// of what happened, enough to log in and send one encrypted message. The account screens replace
// it. What happened shows each event's type only, never words, passwords, tokens or message text.

package io.github.pouriya.keryx

import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.clickable
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp

class MainActivity : ComponentActivity() {
    private val server = mutableStateOf("http://127.0.0.1:7777")
    private val serverFine = mutableStateOf<Boolean?>(null)
    private val words = mutableStateOf<String?>(null)
    private val chats = mutableStateListOf<ChatItem>()
    private val chosen = mutableStateOf<String?>(null)
    private val messages = mutableStateListOf<Shown>()
    private val happened = mutableStateListOf<String>()
    private var lookingFor: String? = null
    private lateinit var engine: Engine

    override fun onCreate(saved: Bundle?) {
        super.onCreate(saved)
        val vault = Vault(applicationContext)
        vault.note("server")?.let { server.value = it }
        val platform = Platform(platform = 1, os = "android", os_version = Build.VERSION.RELEASE)
        engine = Engine(vault, platform) { event -> runOnUiThread { apply(event) } }
        if (vault.note("server") != null) {
            engine.use(server.value) { fine -> runOnUiThread { serverFine.value = fine } }
        }
        setContent {
            MaterialTheme(colorScheme = if (isSystemInDarkTheme()) darkColorScheme() else lightColorScheme()) {
                Surface(Modifier.fillMaxSize()) { Bare() }
            }
        }
    }

    private fun apply(event: Event) {
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
            is Event.Message -> if (event.message.chat == chosen.value) messages.add(event.message)
            is Event.Sent -> if (event.chat == chosen.value) choose(event.chat)
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
                for (message in messages) {
                    val who = if (message.mine) "me" else "them"
                    Text("${message.seq} $who: ${message.text ?: "(${message.problem ?: "no key"})"}")
                }
            }
            Text("What happened", style = MaterialTheme.typography.titleMedium)
            for (line in happened) {
                Text(line, style = MaterialTheme.typography.bodySmall)
            }
        }
    }
}
