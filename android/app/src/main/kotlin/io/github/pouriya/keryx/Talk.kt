package io.github.pouriya.keryx

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue

/** One message of the open chat, confirmed (it has a sequence id) or still waiting (it has a local id). */
internal data class Bubble(
    val seq: Long?,
    val local: Long?,
    val mine: Boolean,
    val account: String?,
    val kind: Int?,
    val text: String?,
    /** The sequence id of the message this one answers, once known. */
    val reply: Long? = null,
    /** The local id of a still-waiting message this one answers, until that one has a sequence id. */
    val replyLocal: Long? = null,
    val files: List<FileInfo> = emptyList(),
    val call: CallBody? = null,
    val callOutcome: Int? = null,
    val problem: String? = null,
    val at: Long,
    val edited: Boolean = false,
    val failed: Boolean = false,
    /** A sticker message's sticker, by its id on the server. */
    val sticker: String? = null,
    /** A question message's tree. Its intro is `text`. */
    val question: Question? = null,
    /** An answer message's answer. */
    val answer: Answer? = null,
) {
    val key: String get() = if (seq != null) "s$seq" else "l$local"

    /** This device holds no key for it: nothing opened, and it is not a call or a file. */
    val locked: Boolean get() =
        text == null && files.isEmpty() && call == null && kind != CONTENT_CALL &&
            sticker == null && question == null && answer == null
}

internal const val CONTENT_TEXT = 0
internal const val CONTENT_REACTION = 2
internal const val CONTENT_CALL = 8
internal const val CONTENT_QUESTION = 10
internal const val CONTENT_ANSWER = 11
internal const val CONTENT_STICKER = 12

/** The open chat's messages, oldest first, kept up to date from core's events. */
internal class Talk {
    val bubbles = mutableStateListOf<Bubble>()
    val selected = mutableStateListOf<String>()

    /** Messages removed for everyone, so a reply to one can say so. */
    val gone = mutableStateMapOf<Long, Boolean>()
    var chat by mutableStateOf<String?>(null)
        private set
    var hasOlder by mutableStateOf(false)
        private set

    /** A draft the server sent for the open chat, waiting for the screen to take it. */
    var restored by mutableStateOf<DraftText?>(null)
        private set

    /** A reply's original that is being looked for above the loaded page. */
    var seeking by mutableStateOf<Long?>(null)

    fun open(chat: String) {
        this.chat = chat
        actions.clear()
        bubbles.clear()
        selected.clear()
        gone.clear()
        hasOlder = false
        seeking = null
        restored = drafts[chat]
    }

    fun close() {
        chat = null
        restored = null
        bubbles.clear()
        selected.clear()
        gone.clear()
        seeking = null
    }

    /** The newest draft known for each chat, so reopening one brings it back. */
    private val drafts = mutableMapOf<String, DraftText>()

    fun drafted(event: Event.Draft) {
        keep(event.chat, event.draft)
        if (event.chat == chat && event.draft != null) restored = event.draft
    }

    fun keep(
        chat: String,
        draft: DraftText?,
    ) {
        if (draft == null) drafts.remove(chat) else drafts[chat] = draft
    }

    fun taken() {
        restored = null
    }

    fun bySeq(seq: Long): Bubble? = bubbles.firstOrNull { it.seq == seq }

    /** The newest loaded answer to the question at `seq`, by anyone, or only by this account. */
    fun answerTo(
        seq: Long,
        mine: Boolean = false,
    ): Bubble? = bubbles.lastOrNull { it.answer?.question == seq && (!mine || it.mine) }

    fun message(shown: Shown) {
        if (shown.chat != chat) return
        val kind = shown.content_kind ?: return
        if (kind == CONTENT_REACTION) return
        val bubble =
            Bubble(
                seq = shown.seq,
                local = null,
                mine = shown.mine,
                account = shown.account,
                kind = kind,
                text = shown.text,
                reply = shown.reply,
                files = shown.files,
                call = shown.call,
                callOutcome = shown.call_outcome,
                problem = shown.problem,
                at = if (shown.saved_at > 0) shown.saved_at else shown.sent_at,
                sticker = shown.sticker,
                question = shown.question,
                answer = shown.answer,
            )
        val at = bubbles.indexOfFirst { it.seq == shown.seq }
        if (at >= 0) {
            bubbles[at] = bubble.copy(edited = bubbles[at].edited)
            return
        }
        if (gone[shown.seq] == true) return
        var index = bubbles.indexOfFirst { it.seq == null || it.seq > shown.seq }
        if (index < 0) index = bubbles.size
        bubbles.add(index, bubble)
    }

