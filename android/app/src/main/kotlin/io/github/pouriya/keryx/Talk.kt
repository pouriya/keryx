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
    /** A new device's notice, shown as a line across the chat, with what it says. */
    val notice: String? = null,
    /** An answer message's answer. */
    val answer: Answer? = null,
    /** A suggestion message's action. Its text is `text`. */
    val suggestion: Suggestion? = null,
) {
    val key: String get() = if (seq != null) "s$seq" else "l$local"

    /** This device holds no key for it: nothing opened, and it is not a call or a file. */
    val locked: Boolean get() =
        text == null && files.isEmpty() && call == null && kind != CONTENT_CALL &&
            sticker == null && question == null && answer == null && suggestion == null
}

/** One reaction in the open chat: confirmed (a sequence id) or still waiting (a local id), on a message by either. */
internal data class Reaction(
    val seq: Long?,
    val local: Long?,
    val on: Long?,
    val onLocal: Long?,
    val emoji: String,
    val mine: Boolean,
)

/** One ack in the open chat, folded into its suggestion's bubble: confirmed (a sequence id) or still waiting (a local id). */
internal data class AckMark(
    val seq: Long?,
    val local: Long?,
    val on: Long,
    val accepted: Boolean,
    val mine: Boolean,
    val account: String?,
)

/** One chip under a message: an exact emoji, how many reactions carry it, and this account's newest of them. */
internal data class Chip(
    val emoji: String,
    val count: Int,
    val mine: Reaction?,
)

internal const val CONTENT_TEXT = 0

/** A message kind: the server's notice of a new device, in the self chat. */
internal const val DEVICE_NOTICE = 4

internal const val CONTENT_REACTION = 2
internal const val CONTENT_CALL = 8
internal const val CONTENT_QUESTION = 10
internal const val CONTENT_ANSWER = 11
internal const val CONTENT_STICKER = 12
internal const val CONTENT_SUGGESTION = 13
internal const val CONTENT_ACK = 14

/** The open chat's messages, oldest first, kept up to date from core's events. */
internal class Talk {
    val bubbles = mutableStateListOf<Bubble>()
    val selected = mutableStateListOf<String>()

    /** The open chat's reactions, added up into each message's chips. */
    val reactions = mutableStateListOf<Reaction>()

    /** This account's reactions with a delete waiting, by key, hidden until the delete lands or fails. */
    private val unreacting = mutableStateMapOf<String, Long>()

    /** The open chat's acks, each shown in its suggestion's bubble and never as a bubble of its own. */
    val acks = mutableStateListOf<AckMark>()

    /** The suggestion being accepted, by sequence id, until its ack is pending or core refuses it. */
    var accepting by mutableStateOf<Long?>(null)
        private set

    /** Why accepting a suggestion was refused, by its sequence id: core's code, until Accept or Decline is tapped again. */
    val refusals = mutableStateMapOf<Long, String>()

    /** Messages removed for everyone, so a reply to one can say so. */
    val gone = mutableStateMapOf<Long, Boolean>()
    var chat by mutableStateOf<String?>(null)
        private set
    var hasOlder by mutableStateOf(false)
        private set

