// core's own inputs, outputs, commands and events, as they cross JNI in JSON. Kept in step with
// core/src/client.rs and core/src/protocol.rs by hand, as android/AGENTS.md chose. Numbers that are
// u64 or i64 in Rust are Long here, and kinds and modes are the schema's numbers.

package io.github.pouriya.keryx

import kotlinx.serialization.KSerializer
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.builtins.ListSerializer
import kotlinx.serialization.builtins.serializer
import kotlinx.serialization.descriptors.SerialDescriptor
import kotlinx.serialization.descriptors.buildClassSerialDescriptor
import kotlinx.serialization.descriptors.element
import kotlinx.serialization.encoding.Decoder
import kotlinx.serialization.encoding.Encoder
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonDecoder
import kotlinx.serialization.json.JsonEncoder
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.long
import kotlinx.serialization.json.put

val wire =
    Json {
        classDiscriminator = "type"
        ignoreUnknownKeys = true
        encodeDefaults = true
    }

/** Bytes as core's `serde_bytes` writes them in JSON: an array of numbers from 0 to 255. */
object Octets : KSerializer<ByteArray> {
    private val list = ListSerializer(Int.serializer())
    override val descriptor: SerialDescriptor = list.descriptor

    override fun serialize(
        encoder: Encoder,
        value: ByteArray,
    ) = encoder.encodeSerializableValue(list, value.map { it.toInt() and 0xff })

    override fun deserialize(decoder: Decoder): ByteArray = decoder.decodeSerializableValue(list).map { it.toByte() }.toByteArray()
}

@Serializable
data class Platform(
    val platform: Long,
    val os: String,
    val os_version: String,
    val web_platform: String? = null,
    val web_platform_version: String? = null,
)

@Serializable
data class CaptchaAnswer(
    val id: String,
    val answer: String,
)

/** A stored message by its sequence id, or one of this client's still waiting by its local id. */
@Serializable(with = PointerShape::class)
data class Pointer(
    val seq: Long? = null,
    val local: Long? = null,
)

/** Core's pointer is a Rust enum, so its JSON holds exactly one of the two keys. */
object PointerShape : KSerializer<Pointer> {
    override val descriptor: SerialDescriptor =
        buildClassSerialDescriptor("Pointer") {
            element<Long?>("seq")
            element<Long?>("local")
        }

    override fun serialize(
        encoder: Encoder,
        value: Pointer,
    ) = (encoder as JsonEncoder).encodeJsonElement(
        buildJsonObject {
            value.seq?.let { put("seq", it) }
            value.local?.let { put("local", it) }
        },
    )

    override fun deserialize(decoder: Decoder): Pointer {
        val shape = (decoder as JsonDecoder).decodeJsonElement().jsonObject
        return Pointer(shape["seq"]?.jsonPrimitive?.long, shape["local"]?.jsonPrimitive?.long)
    }
}

@Serializable
sealed interface Command {
    @Serializable
    @SerialName("sign_up")
    data class SignUp(
        val username: String,
        val password: String,
        val display_name: String? = null,
        val captcha: CaptchaAnswer? = null,
    ) : Command

    @Serializable
    @SerialName("log_in")
    data class LogIn(
        val username: String,
        val password: String,
        val captcha: CaptchaAnswer? = null,
    ) : Command

    @Serializable
    @SerialName("captcha")
    data object Captcha : Command

    @Serializable
    @SerialName("setup")
    data class Setup(
        val token: String,
        val password: String,
        val captcha: CaptchaAnswer? = null,
    ) : Command

    @Serializable
    @SerialName("generate_words")
    data class GenerateWords(
        val count: Int,
    ) : Command

    @Serializable
    @SerialName("set_words")
    data class SetWords(
        val words: String,
    ) : Command

    @Serializable
    @SerialName("restore")
    data class Restore(
        val words: String,
    ) : Command

    @Serializable
    @SerialName("chat_list")
    data object ChatList : Command

    @Serializable
    @SerialName("open_chat")
    data class OpenChat(
        val chat: String,
    ) : Command

    @Serializable
    @SerialName("load_older")
    data class LoadOlder(
        val chat: String,
    ) : Command

