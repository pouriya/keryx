// One account's archive: every message and call row as the server sent it, in its own SQLite file,
// unencrypted because the rows are already ciphertext. Core hands each page here and waits for the
// confirm before asking for the next. Unread is counted in SQL from the seen positions, per topic and
// per thread, and this account's own devices. See android/AGENTS.md.

package io.github.pouriya.keryx

import android.database.sqlite.SQLiteDatabase
import android.database.sqlite.SQLiteStatement
import java.io.File

class Archive private constructor(
    private val db: SQLiteDatabase,
) {
    fun close() = db.close()

    /** One transaction. A removed message replaces its row, wipes the edits pointing at it and drops that call's rows. */
    fun save(
        messages: List<MessageFrame>,
        calls: List<CallRow>,
        events: List<ChatEvent> = emptyList(),
    ) {
        db.beginTransaction()
        try {
            for (message in messages) {
                val frame = wire.encodeToString(MessageFrame.serializer(), message)
                if (message.kind == null) {
                    db.delete(
                        "call_row",
                        "chat = ? AND call_seq = ?",
                        arrayOf(message.chat, message.seq.toString()),
                    )
                    val edits = ArrayList<Long>()
                    db
                        .query(
                            "message",
                            arrayOf("seq"),
                            "chat = ? AND kind = $EDIT AND pointer = ?",
                            arrayOf(message.chat, message.seq.toString()),
                            null,
                            null,
                            null,
                        ).use { cursor ->
                            while (cursor.moveToNext()) edits.add(cursor.getLong(0))
                        }
                    for (seq in edits) {
                        val wiped = MessageFrame(chat = message.chat, seq = seq, server_saved_at = message.server_saved_at)
                        insert(REPLACE_MESSAGE, wiped, wire.encodeToString(MessageFrame.serializer(), wiped))
                    }
                }
                insert(if (message.kind == null) REPLACE_MESSAGE else KEEP_MESSAGE, message, frame)
            }
            for (event in calls) {
                insertCall(event, wire.encodeToString(CallRow.serializer(), event))
            }
            db.compileStatement(KEEP_EVENT).use { statement ->
                for (event in events) {
                    statement.bindString(1, event.chat)
                    statement.bindLong(2, event.n)
                    statement.bindLong(3, event.audience.toLong())
                    statement.bindLong(4, event.server_at)
                    statement.bindString(5, wire.encodeToString(ChatEvent.serializer(), event))
                    statement.executeInsert()
                    statement.clearBindings()
                }
            }
            db.setTransactionSuccessful()
        } finally {
            db.endTransaction()
        }
    }

    /**
     * Newest `limit` messages below `before`, oldest first, with the call rows among them, and the
     * chat's events from the first message's server time, or from the start when it is the chat's
     * first, up to the message at `before`'s.
     */
    fun read(
        chat: String,
        before: Long?,
        limit: Int,
    ): Triple<List<MessageFrame>, List<CallRow>, List<ChatEvent>> {
        val where = if (before == null) "chat = ?" else "chat = ? AND seq < ?"
        val args = if (before == null) arrayOf(chat) else arrayOf(chat, before.toString())
        val messages = ArrayList<MessageFrame>()
        db.query("message", arrayOf("frame"), where, args, null, null, "seq DESC", limit.toString()).use { cursor ->
            while (cursor.moveToNext()) {
                messages.add(wire.decodeFromString(MessageFrame.serializer(), cursor.getString(0)))
            }
        }
        messages.reverse()
        if (messages.isEmpty()) {
            return Triple(emptyList(), emptyList(), emptyList())
        }
        val marks = messages.joinToString(",") { "?" }
        val callArgs = arrayOf(chat, *messages.map { it.seq.toString() }.toTypedArray())
        val calls = ArrayList<CallRow>()
        db
            .rawQuery(
                "SELECT frame FROM call_row WHERE chat = ? AND call_seq IN ($marks) ORDER BY call_seq, n",
                callArgs,
            ).use { cursor ->
                while (cursor.moveToNext()) {
                    calls.add(wire.decodeFromString(CallRow.serializer(), cursor.getString(0)))
                }
            }
        val from = if (messages.first().seq <= 1) Long.MIN_VALUE else messages.first().server_saved_at ?: Long.MIN_VALUE
        val upTo =
            before?.let { seq ->
                db
                    .query("message", arrayOf("frame"), "chat = ? AND seq = ?", arrayOf(chat, seq.toString()), null, null, null)
                    .use { cursor ->
                        if (!cursor.moveToNext()) return@use null
                        wire.decodeFromString(MessageFrame.serializer(), cursor.getString(0)).server_saved_at
                    }
            } ?: Long.MAX_VALUE
        val events = ArrayList<ChatEvent>()
        db
            .rawQuery(
                "SELECT frame FROM chat_event WHERE chat = ? AND server_at >= ? AND server_at <= ? ORDER BY n",
                arrayOf(chat, from.toString(), upTo.toString()),
            ).use { cursor ->
                while (cursor.moveToNext()) {
                    events.add(wire.decodeFromString(ChatEvent.serializer(), cursor.getString(0)))
                }
            }
        return Triple(messages, calls, events)
    }

    /** A group's or channel's kept history, newest first: what owners and moderators read. */
    fun history(chat: String): List<ChatEvent> {
        val events = ArrayList<ChatEvent>()
        db
            .rawQuery(
                "SELECT frame FROM chat_event WHERE chat = ? AND audience = 1 ORDER BY n DESC",
                arrayOf(chat),
            ).use { cursor ->
                while (cursor.moveToNext()) {
                    events.add(wire.decodeFromString(ChatEvent.serializer(), cursor.getString(0)))
                }
            }
        return events
    }

    /** A message mentioning this account, recorded when core opens it, since mentions are inside the encryption. */
    fun mentioned(
        chat: String,
        seq: Long,
    ) {
        db.execSQL("INSERT OR IGNORE INTO mention (chat, seq) VALUES (?, ?)", arrayOf<Any>(chat, seq))
    }

    /** Whether a group or channel shows topics, which decides which seen position its main list counts from. */
    fun topicsOn(
        chat: String,
        on: Boolean,
    ) {
        if (on) {
            db.execSQL("INSERT OR IGNORE INTO topics_on (chat) VALUES (?)", arrayOf<Any>(chat))
        } else {
            db.delete("topics_on", "chat = ?", arrayOf(chat))
        }
    }

    /** Up to `limit` messages of every chat below `before`, newest by server time first, then chat and seq. */
    fun scan(
        before: ScanAt?,
        limit: Int,
    ): List<MessageFrame> {
        val where = if (before == null) null else "(saved_at, chat, seq) < (?, ?, ?)"
        val args = if (before == null) null else arrayOf(before.saved_at.toString(), before.chat, before.seq.toString())
        val messages = ArrayList<MessageFrame>()
        db.query("message", arrayOf("frame"), where, args, null, null, "saved_at DESC, chat DESC, seq DESC", limit.toString()).use {
                cursor ->
            while (cursor.moveToNext()) {
                messages.add(wire.decodeFromString(MessageFrame.serializer(), cursor.getString(0)))
            }
        }
        return messages
    }

    /** How many rows the archive holds, for a search's progress. */
    fun count(): Long = android.database.DatabaseUtils.queryNumEntries(db, "message")

    /**
     * This device's own seen, in the main list, a topic or a thread, kept until the next chat list
     * says where the server has it. Never goes back.
     */
    fun seenUpTo(
        chat: String,
        seq: Long,
        topic: String?,
        thread: Long?,
    ) {
        db.execSQL(
            "INSERT INTO seen (chat, topic, thread, seen_seq) VALUES (?, ?, ?, ?) " +
                "ON CONFLICT(chat, topic, thread) DO UPDATE SET seen_seq = MAX(seen_seq, excluded.seen_seq)",
            arrayOf<Any>(chat, topic.orEmpty(), thread ?: 0L, seq),
        )
    }

    /** This member's seen positions and which chats show topics, from the chat list. Missing means nothing has been seen. */
    fun seen(chats: List<ChatItem>) {
        db.beginTransaction()
        try {
            db.delete("topics_on", null, null)
            db.compileStatement(UPSERT_SEEN).use { statement ->
                fun put(
                    chat: String,
                    topic: String,
                    thread: Long,
                    seq: Long,
                ) {
                    statement.bindString(1, chat)
                    statement.bindString(2, topic)
                    statement.bindLong(3, thread)
                    statement.bindLong(4, seq)
                    statement.executeInsert()
                    statement.clearBindings()
                }
                for (chat in chats) {
                    put(chat.chat, "", 0, chat.seen_seq ?: 0)
                    for (place in chat.place_seen_list) {
                        put(chat.chat, place.topic.orEmpty(), place.thread_seq ?: 0, place.seen_seq)
                    }
                    if (chat.topics_on) {
                        db.execSQL("INSERT OR IGNORE INTO topics_on (chat) VALUES (?)", arrayOf<Any>(chat.chat))
                    }
                }
            }
            db.setTransactionSuccessful()
        } finally {
            db.endTransaction()
        }
    }

    /** The whole list known so far, replacing the previous one. */
    fun devices(ids: List<String>) {
        db.beginTransaction()
        try {
            db.delete("own_device", null, null)
            db.compileStatement("INSERT INTO own_device (device) VALUES (?)").use { statement ->
                for (id in ids) {
                    statement.bindString(1, id)
                    statement.executeInsert()
                    statement.clearBindings()
                }
            }
            db.setTransactionSuccessful()
        } finally {
            db.endTransaction()
        }
    }

    /** The call's end, when this archive has it: 0 missed, 1 answered. */
    fun callOutcome(
        chat: String,
        seq: Long,
    ): Int? {
        db.rawQuery(
            "SELECT outcome FROM call_row WHERE chat = ? AND call_seq = ? AND kind = 4 ORDER BY n DESC LIMIT 1",
            arrayOf(chat, seq.toString()),
        ).use { cursor ->
            if (!cursor.moveToFirst() || cursor.isNull(0)) return null
            return cursor.getInt(0)
        }
    }

    /**
     * Content and lines after the seen position, not reactions, not calls, not acks, and not this
     * account's own. A chat's count is its main list's, every topic's added up; a thread's replies
     * count only on the message that opened it.
     */
    fun unread(): Unread {
        val chats = LinkedHashMap<String, Int>()
        val mentions = LinkedHashMap<String, Int>()
        val topics = LinkedHashMap<Pair<String, String>, Int>()
        val topicMentions = LinkedHashMap<Pair<String, String>, Int>()
        db.rawQuery(UNREAD, null).use { cursor ->
            while (cursor.moveToNext()) {
                val chat = cursor.getString(0)
                val place = cursor.getString(1)
                val count = cursor.getInt(2)
                val mentioned = cursor.getInt(3)
                chats[chat] = (chats[chat] ?: 0) + count
                if (mentioned > 0) mentions[chat] = (mentions[chat] ?: 0) + mentioned
                topics[chat to place] = count
                if (mentioned > 0) topicMentions[chat to place] = mentioned
            }
        }
        val threads = LinkedHashMap<Pair<String, Long>, Int>()
        val replies = LinkedHashMap<Pair<String, Long>, Int>()
        db.rawQuery(THREADS, null).use { cursor ->
            while (cursor.moveToNext()) {
                val key = cursor.getString(0) to cursor.getLong(1)
                replies[key] = cursor.getInt(2)
                val fresh = cursor.getInt(3)
                if (fresh > 0) threads[key] = fresh
            }
        }
        return Unread(chats, mentions, topics, topicMentions, threads, replies)
    }

    private fun insert(
        sql: String,
        message: MessageFrame,
        frame: String,
    ) {
        db.compileStatement(sql).use { statement ->
            statement.bindString(1, message.chat)
            statement.bindLong(2, message.seq)
            bind(statement, 3, message.kind)
            bind(statement, 4, message.content_kind)
            bind(statement, 5, message.device)
            bind(statement, 6, message.pointer_seq)
            statement.bindLong(7, message.server_saved_at ?: 0)
            statement.bindString(8, frame)
            bind(statement, 9, message.topic)
            bind(statement, 10, message.thread_seq)
            statement.executeInsert()
        }
    }

    private fun insertCall(
        event: CallRow,
        frame: String,
    ) {
        db.compileStatement(KEEP_CALL).use { statement ->
            statement.bindString(1, event.chat)
            statement.bindLong(2, event.call_seq)
            statement.bindLong(3, event.n)
            statement.bindLong(4, event.kind.toLong())
            bind(statement, 5, event.outcome)
            statement.bindString(6, frame)
            statement.executeInsert()
        }
    }

    companion object {
        private const val CONTENT = 0
        private const val EDIT = 1
        private const val LINE = 5
        private const val REACTION = 2
        private const val CALL = 8
        private const val ACK = 14

        private const val KEEP_MESSAGE =
            "INSERT OR IGNORE INTO message (chat, seq, kind, content_kind, device, pointer, saved_at, frame, topic, thread) " +
                "VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
        private const val REPLACE_MESSAGE =
            "INSERT OR REPLACE INTO message (chat, seq, kind, content_kind, device, pointer, saved_at, frame, topic, thread) " +
                "VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
        private const val KEEP_CALL =
            "INSERT OR IGNORE INTO call_row (chat, call_seq, n, kind, outcome, frame) VALUES (?, ?, ?, ?, ?, ?)"
        private const val KEEP_EVENT =
            "INSERT OR IGNORE INTO chat_event (chat, n, audience, server_at, frame) VALUES (?, ?, ?, ?, ?)"
        private const val UPSERT_SEEN =
            "INSERT INTO seen (chat, topic, thread, seen_seq) VALUES (?, ?, ?, ?) " +
                "ON CONFLICT(chat, topic, thread) DO UPDATE SET seen_seq = excluded.seen_seq"

        /** What counts: content but reactions, calls and acks, and membership lines. */
        private const val COUNTS =
            "((message.kind = $CONTENT AND message.content_kind IS NOT NULL AND message.content_kind != $REACTION " +
                "AND message.content_kind != $CALL AND message.content_kind != $ACK) OR message.kind = $LINE)"
        private const val NOT_OWN = "NOT EXISTS (SELECT 1 FROM own_device WHERE own_device.device = message.device)"

        /** The main list's unread per chat and place: a topic's id, or empty for General and for a chat without topics. */
        private val UNREAD =
            """
            SELECT message.chat, CASE WHEN topics_on.chat IS NULL THEN '' ELSE COALESCE(message.topic, '') END AS place,
                   COUNT(*), COUNT(mention.seq)
            FROM message
            LEFT JOIN topics_on ON topics_on.chat = message.chat
            LEFT JOIN seen ON seen.chat = message.chat AND seen.thread = 0
              AND seen.topic = CASE WHEN topics_on.chat IS NULL THEN '' ELSE COALESCE(message.topic, '') END
            LEFT JOIN mention ON mention.chat = message.chat AND mention.seq = message.seq
            WHERE message.thread IS NULL
              AND $COUNTS
              AND message.seq > COALESCE(seen.seen_seq, 0)
              AND $NOT_OWN
            GROUP BY message.chat, place
            """.trimIndent()

        /** Each thread's replies, and how many of them are new since its own seen position. */
        private val THREADS =
            """
            SELECT message.chat, message.thread, COUNT(*),
                   SUM(CASE WHEN message.seq > COALESCE(seen.seen_seq, 0) AND $NOT_OWN THEN 1 ELSE 0 END)
            FROM message
            LEFT JOIN seen ON seen.chat = message.chat AND seen.topic = '' AND seen.thread = message.thread
            WHERE message.thread IS NOT NULL AND $COUNTS
            GROUP BY message.chat, message.thread
            """.trimIndent()

        fun open(file: File): Archive {
            val db = SQLiteDatabase.openOrCreateDatabase(file, null)
            db.execSQL(
                "CREATE TABLE IF NOT EXISTS message (" +
                    "chat TEXT NOT NULL, seq INTEGER NOT NULL, kind INTEGER, content_kind INTEGER, " +
                    "device TEXT, pointer INTEGER, saved_at INTEGER NOT NULL, frame TEXT NOT NULL, topic TEXT, thread INTEGER, " +
                    "PRIMARY KEY (chat, seq))",
            )
            // A search reads every chat newest first by server time.
            db.execSQL("CREATE INDEX IF NOT EXISTS message_saved ON message (saved_at, chat, seq)")
            db.execSQL(
                "CREATE TABLE IF NOT EXISTS call_row (" +
                    "chat TEXT NOT NULL, call_seq INTEGER NOT NULL, n INTEGER NOT NULL, kind INTEGER NOT NULL, " +
                    "outcome INTEGER, frame TEXT NOT NULL, PRIMARY KEY (chat, call_seq, n))",
            )
            // A place is empty for the main list, a topic's id, or a thread's opening seq.
            db.execSQL(
                "CREATE TABLE IF NOT EXISTS seen (chat TEXT NOT NULL, topic TEXT NOT NULL, thread INTEGER NOT NULL, " +
                    "seen_seq INTEGER NOT NULL, PRIMARY KEY (chat, topic, thread))",
            )
            db.execSQL("CREATE TABLE IF NOT EXISTS own_device (device TEXT PRIMARY KEY)")
            db.execSQL("CREATE TABLE IF NOT EXISTS topics_on (chat TEXT PRIMARY KEY)")
            db.execSQL("CREATE TABLE IF NOT EXISTS mention (chat TEXT NOT NULL, seq INTEGER NOT NULL, PRIMARY KEY (chat, seq))")
            db.execSQL(
                "CREATE TABLE IF NOT EXISTS chat_event (chat TEXT NOT NULL, n INTEGER NOT NULL, audience INTEGER NOT NULL, " +
                    "server_at INTEGER NOT NULL, frame TEXT NOT NULL, PRIMARY KEY (chat, n))",
            )
            return Archive(db)
        }

        private fun bind(
            statement: SQLiteStatement,
            index: Int,
            value: Int?,
        ) {
            if (value == null) statement.bindNull(index) else statement.bindLong(index, value.toLong())
        }

        private fun bind(
            statement: SQLiteStatement,
            index: Int,
            value: Long?,
        ) {
            if (value == null) statement.bindNull(index) else statement.bindLong(index, value)
        }

        private fun bind(
            statement: SQLiteStatement,
            index: Int,
            value: String?,
        ) {
            if (value == null) statement.bindNull(index) else statement.bindString(index, value)
        }
    }
}

/**
 * What is unread: per chat, its @ count, per topic (the place empty for General or a chat without
 * topics), and per thread by the seq that opened it, with each thread's replies in all.
 */
data class Unread(
    val chats: Map<String, Int> = emptyMap(),
    val mentions: Map<String, Int> = emptyMap(),
    val topics: Map<Pair<String, String>, Int> = emptyMap(),
    val topicMentions: Map<Pair<String, String>, Int> = emptyMap(),
    val threads: Map<Pair<String, Long>, Int> = emptyMap(),
    val replies: Map<Pair<String, Long>, Int> = emptyMap(),
)
