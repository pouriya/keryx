// One background thread holds core's client, the WebSocket, the requests, the timer, the locked
// files and the chunks of files going up and coming down, like the web's worker. It does what core asks and feeds back what happened. Commands come
// from the screen as they are, and events go back as they are. Nothing here logs, since what passes
// through holds passwords, words, tokens and messages.

package io.github.pouriya.keryx

import android.content.Context
import android.graphics.Bitmap
import android.graphics.ImageDecoder
import android.media.MediaMetadataRetriever
import android.net.Uri
import android.os.ParcelFileDescriptor
import android.provider.OpenableColumns
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.put
import okhttp3.Call
import okhttp3.Callback
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import java.io.File
import java.io.FileInputStream
import java.io.IOException
import java.io.RandomAccessFile
import java.nio.ByteBuffer
import java.nio.channels.FileChannel
import java.util.Locale
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.Executors
import java.util.concurrent.ScheduledFuture
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicLong

/** How long a sticker's or photo's fetch is left to answer before it may be asked for again. */
private const val STICKER_RETRY_MS = 30_000L

/** What pickers call an MP3, which goes as music: android/AGENTS.md. */
private val MP3_TYPES = setOf("audio/mpeg", "audio/mp3", "audio/mpeg3")

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

    /** `step` for `chunk_read` and `fetched`, whose bytes cross beside their JSON. */
    @JvmStatic external fun stepBytes(
        handle: Long,
        input: String,
        bytes: ByteArray,
        now: Long,
    ): String

    /** The bytes a `put_chunk` or `write_chunk` held back, once. */
    @JvmStatic external fun chunk(
        handle: Long,
        chunk: Long,
    ): ByteArray?

    /** A BlurHash drawn as RGBA pixels, or null if it is not one. */
    @JvmStatic external fun blurhash(
        hash: String,
        width: Int,
        height: Int,
    ): ByteArray?

    @JvmStatic external fun drop(handle: Long)

    /** Connects the call the last `join_call` named. Blocks for up to 10 seconds. */
    @JvmStatic external fun callConnect(
        handle: Long,
        server: String,
    ): Boolean

    @JvmStatic external fun callAudio(
        handle: Long,
        ts: Long,
        bytes: ByteArray,
    )

    @JvmStatic external fun callVideo(
        handle: Long,
        ts: Long,
        bytes: ByteArray,
        keyframe: Boolean,
    )

    @JvmStatic external fun callState(
        handle: Long,
        state: String,
    )

    /** Null on a timeout, or byte 0 as the kind and then the frame: see android/AGENTS.md. */
    @JvmStatic external fun callNext(
        handle: Long,
        timeout: Long,
    ): ByteArray?
}

/** A small bitmap's pixels as RGBA, from which core makes a BlurHash. */
internal fun thumbnailOf(bitmap: Bitmap): Thumbnail {
    val width = bitmap.width
    val height = bitmap.height
    val pixels = IntArray(width * height)
    bitmap.getPixels(pixels, 0, width, 0, 0, width, height)
    val rgba = ByteArray(pixels.size * 4)
    for ((at, pixel) in pixels.withIndex()) {
        rgba[at * 4] = (pixel shr 16).toByte()
        rgba[at * 4 + 1] = (pixel shr 8).toByte()
        rgba[at * 4 + 2] = pixel.toByte()
        rgba[at * 4 + 3] = (pixel ushr 24).toByte()
    }
    return Thumbnail(width, height, rgba)
}

/** What asking a server for its public features found. */
enum class Reach {
    Answers,
    NotAddress,
    Unreachable,
    NotKeryx,
}