    @Serializable
    @SerialName("start_chat")
    data class StartChat(
        val account: String,
    ) : Command

    @Serializable
    @SerialName("search")
    data class Search(
        val text: String,
    ) : Command

    @Serializable
    @SerialName("send_text")
    data class SendText(
        val chat: String,
        val text: String,
    ) : Command

    /** One file, or up to 20 images and videos as a bundle, with an optional caption. */
    @Serializable
    @SerialName("send_files")
    data class SendFiles(
        val chat: String,
        val files: List<FileSpec>,
        val text: String? = null,
    ) : Command

    /** Fetches a shown file into `handle`. The ffi puts the file's key back in. */
    @Serializable
    @SerialName("download")
    data class Download(
        val file: FileInfo,
        val handle: Long,
    ) : Command

    @Serializable
    @SerialName("edit")
    data class Edit(
        val chat: String,
        val target: Pointer,
        val text: String,
    ) : Command

    @Serializable
    @SerialName("delete")
    data class Delete(
        val chat: String,
        val target: Pointer,
    ) : Command

    @Serializable
    @SerialName("seen")
    data class Seen(
        val chat: String,
        val up_to_seq: Long,
    ) : Command

    @Serializable
    @SerialName("typing")
    data class Typing(
        val chat: String,
        val on: Boolean,
    ) : Command

    @Serializable
    @SerialName("devices")
    data object Devices : Command

    @Serializable
    @SerialName("terminate")
    data class Terminate(
        val device: String,
        val words: String,
    ) : Command

    @Serializable
    @SerialName("log_out")
    data object LogOut : Command

    @Serializable
    @SerialName("network_back")
    data object NetworkBack : Command

    @Serializable
    @SerialName("start_call")
    data class StartCall(
        val chat: String,
        val video: Boolean,
        val voice: Boolean,
    ) : Command

    @Serializable
    @SerialName("decline_call")
    data class DeclineCall(
        val chat: String,
        val call_seq: Long,
    ) : Command

    @Serializable
    @SerialName("join_call")
    data class JoinCall(
        val chat: String,
        val call_seq: Long,
    ) : Command

    @Serializable
    @SerialName("leave_call")
    data class LeaveCall(
        val chat: String,
    ) : Command

    @Serializable
    @SerialName("call_text")
    data class CallText(
        val chat: String,
        val call_seq: Long,
        val text: String,
    ) : Command
}

/** A picture's small pixels, from which core makes its BlurHash. */
@Serializable
data class Thumbnail(
    val width: Int,
    val height: Int,
    @Serializable(with = Octets::class) val rgba: ByteArray,
)

/** One file to send, its bytes read from `handle` as core asks. Kinds: 3 image, 4 video, 5 doc, 6 file. */
@Serializable
data class FileSpec(
    val handle: Long,
    val kind: Int,
    val name: String,
    @SerialName("type") val mime: String,
    val size: Long,
    val width: Int? = null,
    val height: Int? = null,
    val thumbnail: Thumbnail? = null,
    val description: String? = null,
    val title: String? = null,
    val artist: String? = null,
    val duration: Long? = null,
)

/** A file inside a shown message. Its key stays in the ffi, as android/AGENTS.md chose. */
@Serializable
data class FileInfo(
    val id: String,
    val kind: Int,
    val name: String,
    @SerialName("type") val mime: String,
    val size: Long,
    val chunks: Long,
    val width: Int? = null,
    val height: Int? = null,
    val blurhash: String? = null,
    val description: String? = null,
    val title: String? = null,
    val artist: String? = null,
    val duration: Long? = null,
)

@Serializable
sealed interface Input {
    @Serializable
    @SerialName("command")
    data class Run(
        val command: Command,
    ) : Input

    @Serializable
    @SerialName("loaded")
    data class Loaded(
        @Serializable(with = Octets::class) val first: ByteArray?,
        @Serializable(with = Octets::class) val second: ByteArray?,
        val resume: Boolean,
        val token: String?,
    ) : Input

    @Serializable
    @SerialName("saved")
    data class Saved(
        val id: Long,
    ) : Input

    @Serializable
    @SerialName("save_failed")
    data class SaveFailed(
        val id: Long,
    ) : Input

    @Serializable
    @SerialName("connected")
    data object Connected : Input