    /** This device's own edits and deletes by local id: the server never echoes them back. */
    private val actions = HashMap<Long, Event.Pending>()
    private val resolved = HashMap<Long, Long>()

    private fun seqOf(pointer: Pointer?): Long? = pointer?.seq ?: pointer?.local?.let { resolved[it] }

    fun pending(event: Event.Pending) {
        if (event.chat != chat) return
        if (event.kind == KIND_EDIT || event.kind == KIND_DELETE) {
            actions[event.local] = event
            return
        }
        if (event.kind != KIND_CONTENT) return
        if (event.text == null && event.sticker == null && event.answer == null) return
        bubbles.add(
            Bubble(
                seq = null,
                local = event.local,
                mine = true,
                account = null,
                kind =
                    when {
                        event.sticker != null -> CONTENT_STICKER
                        event.answer != null -> CONTENT_ANSWER
                        else -> CONTENT_TEXT
                    },
                text = event.text,
                reply = event.target?.seq,
                replyLocal = event.target?.local,
                at = System.currentTimeMillis(),
                sticker = event.sticker,
                answer = event.answer,
            ),
        )
    }

    fun sent(event: Event.Sent) {
        if (event.chat != chat) return
        resolved[event.local] = event.seq
        actions.remove(event.local)?.let { action ->
            val target = seqOf(action.target) ?: return@let
            if (action.kind == KIND_DELETE) {
                removed(target)
            } else {
                val at = bubbles.indexOfFirst { it.seq == target }
                if (at >= 0) {
                    val answer = action.answer
                    bubbles[at] =
                        if (answer != null) {
                            bubbles[at].copy(answer = answer, edited = true)
                        } else {
                            bubbles[at].copy(text = action.text, edited = true)
                        }
                }
            }
        }
        val at = bubbles.indexOfFirst { it.local == event.local }
        if (bubbles.any { it.seq == event.seq }) {
            if (at >= 0) bubbles.removeAt(at)
        } else if (at >= 0) {
            val waiting = bubbles.removeAt(at)
            var index = bubbles.indexOfFirst { it.seq == null || it.seq > event.seq }
            if (index < 0) index = bubbles.size
            bubbles.add(index, waiting.copy(seq = event.seq, local = null, at = event.saved_at))
        }
        for (i in bubbles.indices) {
            val one = bubbles[i]
            if (one.replyLocal == event.local) bubbles[i] = one.copy(reply = event.seq, replyLocal = null)
        }
    }

    fun failed(event: Event.Failed) {
        actions.remove(event.local)
        val at = bubbles.indexOfFirst { it.local == event.local }
        if (at >= 0) bubbles[at] = bubbles[at].copy(failed = true)
    }

    fun edited(event: Event.Edited) {
        val at = bubbles.indexOfFirst { it.seq == event.target_seq }
        if (at >= 0) {
            val message = event.message
            bubbles[at] = bubbles[at].copy(text = message.text, answer = message.answer ?: bubbles[at].answer, edited = true)
        }
    }

    fun deleted(target: Long) = removed(target)

    fun removed(seq: Long) {
        gone[seq] = true
        bubbles.removeAll { it.seq == seq }
        selected.remove("s$seq")
    }

    fun page(hasOlder: Boolean) {
        this.hasOlder = hasOlder
    }

    fun toggle(bubble: Bubble) {
        if (bubble.key in selected) selected.remove(bubble.key) else selected.add(bubble.key)
    }

    fun picked(): List<Bubble> = bubbles.filter { it.key in selected }
}

private const val KIND_CONTENT = 0
private const val KIND_EDIT = 1
private const val KIND_DELETE = 2

/** A link, or a run of 5 digits or more, inside a message's text. */
internal data class Copyable(
    val start: Int,
    val end: Int,
)

