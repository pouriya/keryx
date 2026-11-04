// One background thread holds core's client, the WebSocket, the requests, the timer and the locked
// files, like the web's worker. It does what core asks and feeds back what happened. Commands come
// from the screen as they are, and events go back as they are. Nothing here logs, since what passes
// through holds passwords, words, tokens and messages.

package io.github.pouriya.keryx

import okhttp3.Call
import okhttp3.Callback
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import java.io.IOException
import java.util.concurrent.Executors
import java.util.concurrent.ScheduledFuture
import java.util.concurrent.TimeUnit

/** The JNI glue in android/ffi. */
object Core {
    init {
        System.loadLibrary("keryx_ffi")
    }

    @JvmStatic external fun create(platform: String): Long

    @JvmStatic external fun step(
        handle: Long,
        input: String,
        now: Long,
    ): String

    @JvmStatic external fun drop(handle: Long)
}

class Engine(
    private val vault: Vault,
    private val platform: Platform,
    private val show: (Event) -> Unit,
) {
    private val thread = Executors.newSingleThreadScheduledExecutor()
    private val http = OkHttpClient()
    private var server: String? = null
    private var client = -1L

    /** Bumped with each new client, so a reply meant for an old one is dropped. */
    private var generation = 0
    private var bound: String? = null
    private var socket: WebSocket? = null
    private var timer: ScheduledFuture<*>? = null

    /** Talks to this server from now on, after checking it answers as a keryx server. */
    fun use(
        address: String,
        answer: (Boolean) -> Unit,
    ) {
        val base = address.trim().trimEnd('/')
        val request =
            try {
                Request.Builder().url("$base/api/v1/public/features").build()
            } catch (_: IllegalArgumentException) {
                answer(false)
                return
            }
        http.newCall(request).enqueue(
            object : Callback {
                override fun onFailure(
                    call: Call,
                    e: IOException,
                ) = answer(false)

                override fun onResponse(
                    call: Call,
                    response: Response,
                ) {
                    val fine = response.use { it.isSuccessful && (it.body?.string() ?: "").contains("\"ok\":true") }
                    if (fine) {
                        thread.execute { start(base) }
                    }
                    answer(fine)
                }
            },
        )
    }

    fun send(command: Command) = thread.execute { handle(command) }

    private fun start(base: String) {
        vault.setNote("server", base)
        server = base
        release()
        val last = vault.note("last")
        if (last != null && last in vault.usernames(base)) {
            bind(last, true)
        } else {
            feed(Input.Loaded(null, null, true, null))
        }
    }

    /** Drops whatever device is in memory, with a new client in its place. What is kept stays. */
    private fun release() {
        socket?.close(1000, null)
        socket = null
        timer?.cancel(false)
        bound = null
        if (client >= 0) {
            Core.drop(client)
        }
        client = Core.create(wire.encodeToString(Platform.serializer(), platform))
        generation += 1
    }

    /** Loads this username's own snapshot. A login does not connect until that login. */
    private fun bind(
        username: String,
        resume: Boolean,
    ) {
        val base = server ?: return
        if (bound == username) {
            vault.setNote("last", username)
            return
        }
        release()
        if (username !in vault.usernames(base)) {
            return
        }
        bound = username
        vault.setNote("last", username)
        val token = vault.get(base, username, "token")?.toString(Charsets.UTF_8)
        feed(Input.Loaded(vault.get(base, username, "slot0"), vault.get(base, username, "slot1"), resume, token))
    }

    private fun handle(command: Command) {
        when (command) {
            is Command.LogIn -> bind(command.username, false)
            is Command.SignUp, is Command.Setup -> release()
            else -> Unit
        }
        feed(Input.Run(command))
    }

    private fun feed(input: Input) {
        if (client < 0) {
            return
        }
        val text = Core.step(client, wire.encodeToString(Input.serializer(), input), System.currentTimeMillis())
        for (output in wire.decodeFromString(kotlinx.serialization.builtins.ListSerializer(Output.serializer()), text)) {
            run(output)
        }
    }

    /** Feeds what a callback on another thread learned, unless the client it was for is gone. */
    private fun later(
        meant: Int,
        input: () -> Input,
    ) = thread.execute {
        if (meant == generation) {
            feed(input())
        }
    }

    private fun run(output: Output) {
        val base = server ?: return
        when (output) {
            is Output.Send -> socket?.send(output.text)
            is Output.Request -> request(base, output)
            is Output.Connect -> connect(base, output.bearer)
            is Output.Disconnect -> {
                val closing = socket
                socket = null
                closing?.close(1000, null)
            }
            is Output.Save -> {
                val username = bound
                try {
                    if (username == null) {
                        throw IOException("no account")
                    }
                    vault.put(base, username, "slot${output.slot}", output.bytes)
                    feed(Input.Saved(output.id))
                } catch (_: Exception) {
                    feed(Input.SaveFailed(output.id))
                }
            }
            is Output.Wipe -> {
                bound?.let { username ->
                    vault.remove(base, username, "slot0")
                    vault.remove(base, username, "slot1")
                }
            }
            is Output.WakeAt -> {
                timer?.cancel(false)
                val meant = generation
                timer =
                    thread.schedule(
                        { if (meant == generation) feed(Input.Timer) },
                        maxOf(0, output.at - System.currentTimeMillis()),
                        TimeUnit.MILLISECONDS,
                    )
            }
            is Output.Show -> {
                when (val event = output.event) {
                    is Event.SignedIn -> {
                        bound = event.username
                        vault.setNote("last", event.username)
                    }
                    is Event.Token ->
                        bound?.let { username ->
                            val token = event.token
                            if (token == null) {
                                vault.remove(base, username, "token")
                            } else {
                                vault.put(base, username, "token", token.toByteArray())
                            }
                        }
                    else -> Unit
                }
                show(output.event)
            }
        }
    }

    private fun request(
        base: String,
        asked: Output.Request,
    ) {
        val meant = generation
        // OkHttp refuses a POST or PUT with no body, which core sends where there is nothing to say.
        val body = asked.body ?: if (asked.method == "GET" || asked.method == "DELETE") null else ""
        val built =
            try {
                val builder =
                    Request
                        .Builder()
                        .url(base + asked.path)
                        .method(asked.method, body?.toRequestBody("application/json".toMediaType()))
                asked.bearer?.let { builder.header("Authorization", "Bearer $it") }
                builder.build()
            } catch (_: IllegalArgumentException) {
                return later(meant) { Input.Reply(asked.id, 0, "") }
            }
        http.newCall(built).enqueue(
            object : Callback {
                override fun onFailure(
                    call: Call,
                    e: IOException,
                ) = later(meant) { Input.Reply(asked.id, 0, "") }

                override fun onResponse(
                    call: Call,
                    response: Response,
                ) {
                    val body =
                        try {
                            response.use { it.body?.string() ?: "" }
                        } catch (_: IOException) {
                            return later(meant) { Input.Reply(asked.id, 0, "") }
                        }
                    later(meant) { Input.Reply(asked.id, response.code, body) }
                }
            },
        )
    }

    private fun connect(
        base: String,
        bearer: String?,
    ) {
        socket?.close(1000, null)
        val meant = generation
        val url = base.replaceFirst("http", "ws") + "/api/v1/ws"
        val builder = Request.Builder().url(url).header("Sec-WebSocket-Protocol", "keryx.v1")
        bearer?.let { builder.header("Authorization", "Bearer $it") }
        socket =
            http.newWebSocket(
                builder.build(),
                object : WebSocketListener() {
                    fun mine(webSocket: WebSocket) = meant == generation && socket === webSocket

                    override fun onOpen(
                        webSocket: WebSocket,
                        response: Response,
                    ) = thread.execute { if (mine(webSocket)) feed(Input.Connected) }

                    override fun onMessage(
                        webSocket: WebSocket,
                        text: String,
                    ) = thread.execute { if (mine(webSocket)) feed(Input.Frame(text)) }

                    override fun onClosing(
                        webSocket: WebSocket,
                        code: Int,
                        reason: String,
                    ) {
                        webSocket.close(code, null)
                        thread.execute {
                            if (mine(webSocket)) {
                                socket = null
                                feed(Input.Closed(code, reason))
                            }
                        }
                    }

                    override fun onFailure(
                        webSocket: WebSocket,
                        t: Throwable,
                        response: Response?,
                    ) = thread.execute {
                        if (mine(webSocket)) {
                            socket = null
                            feed(Input.Closed(1006, ""))
                        }
                    }
                },
            )
    }
}