    @Serializable
    @SerialName("closed")
    data class Closed(
        val code: Int,
        val reason: String,
    ) : Input

    @Serializable
    @SerialName("frame")
    data class Frame(
        val text: String,
    ) : Input

    @Serializable
    @SerialName("reply")
    data class Reply(
        val id: Long,
        val status: Int,
        val body: String,
    ) : Input

    @Serializable
    @SerialName("timer")
    data object Timer : Input

    /** A frame from the joined call's `events` track, as `Core.callNext` handed it over. */
    @Serializable
    @SerialName("call_event")
    data class CallEvent(
        val event: JsonObject,
    ) : Input

    /** `save_rows` was written, all of it. */
    @Serializable
    @SerialName("rows_saved")
    data class RowsSaved(
        val id: Long,
    ) : Input

    /** `save_rows` was not written. Nothing of it stayed. */
    @Serializable
    @SerialName("rows_save_failed")
    data class RowsSaveFailed(
        val id: Long,
    ) : Input

    /** `read_rows`'s answer: messages oldest first, and the call rows among them. */
    @Serializable
    @SerialName("rows_read")
    data class RowsRead(
        val id: Long,
        val message_list: List<MessageFrame>,
        val call_list: List<CallRow>,
    ) : Input
}

@Serializable
sealed interface Output {
    @Serializable
    @SerialName("send")
    data class Send(
        val text: String,
    ) : Output

    @Serializable
    @SerialName("request")
    data class Request(
        val id: Long,
        val method: String,
        val path: String,
        val body: String? = null,
        val bearer: String? = null,
    ) : Output

    @Serializable
    @SerialName("connect")
    data class Connect(
        val bearer: String? = null,
    ) : Output

    @Serializable
    @SerialName("disconnect")
    data object Disconnect : Output

    @Serializable
    @SerialName("save")
    data class Save(
        val id: Long,
        val slot: Int,
        @Serializable(with = Octets::class) val bytes: ByteArray,
    ) : Output

    @Serializable
    @SerialName("wipe")
    data object Wipe : Output

    @Serializable
    @SerialName("wake_at")
    data class WakeAt(
        val at: Long,
    ) : Output

    @Serializable
    @SerialName("event")
    data class Show(
        val event: Event,
    ) : Output

    /** Connect with `Core.callConnect` on the call's own thread: the key stays in Rust. */
    @Serializable
    @SerialName("join_call")
    data class JoinCall(
        val chat: String,
        val call_seq: Long,
    ) : Output

    @Serializable
    @SerialName("leave_call")
    data class LeaveCall(
        val chat: String,
    ) : Output

    /** Read these bytes of `handle`, and answer with `chunk_read` through `Core.stepBytes`. */
    @Serializable
    @SerialName("read_chunk")
    data class ReadChunk(
        val handle: Long,
        val n: Long,
        val offset: Long,
        val length: Long,
    ) : Output

    /** A PUT of the bytes `Core.chunk` hands over, answered with `reply`. */
    @Serializable
    @SerialName("put_chunk")
    data class PutChunk(
        val id: Long,
        val path: String,
        val bearer: String? = null,
        val chunk: Long,
    ) : Output

    /** A GET of raw bytes, answered with `fetched` through `Core.stepBytes`. */
    @Serializable
    @SerialName("get_chunk")
    data class GetChunk(
        val id: Long,
        val path: String,
        val bearer: String? = null,
    ) : Output

    /** Write the opened bytes `Core.chunk` hands over at `offset` of `handle`. */
    @Serializable
    @SerialName("write_chunk")
    data class WriteChunk(
        val handle: Long,
        val n: Long,
        val offset: Long,
        val chunk: Long,
    ) : Output

    /** Keep these rows, then answer with `rows_saved` or `rows_save_failed`. */
    @Serializable
    @SerialName("save_rows")
    data class SaveRows(
        val id: Long,
        val message_list: List<MessageFrame>,
        val call_list: List<CallRow>,
    ) : Output

    /** Read up to `limit` newest messages below `before_seq`, and answer with `rows_read`. */
    @Serializable
    @SerialName("read_rows")
    data class ReadRows(
        val id: Long,
        val chat: String,
        val before_seq: Long? = null,
        val limit: Int,
    ) : Output
}

