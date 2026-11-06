// A call: its screen and everything it drives, for testing calls only until the redesign. The
// camera draws straight into the H.264 encoder, the microphone feeds the Opus encoder, and both go
// to keryx-moq through the ffi, which seals them. What arrives is decoded here: Opus through a
// small jitter buffer to the speaker, and H.264 straight into a SurfaceView. The key never leaves
// Rust. Nothing here logs, since what passes through is the call itself. See android/AGENTS.md.

package io.github.pouriya.keryx

import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import android.hardware.camera2.CameraCaptureSession
import android.hardware.camera2.CameraCharacteristics
import android.hardware.camera2.CameraDevice
import android.hardware.camera2.CameraManager
import android.hardware.camera2.CaptureRequest
import android.hardware.camera2.params.OutputConfiguration
import android.hardware.camera2.params.SessionConfiguration
import android.media.AudioAttributes
import android.media.AudioDeviceInfo
import android.media.AudioFormat
import android.media.AudioManager
import android.media.AudioRecord
import android.media.AudioTrack
import android.media.MediaCodec
import android.media.MediaCodecInfo
import android.media.MediaFormat
import android.media.MediaRecorder
import android.media.audiofx.AcousticEchoCanceler
import android.media.audiofx.NoiseSuppressor
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.HandlerThread
import android.util.Range
import android.util.Size
import android.view.Surface
import android.view.SurfaceHolder
import android.view.SurfaceView
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.boolean
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.int
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.put
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.util.concurrent.Executor
import kotlin.math.abs

private const val RATE = 48_000
private const val FRAME_SAMPLES = 960
private const val FRAME_BYTES = FRAME_SAMPLES * 2
private const val VIDEO_BITRATE = 500_000
private const val VIDEO_FPS = 15

/** The jitter buffer's start and limit, in 20 ms frames: 60 ms growing to 200 ms. */
private const val JITTER_START = 3
private const val JITTER_MOST = 10