private val LINK = Regex("""https?://[^\s]+""", RegexOption.IGNORE_CASE)
private val NUMBER = Regex("""\d{5,}""")

internal fun copyables(text: String): List<Copyable> {
    val found = ArrayList<Copyable>()
    for (match in LINK.findAll(text)) found.add(Copyable(match.range.first, match.range.last + 1))
    for (match in NUMBER.findAll(text)) {
        val start = match.range.first
        if (found.none { start >= it.start && start < it.end }) found.add(Copyable(start, match.range.last + 1))
    }
    return found.sortedBy { it.start }
}

/** A question message's first question, its preview when it has no intro. */
internal fun firstQuestion(question: Question): String = question.nodes.firstOrNull { it.id == question.first }?.text.orEmpty()

internal fun nodeOf(
    question: Question,
    id: String,
): QuestionNode? = question.nodes.firstOrNull { it.id == id }

/** The next question after `item`, by the first rule that matches, as core's `next_of`. Null ends the form. */
internal fun nextOf(
    node: QuestionNode,
    item: AnswerItem,
): String? {
    for (rule in node.next) {
        val picked = rule.picked
        val range = rule.range
        val matched =
            when {
                picked != null -> !item.skipped && picked.any { it in item.picked }
                range != null -> {
                    val number = item.number
                    !item.skipped && number != null && range.size == 2 && range[0] <= number && number <= range[1]
                }
                rule.own != null -> !item.skipped && item.own != null
                rule.skipped != null -> item.skipped
                else -> true
            }
        if (matched) return rule.to
    }
    return null
}

/** One question's answer while the sheet is open: what is picked, typed or written in. */
internal data class Draft(
    val picked: List<String> = emptyList(),
    val own: String = "",
    val ownOn: Boolean = false,
    val number: String = "",
) {
    companion object {
        fun of(item: AnswerItem) =
            Draft(
                picked = item.picked,
                own = item.own.orEmpty(),
                ownOn = item.own != null,
                number = item.number?.let(::plain).orEmpty(),
            )
    }
}

/** Why a typed number cannot be sent, or null when it can. */
internal enum class NumberProblem { Empty, NotNumber, Range, Step }

internal fun numberProblem(
    range: NumberRange,
    typed: String,
): NumberProblem? {
    if (typed.isBlank()) return NumberProblem.Empty
    val value = readNumber(typed) ?: return NumberProblem.NotNumber
    if (value < range.min || value > range.max) return NumberProblem.Range
    val steps = (value - range.min) / (range.step ?: 1.0)
    if (kotlin.math.abs(steps - kotlin.math.round(steps)) > 1e-9) return NumberProblem.Step
    return null
}

/** A typed number, with Persian and Arabic digits and decimal marks read too. */
internal fun readNumber(typed: String): Double? {
    val ascii = StringBuilder()
    for (char in typed.trim()) {
        ascii.append(
            when (char) {
                in '۰'..'۹' -> '0' + (char - '۰')
                in '٠'..'٩' -> '0' + (char - '٠')
                '٫', ',' -> '.'
                else -> char
            },
        )
    }
    return ascii.toString().toDoubleOrNull()
}

/** A number as people write it: 2, not 2.0. */
internal fun plain(value: Double): String =
    if (value == kotlin.math.floor(value) && kotlin.math.abs(value) < 1e15) value.toLong().toString() else value.toString()

/**
 * What a question's draft answers, or null while it is not a full answer. Write your own replaces the
 * number on a number, replaces the pick on a single choice, and adds to the picks on a multi choice.
 */
internal fun itemOf(
    node: QuestionNode,
    draft: Draft,
): AnswerItem? {
    val own = if (draft.ownOn) draft.own.trim() else ""
    if (draft.ownOn && own.isEmpty()) return null
    val range = node.number
    if (range != null) {
        if (own.isNotEmpty()) return AnswerItem(node.id, own = own)
        if (numberProblem(range, draft.number) != null) return null
        return AnswerItem(node.id, number = readNumber(draft.number))
    }
    if (draft.picked.isEmpty() && own.isEmpty()) return null
    return AnswerItem(node.id, picked = draft.picked, own = own.ifEmpty { null })
}