@Serializable
data class AccountBrief(
    val id: String,
    val username: String,
    val display_name: String? = null,
)

@Serializable
data class MarkItem(
    val account: String,
    val received_seq: Long,
    val seen_seq: Long? = null,
)

@Serializable
data class MessageFrame(
    val chat: String,
    val seq: Long,
    val device: String? = null,
    val kind: Int? = null,
    val content_kind: Int? = null,
    val encrypted: Boolean? = null,
    val epoch: Long? = null,
    val pointer_seq: Long? = null,
    val client_sent_at: Long? = null,
    val server_saved_at: Long? = null,
    val body: String? = null,
    val signature: String? = null,
)

/** One row of a call, as `sync` and `save_rows` carry it. Kinds: 0 join, 1 leave, 2 decline, 3 text, 4 end. */
@Serializable
data class CallRow(
    val chat: String,
    val call_seq: Long,
    val n: Long,
    val kind: Int,
    val device: String,
    val server_at: Long,
    val client_at: Long? = null,
    val epoch: Long? = null,
    val signature: String? = null,
    val body: String? = null,
    val outcome: Int? = null,
)

@Serializable
data class ChatItem(
    val chat: String,
    val type: Int,
    val mode: Int,
    val last_seq: Long? = null,
    val last_kind: Int? = null,
    val last_at: Long? = null,
    val last_content_seq: Long? = null,
    val epoch: Long? = null,
    val last_content: MessageFrame? = null,
    val mark_list: List<MarkItem> = emptyList(),
    val seen_seq: Long? = null,
)

@Serializable
data class Shown(
    val chat: String,
    val seq: Long,
    val account: String? = null,
    val mine: Boolean,
    val device: String? = null,
    val content_kind: Int? = null,
    val encrypted: Boolean,
    val sent_at: Long,
    val saved_at: Long,
    val text: String? = null,
    val files: List<FileInfo> = emptyList(),
    val call: CallBody? = null,
    val call_outcome: Int? = null,
    val call_ended_at: Long? = null,
    val problem: String? = null,
)

@Serializable
data class CallBody(
    val video: Boolean,
    val voice: Boolean,
)

/** Something that happened inside a call. Kinds: 0 join, 1 leave, 2 decline, 3 text, 4 end. */
@Serializable
data class ShownCallEvent(
    val chat: String,
    val call_seq: Long,
    val n: Long,
    val kind: Int,
    val device: String,
    val account: String? = null,
    val mine: Boolean,
    val at: Long,
    val sent_at: Long? = null,
    val outcome: Int? = null,
    val text: String? = null,
    val problem: String? = null,
)

@Serializable
data class DeviceLogItem(
    val online_at: Long,
    val offline_at: Long? = null,
    val ip: String,
)

/** One device, whole on a registered device, or reduced to its platform and last active day. */
@Serializable
data class DeviceItem(
    val id: String,
    val platform: Long,
    val os: String? = null,
    val os_version: String? = null,
    val web_platform: String? = null,
    val web_platform_version: String? = null,
    val first_ip: String? = null,
    val last_ip: String? = null,
    val created_at: Long? = null,
    val last_active_at: Long? = null,
    val terminated_at: Long? = null,
    val log_list: List<DeviceLogItem> = emptyList(),
    val last_active_day: Long? = null,
)

@Serializable
data class DevicesReply(
    val device_list: List<DeviceItem>,
)

@Serializable
sealed interface Event {
    @Serializable
    @SerialName("words")
    data class Words(
        val words: String,
    ) : Event

    @Serializable
    @SerialName("need_login")
    data object NeedLogin : Event

    @Serializable
    @SerialName("features")
    data class Features(
        val registration: Boolean,
        val captcha: Boolean,
        val name: String? = null,
    ) : Event

    @Serializable
    @SerialName("captcha")
    data class Captcha(
        val id: String,
        val image: String,
    ) : Event

    @Serializable
    @SerialName("registered")
    data class Registered(
        val username: String,
    ) : Event

    @Serializable
    @SerialName("need_words")
    data class NeedWords(
        val restore: Boolean,
    ) : Event