/** One joined call. `start()` connects on the call's own thread; `close()` stops everything. */
class CallSession(
    private val context: Context,
    private val engine: Engine,
    val chat: String,
    val callSeq: Long,
    video: Boolean,
    private val degrees: Int,
    private val closed: (String) -> Unit,
) {
    val status = mutableStateOf("Connecting")
    val muted = mutableStateOf(false)
    val videoOn = mutableStateOf(video && has(Manifest.permission.CAMERA))
    val remoteVideo = mutableStateOf(false)
    val remoteAspect = mutableStateOf(9f / 16f)
    val texts = mutableStateListOf<String>()

    private val handle = engine.handle

    @Volatile private var running = true

    @Volatile private var connected = false

    @Volatile private var sendVoice = true

    @Volatile private var sendVideo = videoOn.value

    private val audio = context.getSystemService(Context.AUDIO_SERVICE) as AudioManager
    private val cameraThread = HandlerThread("keryx-camera").apply { start() }
    private val cameraHandler = Handler(cameraThread.looper)

    // Set once the camera's thread has quit: Camera2's onClosed and a destroyed preview still post
    // after that, and a post to a dead thread logs a warning, so they are dropped here.
    @Volatile private var cameraGone = false

    private fun onCamera(block: () -> Unit) {
        if (!cameraGone) cameraHandler.post(block)
    }

    private val cameraExecutor = Executor { onCamera(it::run) }

    // Only the camera's thread touches these.
    private var camera: CameraDevice? = null
    private var captureSession: CameraCaptureSession? = null
    private var preview: Surface? = null
    private var size = Size(640, 360)
    private var rotation = 0

    @Volatile private var encoder: MediaCodec? = null

    // Only the receive thread touches these, apart from the surface the screen hands over.
    private val players = HashMap<String, Player>()
    private var decoder: MediaCodec? = null
    private var decoderRotation = -1

    @Volatile private var remoteSurface: Surface? = null

    @Volatile private var remoteRotation = 0

    @Volatile private var remoteSize = Size(640, 360)

    val previewHolder =
        object : SurfaceHolder.Callback {
            override fun surfaceCreated(holder: SurfaceHolder) {
                val surface = holder.surface
                onCamera {
                    preview = surface
                    openCamera()
                }
            }

            override fun surfaceChanged(
                holder: SurfaceHolder,
                format: Int,
                width: Int,
                height: Int,
            ) = Unit

            override fun surfaceDestroyed(holder: SurfaceHolder) {
                onCamera {
                    closeCamera()
                    preview = null
                }
            }
        }

    val remoteHolder =
        object : SurfaceHolder.Callback {
            override fun surfaceCreated(holder: SurfaceHolder) {
                remoteSurface = holder.surface
            }

            override fun surfaceChanged(
                holder: SurfaceHolder,
                format: Int,
                width: Int,
                height: Int,
            ) = Unit

            override fun surfaceDestroyed(holder: SurfaceHolder) {
                remoteSurface = null
            }
        }

    private fun has(permission: String) = context.checkSelfPermission(permission) == PackageManager.PERMISSION_GRANTED

    fun start() {
        audio.mode = AudioManager.MODE_IN_COMMUNICATION
        speaker(true)
        Thread({
            connected = Core.callConnect(handle, engine.base ?: "")
            if (!connected) {
                status.value = "Could not connect"
                closed("connect_failed")
                engine.send(Command.LeaveCall(chat))
                return@Thread
            }
            status.value = "In call"
            sendState()
            Thread(::receive, "keryx-call-receive").start()
            onCamera(::openCamera)
            capture()
        }, "keryx-call").start()
    }

    fun close() {
        running = false
        connected = false
        onCamera {
            closeCamera()
            cameraGone = true
            cameraThread.quitSafely()
        }
        speaker(false)
        audio.mode = AudioManager.MODE_NORMAL
    }

    fun mute(on: Boolean) {
        muted.value = on
        sendVoice = !on
        sendState()
    }

    fun video(on: Boolean) {
        videoOn.value = on
        sendVideo = on
        onCamera { if (on) openCamera() else closeCamera() }
        sendState()
    }

    /** Someone joined: they get a keyframe now, not at the next one. */
    fun keyframe() {
        try {
            encoder?.setParameters(Bundle().apply { putInt(MediaCodec.PARAMETER_KEY_REQUEST_SYNC_FRAME, 0) })
        } catch (_: IllegalStateException) {
            // The encoder is stopping, and the next one starts with a keyframe anyway.
        }
    }

    private fun sendState() {
        if (!connected) {
            return
        }
        val state =
            buildJsonObject {
                put("video", sendVideo)
                put("voice", sendVoice && has(Manifest.permission.RECORD_AUDIO))
                if (sendVideo) {
                    put("width", size.width)
                    put("height", size.height)
                    put("rotation", rotation)
                }
            }
        Core.callState(handle, state.toString())
    }

    private fun speaker(on: Boolean) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            if (on) {
                audio.availableCommunicationDevices
                    .firstOrNull { it.type == AudioDeviceInfo.TYPE_BUILTIN_SPEAKER }
                    ?.let { audio.setCommunicationDevice(it) }
            } else {
                audio.clearCommunicationDevice()
            }
        } else {
            olderSpeaker(on)
        }
    }

    /** Android 10 and 11 have no communication device to choose. */
    @Suppress("DEPRECATION")
    private fun olderSpeaker(on: Boolean) {
        audio.isSpeakerphoneOn = on
    }

    // ---------------------------------------------------------------------------------------------
    // Sending
    // ---------------------------------------------------------------------------------------------

    /** The microphone into Opus, 20 ms at a time, on the call's own thread until the call ends. */
    private fun capture() {
        if (context.checkSelfPermission(Manifest.permission.RECORD_AUDIO) != PackageManager.PERMISSION_GRANTED) {
            return
        }
        val least = AudioRecord.getMinBufferSize(RATE, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT)
        val record =
            AudioRecord(
                MediaRecorder.AudioSource.VOICE_COMMUNICATION,
                RATE,
                AudioFormat.CHANNEL_IN_MONO,
                AudioFormat.ENCODING_PCM_16BIT,
                maxOf(least, FRAME_BYTES * 4),
            )
        if (record.state != AudioRecord.STATE_INITIALIZED) {
            record.release()
            return
        }
        val echo = if (AcousticEchoCanceler.isAvailable()) AcousticEchoCanceler.create(record.audioSessionId) else null
        echo?.enabled = true
        val noise = if (NoiseSuppressor.isAvailable()) NoiseSuppressor.create(record.audioSessionId) else null
        noise?.enabled = true
        val format =
            MediaFormat.createAudioFormat(MediaFormat.MIMETYPE_AUDIO_OPUS, RATE, 1).apply {
                setInteger(MediaFormat.KEY_BIT_RATE, 24_000)
            }
        val opus = MediaCodec.createEncoderByType(MediaFormat.MIMETYPE_AUDIO_OPUS)
        opus.configure(format, null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
        opus.start()
        record.startRecording()
        val pcm = ByteArray(FRAME_BYTES)
        val info = MediaCodec.BufferInfo()
        try {
            while (running) {
                val read = record.read(pcm, 0, pcm.size)
                if (read <= 0 || !sendVoice) {
                    continue
                }
                val input = opus.dequeueInputBuffer(10_000)
                if (input >= 0) {
                    opus.getInputBuffer(input)?.apply {
                        clear()
                        put(pcm, 0, read)
                    }
                    opus.queueInputBuffer(input, 0, read, System.nanoTime() / 1000, 0)
                }
                while (true) {
                    val output = opus.dequeueOutputBuffer(info, 0)
                    if (output < 0) {
                        break
                    }
                    if (info.flags and MediaCodec.BUFFER_FLAG_CODEC_CONFIG == 0 && info.size > 0) {
                        val bytes = ByteArray(info.size)
                        opus.getOutputBuffer(output)?.apply {
                            position(info.offset)
                            get(bytes)
                        }
                        Core.callAudio(handle, info.presentationTimeUs, bytes)
                    }
                    opus.releaseOutputBuffer(output, false)
                }
            }
        } finally {
            record.stop()
            record.release()
            echo?.release()
            noise?.release()
            opus.stop()
            opus.release()
        }
    }

    /** The front camera into H.264, once the call is connected, video is on and the preview is up. */
    private fun openCamera() {
        if (camera != null || !connected || !sendVideo || preview == null) {
            return
        }
        if (context.checkSelfPermission(Manifest.permission.CAMERA) != PackageManager.PERMISSION_GRANTED) {
            return
        }
        val manager = context.getSystemService(Context.CAMERA_SERVICE) as CameraManager
        val id =
            manager.cameraIdList.firstOrNull {
                manager.getCameraCharacteristics(it).get(CameraCharacteristics.LENS_FACING) ==
                    CameraCharacteristics.LENS_FACING_FRONT
            } ?: manager.cameraIdList.firstOrNull() ?: return
        val traits = manager.getCameraCharacteristics(id)
        val sizes = traits.get(CameraCharacteristics.SCALER_STREAM_CONFIGURATION_MAP)?.getOutputSizes(MediaCodec::class.java) ?: return
        size = sizes.filter { it.width * 9 == it.height * 16 && it.width >= 640 }.minByOrNull { it.width }
            ?: sizes.minByOrNull { abs(it.width - 640) + abs(it.height - 360) } ?: return
        val front = traits.get(CameraCharacteristics.LENS_FACING) == CameraCharacteristics.LENS_FACING_FRONT
        val sensor = traits.get(CameraCharacteristics.SENSOR_ORIENTATION) ?: 0
        rotation = if (front) (sensor + degrees) % 360 else (sensor - degrees + 360) % 360
        val ranges = traits.get(CameraCharacteristics.CONTROL_AE_AVAILABLE_TARGET_FPS_RANGES) ?: emptyArray()
        val fps = ranges.filter { VIDEO_FPS in it }.minByOrNull { it.upper } ?: Range(VIDEO_FPS, VIDEO_FPS)
        val codec = videoEncoder() ?: return
        val input = codec.createInputSurface()
        codec.start()
        encoder = codec
        Thread({ drainVideo(codec, input) }, "keryx-call-video").start()
        sendState()
        val shown = preview ?: return
        try {
            manager.openCamera(
                id,
                cameraExecutor,
                object : CameraDevice.StateCallback() {
                    override fun onOpened(device: CameraDevice) {
                        if (!sendVideo || !running) {
                            device.close()
                            return
                        }
                        camera = device
                        repeat(device, listOf(input, shown), fps)
                    }

                    override fun onDisconnected(device: CameraDevice) = device.close()

                    override fun onError(
                        device: CameraDevice,
                        error: Int,
                    ) = device.close()
                },
            )
        } catch (_: SecurityException) {
            closeCamera()
        }
    }

    private fun videoEncoder(): MediaCodec? {
        for (profile in listOf(
            MediaCodecInfo.CodecProfileLevel.AVCProfileConstrainedBaseline,
            MediaCodecInfo.CodecProfileLevel.AVCProfileBaseline,
        )) {
            val format =
                MediaFormat.createVideoFormat(MediaFormat.MIMETYPE_VIDEO_AVC, size.width, size.height).apply {
                    setInteger(MediaFormat.KEY_COLOR_FORMAT, MediaCodecInfo.CodecCapabilities.COLOR_FormatSurface)
                    setInteger(MediaFormat.KEY_BIT_RATE, VIDEO_BITRATE)
                    setInteger(MediaFormat.KEY_FRAME_RATE, VIDEO_FPS)
                    setInteger(MediaFormat.KEY_I_FRAME_INTERVAL, 2)
                    setInteger(MediaFormat.KEY_PROFILE, profile)
                    setInteger(MediaFormat.KEY_LEVEL, MediaCodecInfo.CodecProfileLevel.AVCLevel31)
                }
            val codec = MediaCodec.createEncoderByType(MediaFormat.MIMETYPE_VIDEO_AVC)
            try {
                codec.configure(format, null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
                return codec
            } catch (_: MediaCodec.CodecException) {
                codec.release()
            } catch (_: IllegalArgumentException) {
                codec.release()
            }
        }
        return null
    }

    /** Both surfaces, the encoder's and the preview, at the chosen frame rate. */
    private fun repeat(
        device: CameraDevice,
        targets: List<Surface>,
        fps: Range<Int>,
    ) {
        val callback =
            object : CameraCaptureSession.StateCallback() {
                override fun onConfigured(session: CameraCaptureSession) {
                    if (camera !== device) {
                        session.close()
                        return
                    }
                    captureSession = session
                    val request =
                        device.createCaptureRequest(CameraDevice.TEMPLATE_RECORD).apply {
                            targets.forEach { addTarget(it) }
                            set(CaptureRequest.CONTROL_AE_TARGET_FPS_RANGE, fps)
                        }
                    try {
                        session.setRepeatingRequest(request.build(), null, cameraHandler)
                    } catch (_: IllegalStateException) {
                        closeCamera()
                    }
                }

                override fun onConfigureFailed(session: CameraCaptureSession) = closeCamera()
            }
        try {
            device.createCaptureSession(
                SessionConfiguration(
                    SessionConfiguration.SESSION_REGULAR,
                    targets.map { OutputConfiguration(it) },
                    cameraExecutor,
                    callback,
                ),
            )
        } catch (_: Exception) {
            closeCamera()
        }
    }

    /** Stops the camera; the encoder's own thread then stops and releases it. */
    private fun closeCamera() {
        captureSession?.close()
        captureSession = null
        camera?.close()
        camera = null
        encoder = null
    }

    /** The encoder's output, a keyframe carrying its SPS and PPS in front, as AGENTS.md chose. */
    private fun drainVideo(
        codec: MediaCodec,
        input: Surface,
    ) {
        val info = MediaCodec.BufferInfo()
        var config = ByteArray(0)
        try {
            while (running && encoder === codec) {
                val output = codec.dequeueOutputBuffer(info, 20_000)
                if (output < 0) {
                    continue
                }
                val bytes = ByteArray(info.size)
                codec.getOutputBuffer(output)?.apply {
                    position(info.offset)
                    get(bytes)
                }
                codec.releaseOutputBuffer(output, false)
                if (info.flags and MediaCodec.BUFFER_FLAG_CODEC_CONFIG != 0) {
                    config = bytes
                } else if (bytes.isNotEmpty()) {
                    val key = info.flags and MediaCodec.BUFFER_FLAG_KEY_FRAME != 0
                    Core.callVideo(handle, info.presentationTimeUs, if (key) config + bytes else bytes, key)
                }
            }
        } catch (_: IllegalStateException) {
            // Stopped under us.
        } finally {
            try {
                codec.stop()
            } catch (_: IllegalStateException) {
                // Already stopped.
            }
            codec.release()
            input.release()
        }
    }

    // ---------------------------------------------------------------------------------------------
    // Receiving
    // ---------------------------------------------------------------------------------------------

    private fun receive() {
        try {
            while (running) {
                val asked = System.nanoTime()
                val frame = Core.callNext(handle, 20)
                drainDecoder()
                if (frame == null) {
                    if (System.nanoTime() - asked < 5_000_000) {
                        Thread.sleep(20)
                    }
                    continue
                }
                when (frame[0].toInt()) {
                    0 -> {
                        val device = String(frame, 1, 16, Charsets.ISO_8859_1)
                        players.getOrPut(device) { Player().apply { start() } }.offer(frame.copyOfRange(25, frame.size))
                    }
                    1 -> video(ByteBuffer.wrap(frame, 17, 8).long, frame.copyOfRange(25, frame.size))
                    2 -> state(wire.parseToJsonElement(String(frame, 1, frame.size - 1, Charsets.UTF_8)).jsonObject)
                    3 -> engine.callEvent(String(frame, 1, frame.size - 1, Charsets.UTF_8))
                    4 -> {
                        val ended = wire.parseToJsonElement(String(frame, 1, frame.size - 1, Charsets.UTF_8)).jsonObject
                        val reason = ended["reason"]?.jsonPrimitive?.content ?: ""
                        status.value = "Closed: $reason"
                        closed(reason)
                        engine.send(Command.LeaveCall(chat))
                        return
                    }
                }
            }
        } finally {
            running = false
            releaseDecoder()
        }
    }

    private fun state(shown: JsonObject) {
        val state = shown["state"]?.jsonObject ?: return
        val on = state["video"]?.jsonPrimitive?.boolean ?: false
        val width = state["width"]?.jsonPrimitive?.int ?: remoteSize.width
        val height = state["height"]?.jsonPrimitive?.int ?: remoteSize.height
        val turned = state["rotation"]?.jsonPrimitive?.int ?: 0
        remoteSize = Size(width, height)
        remoteRotation = turned
        remoteAspect.value = if (turned % 180 == 0) width.toFloat() / height else height.toFloat() / width
        remoteVideo.value = on
    }

    /** Decodes an access unit straight onto the screen. A new decoder waits for a keyframe. */
    private fun video(
        ts: Long,
        bytes: ByteArray,
    ) {
        val surface = remoteSurface
        if (surface == null) {
            releaseDecoder()
            return
        }
        try {
            if (decoder == null || decoderRotation != remoteRotation) {
                if (!keyframe(bytes)) {
                    return
                }
                releaseDecoder()
                val format =
                    MediaFormat.createVideoFormat(MediaFormat.MIMETYPE_VIDEO_AVC, remoteSize.width, remoteSize.height).apply {
                        setInteger(MediaFormat.KEY_ROTATION, remoteRotation)
                    }
                decoder =
                    MediaCodec.createDecoderByType(MediaFormat.MIMETYPE_VIDEO_AVC).apply {
                        configure(format, surface, null, 0)
                        start()
                    }
                decoderRotation = remoteRotation
            }
            val codec = decoder ?: return
            val input = codec.dequeueInputBuffer(10_000)
            if (input < 0) {
                return
            }
            codec.getInputBuffer(input)?.apply {
                clear()
                put(bytes)
            }
            codec.queueInputBuffer(input, 0, bytes.size, ts, 0)
        } catch (_: Exception) {
            releaseDecoder()
        }
    }

    private fun drainDecoder() {
        val codec = decoder ?: return
        val info = MediaCodec.BufferInfo()
        try {
            while (true) {
                val output = codec.dequeueOutputBuffer(info, 0)
                if (output == MediaCodec.INFO_OUTPUT_FORMAT_CHANGED) {
                    continue
                }
                if (output < 0) {
                    break
                }
                codec.releaseOutputBuffer(output, true)
            }
        } catch (_: Exception) {
            releaseDecoder()
        }
    }

    private fun releaseDecoder() {
        val codec = decoder ?: return
        decoder = null
        decoderRotation = -1
        try {
            codec.stop()
        } catch (_: IllegalStateException) {
            // Already broken.
        }
        codec.release()
    }

    /** Whether an Annex B access unit holds an IDR slice or an SPS. */
    private fun keyframe(bytes: ByteArray): Boolean {
        var at = 0
        while (at + 3 < bytes.size) {
            if (bytes[at].toInt() == 0 && bytes[at + 1].toInt() == 0 && bytes[at + 2].toInt() == 1) {
                val type = bytes[at + 3].toInt() and 0x1f
                if (type == 5 || type == 7) {
                    return true
                }
                at += 3
            } else {
                at += 1
            }
        }
        return false
    }

    /**
     * One sender's voice: Opus packets wait in a jitter buffer that starts at 60 ms and grows by
     * 20 ms, to at most 200 ms, each time it runs dry, then play through the speaker.
     */
    private inner class Player : Thread("keryx-call-play") {
        private val packets = ArrayDeque<ByteArray>()
        private val lock = Object()
        private var depth = JITTER_START

        fun offer(packet: ByteArray) =
            synchronized(lock) {
                packets.addLast(packet)
                while (packets.size > depth + 5) {
                    packets.removeFirst()
                }
                lock.notifyAll()
            }

        override fun run() {
            val format =
                MediaFormat.createAudioFormat(MediaFormat.MIMETYPE_AUDIO_OPUS, RATE, 1).apply {
                    setByteBuffer("csd-0", opusHead())
                    setByteBuffer("csd-1", nanoseconds(312L * 1_000_000_000 / RATE))
                    setByteBuffer("csd-2", nanoseconds(80_000_000))
                }
            val opus = MediaCodec.createDecoderByType(MediaFormat.MIMETYPE_AUDIO_OPUS)
            opus.configure(format, null, null, 0)
            opus.start()
            val least = AudioTrack.getMinBufferSize(RATE, AudioFormat.CHANNEL_OUT_MONO, AudioFormat.ENCODING_PCM_16BIT)
            val track =
                AudioTrack
                    .Builder()
                    .setAudioAttributes(
                        AudioAttributes
                            .Builder()
                            .setUsage(AudioAttributes.USAGE_VOICE_COMMUNICATION)
                            .setContentType(AudioAttributes.CONTENT_TYPE_SPEECH)
                            .build(),
                    ).setAudioFormat(
                        AudioFormat
                            .Builder()
                            .setEncoding(AudioFormat.ENCODING_PCM_16BIT)
                            .setSampleRate(RATE)
                            .setChannelMask(AudioFormat.CHANNEL_OUT_MONO)
                            .build(),
                    ).setBufferSizeInBytes(maxOf(least, FRAME_BYTES * 4))
                    .setTransferMode(AudioTrack.MODE_STREAM)
                    .build()
            track.play()
            val info = MediaCodec.BufferInfo()
            var filling = true
            try {
                while (running) {
                    val packet =
                        synchronized(lock) {
                            if (filling) {
                                while (running && packets.size < depth) {
                                    lock.wait(100)
                                }
                                filling = false
                            }
                            if (packets.isEmpty()) {
                                lock.wait(25)
                            }
                            packets.removeFirstOrNull()
                        }
                    if (packet == null) {
                        depth = minOf(depth + 1, JITTER_MOST)
                        filling = true
                        continue
                    }
                    val input = opus.dequeueInputBuffer(10_000)
                    if (input >= 0) {
                        opus.getInputBuffer(input)?.apply {
                            clear()
                            put(packet)
                        }
                        opus.queueInputBuffer(input, 0, packet.size, 0, 0)
                    }
                    while (true) {
                        val output = opus.dequeueOutputBuffer(info, 5_000)
                        if (output < 0) {
                            break
                        }
                        val pcm = ByteArray(info.size)
                        opus.getOutputBuffer(output)?.apply {
                            position(info.offset)
                            get(pcm)
                        }
                        opus.releaseOutputBuffer(output, false)
                        track.write(pcm, 0, pcm.size)
                    }
                }
            } catch (_: InterruptedException) {
                // The call ended.
            } finally {
                track.stop()
                track.release()
                opus.stop()
                opus.release()
            }
        }
    }
}

/** The Opus header every receiver builds for itself: mono, 48 kHz, 312 samples of pre-skip. */
private fun opusHead(): ByteBuffer =
    ByteBuffer.allocate(19).order(ByteOrder.LITTLE_ENDIAN).apply {
        put("OpusHead".toByteArray(Charsets.US_ASCII))
        put(1)
        put(1)
        putShort(312)
        putInt(RATE)
        putShort(0)
        put(0)
        flip()
    }

private fun nanoseconds(value: Long): ByteBuffer =
    ByteBuffer.allocate(8).order(ByteOrder.LITTLE_ENDIAN).apply {
        putLong(value)
        flip()
    }

/** The call screen: their video, my preview in a corner, the call's text, mute, video and hang up. */
@Composable
fun CallScreen(
    session: CallSession,
    video: (Boolean) -> Unit,
    say: (String) -> Unit,
    hangUp: () -> Unit,
) {
    val text = remember { mutableStateOf("") }
    Box(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background)) {
        if (session.remoteVideo.value) {
            AndroidView(
                { SurfaceView(it).apply { holder.addCallback(session.remoteHolder) } },
                Modifier.align(Alignment.Center).fillMaxWidth().aspectRatio(session.remoteAspect.value),
            )
        } else {
            Text("No video from them", Modifier.align(Alignment.Center))
        }
        if (session.videoOn.value) {
            AndroidView(
                {
                    SurfaceView(it).apply {
                        setZOrderMediaOverlay(true)
                        holder.addCallback(session.previewHolder)
                    }
                },
                Modifier.align(Alignment.TopEnd).safeDrawingPadding().padding(12.dp).size(96.dp, 170.dp),
            )
        }
        Column(
            Modifier.align(Alignment.BottomCenter).safeDrawingPadding().padding(12.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Text(session.status.value)
            for (line in session.texts.takeLast(6)) {
                Text(line, style = MaterialTheme.typography.bodySmall)
            }
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedTextField(text.value, { text.value = it }, Modifier.weight(1f), label = { Text("Text in the call") })
                Button({
                    say(text.value)
                    text.value = ""
                }) { Text("Send") }
            }
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Button({ session.mute(!session.muted.value) }) { Text(if (session.muted.value) "Unmute" else "Mute") }
                Button({ video(!session.videoOn.value) }) { Text(if (session.videoOn.value) "Video off" else "Video on") }
                Button(hangUp) { Text("Hang up") }
            }
        }
    }
}