    /** A page has come since the chat opened, so `hasOlder` says something. */
    var paged by mutableStateOf(false)
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
        reactions.clear()
        unreacting.clear()
        acks.clear()
        accepting = null
        refusals.clear()
        selected.clear()
        gone.clear()
        hasOlder = false
        paged = false
        seeking = null
        restored = drafts[chat]
    }

    fun close() {
        chat = null
        restored = null
        bubbles.clear()
        reactions.clear()
        unreacting.clear()
        acks.clear()
        accepting = null
        refusals.clear()
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

    /**
     * A message's chips, one per exact emoji in the order they first came, leaving out reactions with a
     * delete waiting. A chip counts reactions, not people, since one person may repeat an emoji.
     */
    fun chips(bubble: Bubble): List<Chip> {
        val on =
            reactions.filter {
                (bubble.seq != null && it.on == bubble.seq || bubble.local != null && it.onLocal == bubble.local) &&
                    reactionKey(it) !in unreacting
            }
        if (on.isEmpty()) return emptyList()
        return on.groupBy { it.emoji }.map { (emoji, list) -> Chip(emoji, list.size, list.lastOrNull { it.mine }) }
    }

    /** This account's newest ack on the suggestion at `seq`, waiting or confirmed. */
    fun ackOf(seq: Long): AckMark? = acks.lastOrNull { it.on == seq && it.mine }

    /** Another account's newest ack on the suggestion at `seq`. */
    fun theirAck(seq: Long): AckMark? = acks.lastOrNull { it.on == seq && !it.mine }

    /** Accept or Decline was tapped: any earlier refusal goes, and Accept turns while it saves. */
    fun answering(
        seq: Long,
        accepting: Boolean,
    ) {
        refusals.remove(seq)
        if (accepting) this.accepting = seq
    }

    /** Core refused something: when a suggestion was being accepted, that is why, shown under its buttons. */
    fun refused(code: String) {
        val seq = accepting ?: return
        accepting = null
        refusals[seq] = code
    }

    private fun acked(shown: Shown) {
        val ack = shown.ack ?: return
        if (gone[shown.seq] == true || acks.any { it.seq == shown.seq }) return
        acks.add(AckMark(shown.seq, null, ack.suggestion, ack.accepted, shown.mine, shown.account))
    }

    private fun reactionKey(reaction: Reaction): String = if (reaction.seq != null) "s${reaction.seq}" else "l${reaction.local}"

    private fun reacted(shown: Shown) {
        val emoji = shown.emoji ?: return
        val on = shown.on ?: return
        if (gone[shown.seq] == true || reactions.any { it.seq == shown.seq }) return
        reactions.add(Reaction(shown.seq, null, on, null, emoji, shown.mine))
    }

    fun message(shown: Shown) {
        if (shown.chat != chat) return
        val kind = shown.content_kind ?: return
        if (kind == CONTENT_REACTION) return reacted(shown)
        if (kind == CONTENT_ACK) return acked(shown)
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
                suggestion = shown.suggestion,
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

    /** A new device's notice, as a line in the self chat, chosen by the author; shown again when its device's name is known. */
    fun notice(
        event: Event.Notice,
        text: String,
    ) {
        if (event.chat != chat) return
        val bubble =
            Bubble(
                seq = event.seq,
                local = null,
                mine = false,
                account = null,
                kind = null,
                text = null,
                at = event.saved_at,
                notice = text,
            )
        val at = bubbles.indexOfFirst { it.seq == event.seq }
        if (at >= 0) {
            bubbles[at] = bubble
            return
        }
        var index = bubbles.indexOfFirst { it.seq == null || it.seq > event.seq }
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
            val target = event.target
            val reaction =
                reactions.firstOrNull {
                    target != null && (it.seq != null && it.seq == target.seq || it.local != null && it.local == target.local)
                }
            if (event.kind == KIND_DELETE && reaction != null) unreacting[reactionKey(reaction)] = event.local
            return
        }
        if (event.kind != KIND_CONTENT) return
        val ack = event.ack
        if (ack != null) {
            acks.add(AckMark(null, event.local, ack.suggestion, ack.accepted, true, null))
            refusals.remove(ack.suggestion)
            if (accepting == ack.suggestion) accepting = null
            return
        }
        val emoji = event.emoji
        if (emoji != null) {
            reactions.add(Reaction(null, event.local, event.target?.seq, event.target?.local, emoji, true))
            return
        }
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
        val waiting = reactions.indexOfFirst { it.local == event.local }
        if (waiting >= 0) {
            val reaction = reactions.removeAt(waiting)
            val key = "l${event.local}"
            unreacting.remove(key)?.let { unreacting["s${event.seq}"] = it }
            if (reactions.none { it.seq == event.seq } && gone[event.seq] != true) {
                reactions.add(waiting, reaction.copy(seq = event.seq, local = null))
            }
        }
        for (i in reactions.indices) {
            val one = reactions[i]
            if (one.onLocal == event.local) reactions[i] = one.copy(on = event.seq, onLocal = null)
        }
        val acked = acks.indexOfFirst { it.local == event.local }
        if (acked >= 0) {
            val ack = acks.removeAt(acked)
            if (acks.none { it.seq == event.seq } && gone[event.seq] != true) acks.add(acked, ack.copy(seq = event.seq, local = null))
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
        unreacting.entries.removeAll { it.value == event.local }
        reactions.removeAll { it.local == event.local }
        acks.removeAll { it.local == event.local }
        val at = bubbles.indexOfFirst { it.local == event.local }
        if (at >= 0) bubbles[at] = bubbles[at].copy(failed = true)
    }

    fun edited(event: Event.Edited) {
        if (event.message.chat != chat) return
        val at = bubbles.indexOfFirst { it.seq == event.target_seq }
        if (at >= 0) {
            val message = event.message
            bubbles[at] = bubbles[at].copy(text = message.text, answer = message.answer ?: bubbles[at].answer, edited = true)
        }
    }

    /** A message of [from] was removed, by a delete or as an empty row from the server. */
    fun deleted(
        from: String,
        target: Long,
    ) {
        if (from == chat) removed(target)
    }

    fun removed(seq: Long) {
        gone[seq] = true
        reactions.removeAll { it.seq == seq }
        unreacting.remove("s$seq")
        acks.removeAll { it.seq == seq }
        bubbles.removeAll { it.seq == seq }
        selected.remove("s$seq")
    }

    fun page(hasOlder: Boolean) {
        this.hasOlder = hasOlder
        paged = true
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
