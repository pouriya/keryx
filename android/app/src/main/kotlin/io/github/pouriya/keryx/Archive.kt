// One account's archive: every message and call row as the server sent it, in its own SQLite file,
// unencrypted because the rows are already ciphertext. Core hands each page here and waits for the
// confirm before asking for the next. Unread is counted in SQL from the seen position and this
// account's own devices. See android/AGENTS.md.

package io.github.pouriya.keryx

import android.database.sqlite.SQLiteDatabase
import android.database.sqlite.SQLiteStatement
import java.io.File

class Archive private constructor(
    private val db: SQLiteDatabase,
) {
    fun close() = db.close()

    /** One transaction. A removed message replaces its row and drops that call's rows. */
    fun save(
        messages: List<MessageFrame>,
        calls: List<CallRow>,
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
                }
                insert(
                    if (message.kind == null) REPLACE_MESSAGE else KEEP_MESSAGE,
                    message.chat,
                    message.seq,
                    message.kind,
                    message.content_kind,
                    message.device,
                    frame,
                )
            }
            for (event in calls) {
                insertCall(event, wire.encodeToString(CallRow.serializer(), event))
            }
            db.setTransactionSuccessful()
        } finally {
            db.endTransaction()
        }
    }

    /** Newest `limit` messages below `before`, oldest first, with the call rows among them. */
    fun read(
        chat: String,
        before: Long?,
        limit: Int,
    ): Pair<List<MessageFrame>, List<CallRow>> {
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
            return emptyList<MessageFrame>() to emptyList()
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
        return messages to calls
    }

    /** This member's seen position, from the chat list. Missing means nothing has been seen. */
    fun seen(chats: List<ChatItem>) {
        db.beginTransaction()
        try {
            db.compileStatement(UPSERT_SEEN).use { statement ->
                for (chat in chats) {
                    statement.bindString(1, chat.chat)
                    statement.bindLong(2, chat.seen_seq ?: 0)
                    statement.executeInsert()
                    statement.clearBindings()
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

    /**
     * Content after the seen position, not reactions and not this account's own. A call counts
     * only when its end row says missed.
     */
    fun unread(): Map<String, Int> {
        val counts = LinkedHashMap<String, Int>()
        db.rawQuery(UNREAD, null).use { cursor ->
            while (cursor.moveToNext()) {
                counts[cursor.getString(0)] = cursor.getInt(1)
            }
        }
        return counts
    }

    private fun insert(
        sql: String,
        chat: String,
        seq: Long,
        kind: Int?,
        contentKind: Int?,
        device: String?,
        frame: String,
    ) {
        db.compileStatement(sql).use { statement ->
            statement.bindString(1, chat)
            statement.bindLong(2, seq)
            bind(statement, 3, kind)
            bind(statement, 4, contentKind)
            bind(statement, 5, device)
            statement.bindString(6, frame)
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
        private const val REACTION = 2
        private const val CALL = 8
        private const val END = 4
        private const val MISSED = 0

        private const val KEEP_MESSAGE =
            "INSERT OR IGNORE INTO message (chat, seq, kind, content_kind, device, frame) VALUES (?, ?, ?, ?, ?, ?)"
        private const val REPLACE_MESSAGE =
            "INSERT OR REPLACE INTO message (chat, seq, kind, content_kind, device, frame) VALUES (?, ?, ?, ?, ?, ?)"
        private const val KEEP_CALL =
            "INSERT OR IGNORE INTO call_row (chat, call_seq, n, kind, outcome, frame) VALUES (?, ?, ?, ?, ?, ?)"
        private const val UPSERT_SEEN =
            "INSERT INTO seen (chat, seen_seq) VALUES (?, ?) " +
                "ON CONFLICT(chat) DO UPDATE SET seen_seq = excluded.seen_seq"

        private val UNREAD =
            """
            SELECT message.chat, COUNT(*)
            FROM message
            LEFT JOIN seen ON seen.chat = message.chat
            WHERE message.kind = $CONTENT
              AND message.content_kind IS NOT NULL
              AND message.content_kind != $REACTION
              AND message.seq > COALESCE(seen.seen_seq, 0)
              AND NOT EXISTS (SELECT 1 FROM own_device WHERE own_device.device = message.device)
              AND (
                message.content_kind != $CALL
                OR EXISTS (
                  SELECT 1 FROM call_row
                  WHERE call_row.chat = message.chat
                    AND call_row.call_seq = message.seq
                    AND call_row.kind = $END
                    AND call_row.outcome = $MISSED
                )
              )
            GROUP BY message.chat
            """.trimIndent()

        fun open(file: File): Archive {
            val db = SQLiteDatabase.openOrCreateDatabase(file, null)
            db.execSQL(
                "CREATE TABLE IF NOT EXISTS message (" +
                    "chat TEXT NOT NULL, seq INTEGER NOT NULL, kind INTEGER, content_kind INTEGER, " +
                    "device TEXT, frame TEXT NOT NULL, PRIMARY KEY (chat, seq))",
            )
            db.execSQL(
                "CREATE TABLE IF NOT EXISTS call_row (" +
                    "chat TEXT NOT NULL, call_seq INTEGER NOT NULL, n INTEGER NOT NULL, kind INTEGER NOT NULL, " +
                    "outcome INTEGER, frame TEXT NOT NULL, PRIMARY KEY (chat, call_seq, n))",
            )
            db.execSQL("CREATE TABLE IF NOT EXISTS seen (chat TEXT PRIMARY KEY, seen_seq INTEGER NOT NULL)")
            db.execSQL("CREATE TABLE IF NOT EXISTS own_device (device TEXT PRIMARY KEY)")
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
            value: String?,
        ) {
            if (value == null) statement.bindNull(index) else statement.bindString(index, value)
        }
    }
}