    @Serializable
    @SerialName("signed_in")
    data class SignedIn(
        val account: String,
        val username: String,
        val device: String? = null,
    ) : Event

    @Serializable
    @SerialName("token")
    data class Token(
        val token: String? = null,
    ) : Event

    @Serializable
    @SerialName("device_limit")
    data class DeviceLimit(
        val message: String,
    ) : Event

    @Serializable
    @SerialName("account")
    data class Account(
        val account: AccountBrief,
    ) : Event

    @Serializable
    @SerialName("chats")
    data class Chats(
        val chat_list: List<ChatItem>,
    ) : Event

    /** This account's devices so far, so unread can leave out its own messages. */
    @Serializable
    @SerialName("own_devices")
    data class OwnDevices(
        val device_list: List<String>,
    ) : Event

    @Serializable
    @SerialName("message")
    data class Message(
        val message: Shown,
    ) : Event

    @Serializable
    @SerialName("edited")
    data class Edited(
        val message: Shown,
        val target_seq: Long,
    ) : Event

    @Serializable
    @SerialName("deleted")
    data class Deleted(
        val chat: String,
        val seq: Long,
        val target_seq: Long,
    ) : Event

    @Serializable
    @SerialName("removed")
    data class Removed(
        val chat: String,
        val seq: Long,
    ) : Event

    @Serializable
    @SerialName("notice")
    data class Notice(
        val chat: String,
        val seq: Long,
        val device: String,
        val saved_at: Long,
        val problem: String? = null,
    ) : Event

    @Serializable
    @SerialName("page_loaded")
    data class PageLoaded(
        val chat: String,
        val has_older: Boolean,
    ) : Event

    @Serializable
    @SerialName("pending")
    data class Pending(
        val chat: String,
        val local: Long,
        val kind: Int,
        val target: Pointer? = null,
        val text: String? = null,
        val handles: List<Long> = emptyList(),
    ) : Event

    @Serializable
    @SerialName("sent")
    data class Sent(
        val chat: String,
        val local: Long,
        val seq: Long,
        val saved_at: Long,
    ) : Event

    @Serializable
    @SerialName("failed")
    data class Failed(
        val chat: String,
        val local: Long,
        val code: String,
    ) : Event

    @Serializable
    @SerialName("marks")
    data class Marks(
        val chat: String,
        val mark: MarkItem,
    ) : Event

    @Serializable
    @SerialName("typing")
    data class Typing(
        val chat: String,
        val account: String,
        val on: Boolean,
    ) : Event

    @Serializable
    @SerialName("started")
    data class Started(
        val chat: String,
        val account: String,
    ) : Event

    @Serializable
    @SerialName("chat_ready")
    data class ChatReady(
        val chat: String,
    ) : Event

    @Serializable
    @SerialName("chat_left")
    data class ChatLeft(
        val chat: String,
    ) : Event

    @Serializable
    @SerialName("search_results")
    data class SearchResults(
        val account_list: List<AccountBrief>,
    ) : Event

    @Serializable
    @SerialName("device_list")
    data class DeviceList(
        val reply: DevicesReply,
    ) : Event

    @Serializable
    @SerialName("terminated")
    data class Terminated(
        val device: String,
    ) : Event

    @Serializable
    @SerialName("online")
    data object Online : Event

    @Serializable
    @SerialName("offline")
    data object Offline : Event

    @Serializable
    @SerialName("logged_out")
    data object LoggedOut : Event

    @Serializable
    @SerialName("call_event")
    data class CallEvent(
        val event: ShownCallEvent,
    ) : Event

    /** `done` of `total` chunks of `handle`, going up or coming down. */
    @Serializable
    @SerialName("progress")
    data class Progress(
        val handle: Long,
        val done: Long,
        val total: Long,
    ) : Event

    @Serializable
    @SerialName("downloaded")
    data class Downloaded(
        val handle: Long,
    ) : Event

    /** An upload or download stopped for good. An upload's message is not sent. */
    @Serializable
    @SerialName("transfer_failed")
    data class TransferFailed(
        val handle: Long,
        val code: String,
    ) : Event

    @Serializable
    @SerialName("error")
    data class Error(
        val code: String,
        val message: String,
    ) : Event
}