class Engine(
    private val context: Context,
    private val vault: Vault,
    private val platform: Platform,
    private val show: (Event) -> Unit,
    private val calls: (Output) -> Unit,
    private val unread: (Map<String, Int>) -> Unit,
    private val callEnded: (String, Long, Int) -> Unit,
    private val trying: () -> Unit,
) {
    private val thread = Executors.newSingleThreadScheduledExecutor()
    private val http = OkHttpClient()

    @Volatile private var server: String? = null

    @Volatile private var client = -1L

    /** The client's handle, for the call's own threads. */
    val handle get() = client

    /** The server's address, for the call's connection. */
    val base get() = server

    /** Bumped with each new client, so a reply meant for an old one is dropped. */
    private var generation = 0
    private var bound: String? = null
    private var archive: Archive? = null
    private var socket: WebSocket? = null
    private var timer: ScheduledFuture<*>? = null

    /** Files are read and their thumbnails made here, off the engine's thread. */
    private val preparing = Executors.newSingleThreadExecutor()
    private val handles = AtomicLong(1)

    /** Files going up, by handle: picked, then opened at their first chunk. */
    private val picked = HashMap<Long, Uri>()
    private val reading = HashMap<Long, Pair<ParcelFileDescriptor, FileChannel>>()

    /** Files coming down, by handle, into the app's cache. */
    private val downloads = ConcurrentHashMap<Long, File>()

    /** Where a download is written, once asked for. */
    fun downloadedFile(handle: Long): File? = downloads[handle]

    /** Stickers asked for and not yet answered, by id, with when: on the engine's thread. */
    private val stickersAsked = HashMap<String, Long>()

    /**
     * Where a sticker's WebP is kept once fetched, in the cache folder. A sticker's bytes never change,
     * as an id is never reused, so it is fetched again only if Android cleared the cache.
     */
    fun stickerFile(id: String) = File(File(context.cacheDir, "stickers"), "$id.webp")

    /** Fetches a sticker unless it is kept, or was asked for in the last 30 seconds. */
    fun sticker(id: String) =
        thread.execute {
            val now = System.currentTimeMillis()
            if (stickerFile(id).exists() || (stickersAsked[id] ?: 0L) > now - STICKER_RETRY_MS) return@execute
            stickersAsked[id] = now
            handle(Command.StickerImage(id))
        }

    /** Keeps a fetched sticker's WebP, written whole or not at all. */
    private fun keepSticker(event: Event.StickerImage): Boolean {
        stickersAsked.remove(event.sticker)
        val target = stickerFile(event.sticker)
        return try {
            target.parentFile?.mkdirs()
            val part = File(target.parentFile, "${event.sticker}.part")
            part.writeBytes(java.util.Base64.getDecoder().decode(event.bytes))
            part.renameTo(target)
        } catch (_: Exception) {
            false
        }
    }

    /** Photos asked for and not yet answered, by id, with when: on the engine's thread. */
    private val photosAsked = HashMap<String, Long>()

    /** This account, its photo when the upload began, the JPEG going up, and the photo its save named. */
    private var account: String? = null
    private var ownPhoto: String? = null
    private var uploadFrom: String? = null
    private var uploading: ByteArray? = null
    private var savedPhoto: String? = null

    /**
     * Where an opened photo is kept, in the cache folder, as android/AGENTS.md chose. A photo id is
     * never reused, so it is fetched again only if Android cleared the cache.
     */
    fun photoFile(id: String) = File(File(context.cacheDir, "photos"), "$id.jpg")

    /** Fetches and opens the photo a shown profile names, unless it is kept or was just asked for. */
    fun photo(
        account: String,
        id: String,
    ) = thread.execute {
        val now = System.currentTimeMillis()
        if (photoFile(id).exists() || (photosAsked[id] ?: 0L) > now - STICKER_RETRY_MS) return@execute
        photosAsked[id] = now
        handle(Command.Photo(account))
    }

    /** Keeps bytes as a file, written whole or not at all. */
    private fun keep(
        target: File,
        bytes: ByteArray,
    ): Boolean =
        try {
            target.parentFile?.mkdirs()
            val part = File(target.parentFile, "${target.name}.part")
            part.writeBytes(bytes)
            part.renameTo(target)
        } catch (_: Exception) {
            false
        }

    /**
     * Sends a new photo: a square JPEG and its small pixels, which cross JNI as bytes beside the
     * JSON. Once its save names it, the JPEG is kept as that photo, so it is never fetched back.
     */
    fun setPhoto(
        jpeg: ByteArray,
        thumbnail: Thumbnail,
    ) = thread.execute {
        uploadFrom = ownPhoto
        uploading = jpeg
        savedPhoto = null
        feedBytes(
            buildJsonObject {
                put("type", "set_photo")
                put("thumbnail", wire.encodeToJsonElement(Thumbnail.serializer(), thumbnail))
            },
            jpeg,
        )
    }

    /** Follows this account's own photo, and keeps an upload once its save named it. */
    private fun ownProfile(event: Event) {
        when (event) {
            is Event.SignedIn -> account = event.account
            is Event.Profile ->
                if (event.profile.account == account) {
                    ownPhoto = event.profile.photo?.id
                    savedPhoto = ownPhoto
                }
            is Event.ProfileSaved -> {
                val jpeg = uploading
                val named = savedPhoto
                if (jpeg != null && named != null && named != uploadFrom) {
                    keep(photoFile(named), jpeg)
                    uploading = null
                }
            }
            is Event.Error -> uploading = null
            is Event.LoggedOut -> {
                account = null
                ownPhoto = null
                uploading = null
            }
            else -> Unit
        }
    }

    /**
     * Sends the picked files as one message, with the caption unless it is blank. One file's kind
     * comes from its MIME type, and several go as a bundle, which core takes only as images and
     * videos, as android/AGENTS.md chose.
     */
    fun sendFiles(
        chat: String,
        uris: List<Uri>,
        caption: String,
    ) = preparing.execute {
        val specs = uris.mapNotNull { uri -> spec(uri)?.let { uri to it } }
        if (specs.isEmpty()) {
            return@execute
        }
        thread.execute {
            for ((uri, spec) in specs) {
                picked[spec.handle] = uri
            }
            handle(Command.SendFiles(chat, specs.map { it.second }, caption.ifBlank { null }))
        }
    }

    /** Fetches a shown file into the app's cache, and returns the handle its progress names. */
    fun download(file: FileInfo): Long {
        val handle = handles.getAndIncrement()
        val folder = File(File(context.cacheDir, "files"), file.id)
        val target = File(folder, file.name.replace('/', '_').trimStart('.').ifBlank { "file" })
        downloads[handle] = target
        thread.execute {
            folder.mkdirs()
            target.delete()
            handle(Command.Download(file, handle))
        }
        return handle
    }

    private fun spec(uri: Uri): FileSpec? {
        val resolver = context.contentResolver
        var name = "file"
        var size = -1L
        try {
            resolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE), null, null, null)?.use {
                if (it.moveToFirst()) {
                    name = it.getString(0) ?: name
                    if (!it.isNull(1)) size = it.getLong(1)
                }
            }
            if (size < 0) {
                size = resolver.openFileDescriptor(uri, "r")?.use { it.statSize } ?: -1
            }
        } catch (_: Exception) {
            return null
        }
        if (size < 0) {
            return null
        }
        val picked = resolver.getType(uri) ?: "application/octet-stream"
        // An MP3 is music whatever the picker called it, and always goes as audio/mpeg.
        val mp3 = picked in MP3_TYPES || name.lowercase(Locale.ROOT).endsWith(".mp3")
        val kind = if (mp3) 4 else kindOf(picked)
        val tags =
            if (mp3) {
                try {
                    tags(uri)
                } catch (_: Exception) {
                    null
                }
            } else {
                null
            }
        val picture =
            try {
                when (kind) {
                    3 -> image(ImageDecoder.createSource(resolver, uri))
                    4 -> tags?.cover
                    5 -> video(uri)
                    else -> null
                }
            } catch (_: Exception) {
                null
            }
        return FileSpec(
            handle = handles.getAndIncrement(),
            kind = kind,
            name = name,
            mime = if (mp3) "audio/mpeg" else picked,
            size = size,
            width = picture?.first,
            height = picture?.second,
            thumbnail = picture?.third,
            title = tags?.title,
            artist = tags?.artist,
            duration = tags?.duration,
        )
    }

    private fun kindOf(mime: String) =
        when {
            mime.startsWith("image/") -> 3
            mime.startsWith("video/") -> 5
            mime == "application/pdf" || mime == "application/msword" ||
                mime.startsWith("application/vnd.openxmlformats-officedocument.") ||
                mime.startsWith("application/vnd.ms-") ||
                mime.startsWith("application/vnd.oasis.opendocument.") -> 6
            else -> 7
        }

    private class Tags(
        val title: String?,
        val artist: String?,
        val duration: Long?,
        val cover: Triple<Int, Int, Thumbnail>?,
    )

    /** An MP3's title, artist and length in milliseconds from its tags, and its cover small. */
    private fun tags(uri: Uri): Tags {
        val retriever = MediaMetadataRetriever()
        try {
            retriever.setDataSource(context, uri)
            val text = { key: Int -> retriever.extractMetadata(key)?.takeIf { it.isNotBlank() } }
            val cover =
                retriever.embeddedPicture?.let { bytes ->
                    try {
                        image(ImageDecoder.createSource(ByteBuffer.wrap(bytes)))
                    } catch (_: Exception) {
                        null
                    }
                }
            return Tags(
                title = text(MediaMetadataRetriever.METADATA_KEY_TITLE),
                artist = text(MediaMetadataRetriever.METADATA_KEY_ARTIST),
                duration = text(MediaMetadataRetriever.METADATA_KEY_DURATION)?.toLongOrNull(),
                cover = cover,
            )
        } finally {
            retriever.release()
        }
    }

    /** An image's size, and its pixels decoded small for the BlurHash. */
    private fun image(source: ImageDecoder.Source): Triple<Int, Int, Thumbnail> {
        var width = 0
        var height = 0
        val bitmap =
            ImageDecoder.decodeBitmap(source) { decoder, info, _ ->
                width = info.size.width
                height = info.size.height
                val (small, smallHeight) = fit(width, height)
                decoder.setTargetSize(small, smallHeight)
                decoder.allocator = ImageDecoder.ALLOCATOR_SOFTWARE
            }
        return Triple(width, height, thumbnailOf(bitmap))
    }

    /** A video's size as shown, and its first frame small for the BlurHash. */
    private fun video(uri: Uri): Triple<Int, Int, Thumbnail>? {
        val retriever = MediaMetadataRetriever()
        try {
            retriever.setDataSource(context, uri)
            val number = { key: Int -> retriever.extractMetadata(key)?.toIntOrNull() ?: 0 }
            val turned = number(MediaMetadataRetriever.METADATA_KEY_VIDEO_ROTATION) % 180 != 0
            val wide = number(MediaMetadataRetriever.METADATA_KEY_VIDEO_WIDTH)
            val high = number(MediaMetadataRetriever.METADATA_KEY_VIDEO_HEIGHT)
            val (width, height) = if (turned) high to wide else wide to high
            val frame = retriever.getFrameAtTime(0) ?: return null
            val (small, smallHeight) = fit(frame.width, frame.height)
            return Triple(width, height, thumbnailOf(Bitmap.createScaledBitmap(frame, small, smallHeight, true)))
        } finally {
            retriever.release()
        }
    }

    /** At most 32 pixels on the longer side, which is plenty for a BlurHash. */
    private fun fit(
        width: Int,
        height: Int,
    ): Pair<Int, Int> {
        if (width <= 0 || height <= 0) {
            return 32 to 32
        }
        val longer = maxOf(width, height)
        return maxOf(1, width * 32 / longer) to maxOf(1, height * 32 / longer)
    }

    /**
     * Talks to this server from now on, after checking it answers as a keryx server.
     * [Reach.NotAddress] is not a URL, [Reach.Unreachable] did not answer, and [Reach.NotKeryx]
     * answered as something else.
     */
    fun use(
        address: String,
        answer: (Reach) -> Unit,
    ) {
        look(address) { reach, _ ->
            if (reach == Reach.Answers) {
                thread.execute { start(address.trim().trimEnd('/')) }
            }
            answer(reach)
        }
    }

    /**
     * The same features check as [use], without starting a client. [Reach.NotKeryx] answered as
     * something else. [Reach.Unreachable] did not answer, so a later try can.
     */
    fun look(
        address: String,
        answer: (Reach, String?) -> Unit,
    ) {
        val base = address.trim().trimEnd('/')
        val scheme = base.substringBefore("://", "").lowercase()
        if (scheme != "http" && scheme != "https") {
            answer(Reach.NotAddress, null)
            return
        }
        val request =
            try {
                Request.Builder().url("$base/api/v1/public/features").build()
            } catch (_: IllegalArgumentException) {
                answer(Reach.NotAddress, null)
                return
            }
        http.newCall(request).enqueue(
            object : Callback {
                override fun onFailure(
                    call: Call,
                    e: IOException,
                ) = answer(Reach.Unreachable, null)

                override fun onResponse(
                    call: Call,
                    response: Response,
                ) {
                    val body = response.use { if (it.isSuccessful) it.body?.string().orEmpty() else "" }
                    val fine = body.contains("\"ok\":true")
                    answer(if (fine) Reach.Answers else Reach.NotKeryx, if (fine) readName(body) else null)
                }
            },
        )
    }

    /** The server's name from a features body. Missing or blank means the client shows the host. */
    private fun readName(body: String): String? =
        try {
            org.json.JSONObject(body).optJSONObject("data")?.optString("name")?.trim()?.ifEmpty { null }
        } catch (_: org.json.JSONException) {
            null
        }

    fun send(command: Command) = thread.execute { handle(command) }

    /** Opens the last account kept for this server. The name check stays with the list. */
    fun openLast(address: String) {
        thread.execute { start(address.trim().trimEnd('/')) }
    }

    /**
     * Stops for good once its screen is gone: the socket closes and the client is dropped. The
     * threads stay idle rather than shut down, since a late socket callback still hands them work,
     * which the new generation turns away.
     */
    fun close() =
        thread.execute {
            socket?.close(1000, null)
            socket = null
            timer?.cancel(false)
            reading.values.forEach { it.first.close() }
            reading.clear()
            picked.clear()
            closeArchive()
            generation += 1
            if (client >= 0) {
                Core.drop(client)
                client = -1
            }
        }

    /** A call event as `Core.callNext` handed it over, fed back into core to be opened. */
    fun callEvent(json: String) =
        thread.execute {
            val event =
                try {
                    wire.parseToJsonElement(json).jsonObject
                } catch (_: IllegalArgumentException) {
                    return@execute
                }
            feed(Input.CallEvent(event))
        }

    private fun start(base: String) {
        vault.setNote("server", base)
        server = base
        release()
        val last = vault.note("last")
        if (last != null && last in vault.accounts(base)) {
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
        reading.values.forEach { it.first.close() }
        reading.clear()
        picked.clear()
        closeArchive()
        if (client >= 0) {
            Core.drop(client)
        }
        client = Core.create(wire.encodeToString(Platform.serializer(), platform))
        generation += 1
    }

    /** Resumes the last account's own snapshot, kept by its id, at start. */
    private fun bind(
        account: String,
        resume: Boolean,
    ) {
        val base = server ?: return
        bound = account
        vault.setNote("last", account)
        openArchive(base, account)
        val token = vault.get(base, account, "token")?.toString(Charsets.UTF_8)
        feed(Input.Loaded(vault.get(base, account, "slot0"), vault.get(base, account, "slot1"), resume, token))
    }

    /** A login found its account: hands core that account's snapshot, if this phone keeps one. */
    private fun open(account: String) {
        val base = server ?: return
        bound = account
        if (account in vault.accounts(base)) openArchive(base, account)
        feed(Input.Opened(vault.get(base, account, "slot0"), vault.get(base, account, "slot1")))
    }

    private fun handle(command: Command) {
        when (command) {
            // Core asks the server which account this is, then says which folder to open.
            is Command.LogIn, is Command.SignUp, is Command.Setup -> release()
            else -> Unit
        }
        feed(Input.Run(command))
    }

    private fun feed(input: Input) {
        if (client < 0) {
            return
        }
        outputs(Core.step(client, wire.encodeToString(Input.serializer(), input), System.currentTimeMillis()))
    }

    /** Feeds `chunk_read` or `fetched`, with their bytes beside the JSON. */
    private fun feedBytes(
        input: JsonObject,
        bytes: ByteArray,
    ) {
        if (client < 0) {
            return
        }
        outputs(Core.stepBytes(client, input.toString(), bytes, System.currentTimeMillis()))
    }

    private fun outputs(text: String) {
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

    /** Feeds bytes a callback on another thread fetched, unless the client is gone. */
    private fun laterBytes(
        meant: Int,
        input: JsonObject,
        bytes: ByteArray,
    ) = thread.execute {
        if (meant == generation) {
            feedBytes(input, bytes)
        }
    }

    /** Stops reading a file that went up, or will not. */
    private fun doneReading(handle: Long) {
        reading.remove(handle)?.first?.close()
        picked.remove(handle)
    }

    private fun readChunk(asked: Output.ReadChunk) {
        val bytes =
            try {
                val (_, channel) =
                    reading.getOrPut(asked.handle) {
                        val uri = picked[asked.handle] ?: throw IOException("not picked")
                        val opened = context.contentResolver.openFileDescriptor(uri, "r") ?: throw IOException("not opened")
                        opened to FileInputStream(opened.fileDescriptor).channel
                    }
                val buffer = ByteBuffer.allocate(asked.length.toInt())
                channel.position(asked.offset)
                while (buffer.hasRemaining() && channel.read(buffer) >= 0) {
                    continue
                }
                buffer.array().copyOf(buffer.position())
            } catch (_: Exception) {
                ByteArray(0)
            }
        feedBytes(
            buildJsonObject {
                put("type", "chunk_read")
                put("handle", asked.handle)
                put("n", asked.n)
            },
            bytes,
        )
    }

    private fun writeChunk(asked: Output.WriteChunk) {
        val bytes = Core.chunk(client, asked.chunk) ?: return
        val target = downloads[asked.handle] ?: return
        try {
            RandomAccessFile(target, "rw").use {
                it.seek(asked.offset)
                it.write(bytes)
            }
        } catch (_: IOException) {
            // Core counts the chunk written either way, and opening the file shows what is missing.
        }
    }

    private fun putChunk(
        base: String,
        asked: Output.PutChunk,
    ) {
        val meant = generation
        val bytes = Core.chunk(client, asked.chunk) ?: ByteArray(0)
        val builder =
            Request
                .Builder()
                .url(base + asked.path)
                .method(asked.method, bytes.toRequestBody("application/octet-stream".toMediaType()))
        asked.bearer?.let { builder.header("Authorization", "Bearer $it") }
        http.newCall(builder.build()).enqueue(
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

    private fun getChunk(
        base: String,
        asked: Output.GetChunk,
    ) {
        val meant = generation
        val fetched = { status: Int ->
            buildJsonObject {
                put("type", "fetched")
                put("id", asked.id)
                put("status", status)
            }
        }
        val builder = Request.Builder().url(base + asked.path)
        asked.bearer?.let { builder.header("Authorization", "Bearer $it") }
        http.newCall(builder.build()).enqueue(
            object : Callback {
                override fun onFailure(
                    call: Call,
                    e: IOException,
                ) = laterBytes(meant, fetched(0), ByteArray(0))

                override fun onResponse(
                    call: Call,
                    response: Response,
                ) {
                    val bytes =
                        try {
                            response.use { it.body?.bytes() ?: ByteArray(0) }
                        } catch (_: IOException) {
                            return laterBytes(meant, fetched(0), ByteArray(0))
                        }
                    laterBytes(meant, fetched(response.code), bytes)
                }
            },
        )
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
                val account = bound
                try {
                    if (account == null) {
                        throw IOException("no account")
                    }
                    vault.put(base, account, "slot${output.slot}", output.bytes)
                    feed(Input.Saved(output.id))
                } catch (_: Exception) {
                    feed(Input.SaveFailed(output.id))
                }
            }
            is Output.Wipe -> {
                bound?.let { account ->
                    vault.remove(base, account, "slot0")
                    vault.remove(base, account, "slot1")
                }
            }
            is Output.Open -> open(output.account)
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
            is Output.JoinCall, is Output.LeaveCall -> calls(output)
            is Output.ReadChunk -> readChunk(output)
            is Output.PutChunk -> putChunk(base, output)
            is Output.GetChunk -> getChunk(base, output)
            is Output.WriteChunk -> writeChunk(output)
            is Output.SaveRows -> saveRows(output)
            is Output.ReadRows -> readRows(output)
            is Output.Show -> {
                val showing = rememberedCall(output.event)
                ownProfile(showing)
                when (val event = showing) {
                    is Event.SignedIn -> {
                        bound = event.account
                        vault.setNote("last", event.account)
                        if (archive == null) openArchive(base, event.account)
                    }
                    is Event.Chats -> {
                        archive?.seen(event.chat_list)
                        reportUnread()
                    }
                    is Event.OwnDevices -> {
                        archive?.devices(event.device_list)
                        reportUnread()
                    }
                    is Event.Token ->
                        bound?.let { account ->
                            val token = event.token
                            if (token == null) {
                                vault.remove(base, account, "token")
                            } else {
                                vault.put(base, account, "token", token.toByteArray())
                            }
                        }
                    is Event.StickerImage -> if (!keepSticker(event)) return
                    is Event.Photo -> {
                        photosAsked.remove(event.photo)
                        val opened =
                            try {
                                java.util.Base64.getDecoder().decode(event.bytes)
                            } catch (_: IllegalArgumentException) {
                                return
                            }
                        if (!keep(photoFile(event.photo), opened)) return
                    }
                    is Event.StickerMissing -> stickersAsked.remove(event.sticker)
                    is Event.Progress -> if (event.done >= event.total) doneReading(event.handle)
                    is Event.TransferFailed -> doneReading(event.handle)
                    else -> Unit
                }
                show(showing)
            }
        }
    }

    private fun openArchive(
        base: String,
        account: String,
    ) {
        archive?.close()
        archive = null
        val dir = vault.accountDir(base, account)
        dir.mkdirs()
        archive =
            try {
                Archive.open(File(dir, "archive.db"))
            } catch (_: android.database.SQLException) {
                null
            }
    }

    private fun closeArchive() {
        archive?.close()
        archive = null
        unread(emptyMap())
    }

    private fun reportUnread() {
        val counts =
            try {
                archive?.unread() ?: emptyMap()
            } catch (_: android.database.SQLException) {
                emptyMap()
            }
        unread(counts)
    }

    private fun saveRows(asked: Output.SaveRows) {
        val kept = archive
        if (kept == null) {
            feed(Input.RowsSaveFailed(asked.id))
            return
        }
        try {
            kept.save(asked.message_list, asked.call_list)
        } catch (_: android.database.SQLException) {
            feed(Input.RowsSaveFailed(asked.id))
            return
        }
        reportUnread()
        for (row in asked.call_list) {
            val outcome = row.outcome
            if (row.kind == 4 && outcome != null) callEnded(row.chat, row.call_seq, outcome)
        }
        feed(Input.RowsSaved(asked.id))
    }

    /** A call opened before its end was noticed keeps the end this archive already has. */
    private fun rememberedCall(event: Event): Event {
        if (event is Event.Message && event.message.content_kind == 8 && event.message.call_outcome == null) {
            val outcome = storedCall(event.message.chat, event.message.seq) ?: return event
            return event.copy(message = event.message.copy(call_outcome = outcome))
        }
        if (event is Event.CallEvent && event.event.kind == 4 && event.event.outcome == null) {
            val outcome = storedCall(event.event.chat, event.event.call_seq) ?: return event
            return event.copy(event = event.event.copy(outcome = outcome))
        }
        return event
    }

    private fun storedCall(
        chat: String,
        seq: Long,
    ): Int? =
        try {
            archive?.callOutcome(chat, seq)
        } catch (_: android.database.SQLException) {
            null
        }

    private fun readRows(asked: Output.ReadRows) {
        val read =
            try {
                archive?.read(asked.chat, asked.before_seq, asked.limit)
            } catch (_: android.database.SQLException) {
                null
            }
        val (messages, calls) = read ?: (emptyList<MessageFrame>() to emptyList())
        feed(Input.RowsRead(asked.id, messages, calls))
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
        trying()
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
