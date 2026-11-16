// The words screen. A new account is shown a phrase as pills, then asked for three of those words.
// A later phone types the phrase it already has, or is told that new words cannot replace the old keys.
package io.github.pouriya.keryx

import android.content.Context
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.ime
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawing
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalLayoutDirection
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.LayoutDirection
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import java.security.SecureRandom

private enum class WordsStep { Phrase, Check, Enter, Lost }

private val accent = Color(0xFF4576C4)
private val onAccent = Color(0xFFFFFFFF)

private class WordList(
    private val words: List<String>,
) {
    fun word(text: String) = words.binarySearch(text) >= 0

    fun prefix(text: String): Boolean {
        if (text.isEmpty()) return true
        val start = words.binarySearch(text).let { if (it >= 0) it else -it - 1 }
        return start < words.size && words[start].startsWith(text)
    }

    /** At most five, and only once two letters are in. */
    fun matches(
        prefix: String,
        limit: Int = 5,
    ): List<String> {
        if (prefix.length < 2) return emptyList()
        val start = words.binarySearch(prefix).let { if (it >= 0) it else -it - 1 }
        return words.drop(start).takeWhile { it.startsWith(prefix) }.take(limit)
    }

    companion object {
        fun load(context: Context): WordList {
            val words = context.resources.openRawResource(R.raw.bip39_english).bufferedReader().readLines()
            return WordList(words)
        }
    }
}

@Composable
fun Words(
    server: String,
    username: String,
    phrase: String?,
    restore: Boolean,
    error: String?,
    busy: Boolean,
    onGenerate: (Int) -> Unit,
    onSet: (String) -> Unit,
    onRestore: (String) -> Unit,
    onClearError: () -> Unit,
    onBindBack: (() -> Boolean) -> Unit,
) {
    val ink = ink()
    val context = LocalContext.current
    val list = remember(context) { WordList.load(context) }
    val clipboard = LocalClipboardManager.current
    var step by remember { mutableStateOf(if (restore) WordsStep.Enter else WordsStep.Phrase) }
    var count by remember { mutableIntStateOf(12) }
    var copying by remember { mutableStateOf(false) }
    var answers by remember { mutableStateOf(listOf("", "", "")) }
    val parts = phrase?.split(' ')?.filter { it.isNotEmpty() } ?: emptyList()
    val places = remember(phrase) { if (parts.size >= 3) pickThree(parts.size) else emptyList() }
    val keyboard = WindowInsets.ime.getBottom(LocalDensity.current) > 0
    SideEffect {
        onBindBack {
            when {
                copying -> {
                    copying = false
                    true
                }
                step == WordsStep.Check -> {
                    step = WordsStep.Phrase
                    true
                }
                step == WordsStep.Lost -> {
                    step = if (restore) WordsStep.Enter else WordsStep.Phrase
                    true
                }
                else -> false
            }
        }
    }
    Bars(ink.page, ink.page == Color(0xFF141B24))
    Box(Modifier.fillMaxSize().background(ink.page).imePadding()) {
        Column(
            Modifier
                .fillMaxSize()
                .windowInsetsPadding(WindowInsets.safeDrawing)
                .padding(horizontal = 24.dp, vertical = 8.dp),
        ) {
            when (step) {
                WordsStep.Phrase ->
                    Phrase(
                        ink,
                        parts,
                        count,
                        error,
                        onCount = {
                            if (it != count) {
                                count = it
                                onClearError()
                                onGenerate(it)
                            }
                        },
                        onWrote = {
                            answers = listOf("", "", "")
                            onClearError()
                            step = WordsStep.Check
                        },
                        onCopy = { copying = true },
                    )
                WordsStep.Check ->
                    Check(
                        ink, list, parts, places, answers, error, busy,
                        onAnswer = { at, value ->
                            answers = answers.toMutableList().also { it[at] = value }
                            onClearError()
                        },
                        onContinue = { phrase?.let(onSet) },
                        onBack = { step = WordsStep.Phrase },
                    )
                WordsStep.Enter ->
                    Enter(
                        ink,
                        list,
                        keyboard,
                        error,
                        busy,
                        onRestore,
                        onClearError,
                        onLost = {
                            onClearError()
                            step = WordsStep.Lost
                        },
                    )
                WordsStep.Lost ->
                    Lost(ink, onNew = {
                        count = 12
                        onClearError()
                        onGenerate(12)
                        step = WordsStep.Phrase
                    }, onBack = { step = if (restore) WordsStep.Enter else WordsStep.Phrase })
            }
        }
        if (copying && phrase != null) {
            CopyWarning(ink, onCopy = {
                clipboard.setText(AnnotatedString(copiedWords(server, username, parts)))
                copying = false
            }, onBack = { copying = false })
        }
    }
}

@Composable
private fun ColumnScope.Phrase(
    ink: Ink,
    parts: List<String>,
    count: Int,
    error: String?,
    onCount: (Int) -> Unit,
    onWrote: () -> Unit,
    onCopy: () -> Unit,
) {
    Text(
        stringResource(R.string.words_title),
        Modifier.fillMaxWidth(),
        color = ink.text,
        fontSize = 22.sp,
        fontWeight = FontWeight.SemiBold,
        textAlign = TextAlign.Center,
    )
    Text(
        stringResource(R.string.words_keep),
        Modifier.padding(top = 8.dp, bottom = 12.dp).fillMaxWidth(),
        color = ink.muted,
        fontSize = 14.sp,
        textAlign = TextAlign.Center,
    )
    Length(ink, count, onCount)
    PillGrid(parts.size) { at, modifier -> Pill(ink, at + 1, parts[at], modifier) }
    if (error != null) Text(error, Modifier.padding(top = 8.dp), color = ink.danger, fontSize = 14.sp)
    Go(stringResource(R.string.wrote_them), parts.isNotEmpty(), onWrote)
    LineButton(stringResource(R.string.copy), parts.isNotEmpty(), onCopy)
}

@Composable
private fun ColumnScope.Check(
    ink: Ink,
    list: WordList,
    parts: List<String>,
    places: List<Int>,
    answers: List<String>,
    error: String?,
    busy: Boolean,
    onAnswer: (Int, String) -> Unit,
    onContinue: () -> Unit,
    onBack: () -> Unit,
) {
    var focus by remember { mutableIntStateOf(0) }
    val requester = remember { FocusRequester() }
    val current = answers.getOrElse(focus) { "" }
    val bad = current.isNotEmpty() && !list.prefix(current)
    val ready = places.indices.all { answers.getOrElse(it) { "" } == parts.getOrNull(places[it]) }
    val filled = places.indices.all { answers.getOrElse(it) { "" }.isNotEmpty() }
    val shown =
        when {
            bad -> stringResource(R.string.wrong_word)
            filled && !ready -> stringResource(R.string.words_mismatch)
            else -> error
        }

    fun move(at: Int) {
        focus = at.coerceIn(0, places.lastIndex.coerceAtLeast(0))
        runCatching { requester.requestFocus() }
    }
    Text(
        stringResource(R.string.words_title),
        Modifier.fillMaxWidth(),
        color = ink.text,
        fontSize = 22.sp,
        fontWeight = FontWeight.SemiBold,
        textAlign = TextAlign.Center,
    )
    if (places.size == 3) {
        Text(
            pluralStringResource(R.plurals.words_check, places.size, places[0] + 1, places[1] + 1, places[2] + 1),
            Modifier.padding(top = 8.dp, bottom = 12.dp).fillMaxWidth(),
            color = ink.muted,
            fontSize = 14.sp,
            textAlign = TextAlign.Center,
        )
    }
    PillGrid(places.size) { at, modifier ->
        val wrong = at == focus && bad
        Pill(
            ink,
            places[at] + 1,
            answers.getOrElse(at) { "" },
            modifier,
            edge =
                if (wrong) {
                    ink.danger
                } else if (at == focus) {
                    accent
                } else {
                    ink.line
                },
            color = if (wrong) ink.danger else ink.text,
            onClick = { move(at) },
        )
    }
    if (places.isNotEmpty()) {
        WordLine(
            ink,
            stringResource(R.string.word_n, places[focus] + 1),
            current,
            bad,
            requester,
            last = focus == places.lastIndex,
            onValue = { raw -> onAnswer(focus, raw.lowercase().filter { it.isLetter() }) },
            onNext = { move(focus + 1) },
            onDone = { if (ready && !busy) onContinue() },
        )
        Matches(ink, list, current) { word ->
            onAnswer(focus, word)
            val next = (focus + 1 until places.size).firstOrNull { answers.getOrElse(it) { "" }.isEmpty() }
            if (next != null) move(next)
        }
    }
    if (shown != null) Text(shown, Modifier.padding(top = 8.dp), color = ink.danger, fontSize = 14.sp)
    Go(stringResource(R.string.continue_word), ready && !busy, onContinue)
    Quiet(stringResource(R.string.back), onBack)
}

@Composable
private fun ColumnScope.Enter(
    ink: Ink,
    list: WordList,
    keyboard: Boolean,
    error: String?,
    busy: Boolean,
    onRestore: (String) -> Unit,
    onClearError: () -> Unit,
    onLost: () -> Unit,
) {
    var count by remember { mutableIntStateOf(12) }
    var boxes by remember { mutableStateOf(List(12) { "" }) }
    var focus by remember { mutableIntStateOf(0) }
    val requester = remember { FocusRequester() }
    val current = boxes.getOrElse(focus) { "" }
    val bad = current.isNotEmpty() && !list.prefix(current)
    val ready = boxes.isNotEmpty() && boxes.all { list.word(it) }
    val shown = if (bad) stringResource(R.string.wrong_word) else error

    fun put(
        next: List<String>,
        at: Int,
        move: Boolean,
    ) {
        boxes = next
        focus = at.coerceIn(0, next.lastIndex)
        onClearError()
        if (move) runCatching { requester.requestFocus() }
    }
    Text(
        stringResource(R.string.words_title),
        Modifier.fillMaxWidth(),
        color = ink.text,
        fontSize = if (keyboard) 20.sp else 22.sp,
        fontWeight = FontWeight.SemiBold,
        textAlign = TextAlign.Center,
    )
    Text(
        stringResource(R.string.restore_intro),
        Modifier.padding(top = 4.dp, bottom = 8.dp).fillMaxWidth(),
        color = ink.muted,
        fontSize = if (keyboard) 13.sp else 14.sp,
        textAlign = TextAlign.Center,
    )
    Length(ink, count) { value ->
        if (value != count) {
            count = value
            put(List(value) { boxes.getOrElse(it) { "" } }, focus.coerceAtMost(value - 1), false)
        }
    }
    PillGrid(boxes.size) { at, modifier ->
        val wrong = at == focus && bad
        Pill(
            ink,
            at + 1,
            boxes[at],
            modifier,
            edge =
                if (wrong) {
                    ink.danger
                } else if (at == focus) {
                    accent
                } else {
                    ink.line
                },
            color = if (wrong) ink.danger else ink.text,
            onClick = { put(boxes, at, true) },
        )
    }
    WordLine(
        ink,
        stringResource(R.string.word_n, focus + 1),
        current,
        bad,
        requester,
        last = focus == boxes.lastIndex,
        onValue = { raw ->
            val tokens = raw.lowercase().split(Regex("\\s+")).filter { it.isNotEmpty() && it.all { letter -> letter.isLetter() } }
            if (raw.any { it.isWhitespace() } && tokens.size > 1) {
                val next = boxes.toMutableList()
                tokens.forEachIndexed { offset, word ->
                    val at = focus + offset
                    if (at < next.size) next[at] = word
                }
                val land = (focus until next.size).firstOrNull { next[it].isEmpty() || !list.word(next[it]) } ?: focus
                put(next, land, true)
            } else {
                put(boxes.toMutableList().also { it[focus] = tokens.firstOrNull() ?: "" }, focus, false)
            }
        },
        onNext = { if (focus < boxes.lastIndex) put(boxes, focus + 1, true) },
        onDone = { if (ready && !busy) onRestore(boxes.joinToString(" ")) },
    )
    Matches(ink, list, current) { word ->
        val next = boxes.toMutableList()
        next[focus] = word
        val land = (focus + 1 until next.size).firstOrNull { next[it].isEmpty() } ?: focus
        put(next, land, true)
    }
    if (shown != null) Text(shown, Modifier.padding(top = 8.dp), color = ink.danger, fontSize = 14.sp)
    if (!keyboard) {
        Go(stringResource(R.string.continue_word), ready && !busy, { onRestore(boxes.joinToString(" ")) })
        Quiet(stringResource(R.string.lost_words), onLost)
    }
}

@Composable
private fun ColumnScope.Lost(
    ink: Ink,
    onNew: () -> Unit,
    onBack: () -> Unit,
) {
    Text(
        stringResource(R.string.lost_title),
        Modifier.fillMaxWidth().padding(top = 8.dp),
        color = ink.text,
        fontSize = 22.sp,
        fontWeight = FontWeight.SemiBold,
        textAlign = TextAlign.Center,
    )
    Text(stringResource(R.string.lost_body), Modifier.padding(top = 16.dp).weight(1f), color = ink.text, fontSize = 16.sp)
    Go(stringResource(R.string.lost_go), true, onNew)
    Quiet(stringResource(R.string.back), onBack)
}

@Composable
private fun CopyWarning(
    ink: Ink,
    onCopy: () -> Unit,
    onBack: () -> Unit,
) {
    Box(
        Modifier
            .fillMaxSize()
            .background(Color(0x73141B24))
            .clickable(interactionSource = remember { MutableInteractionSource() }, indication = null) {},
        contentAlignment = Alignment.Center,
    ) {
        Column(
            Modifier
                .padding(22.dp)
                .border(2.dp, ink.text, RoundedCornerShape(16.dp))
                .background(ink.page, RoundedCornerShape(16.dp))
                .padding(20.dp),
        ) {
            Text(stringResource(R.string.copy_warning), color = ink.text, fontSize = 16.sp)
            Go(stringResource(R.string.copy), true, onCopy, ink.danger)
            Quiet(stringResource(R.string.back), onBack)
        }
    }
}

@Composable
private fun Length(
    ink: Ink,
    count: Int,
    onCount: (Int) -> Unit,
) {
    Row(Modifier.fillMaxWidth().padding(bottom = 8.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        listOf(12, 24).forEach { value ->
            val on = value == count
            Text(
                pluralStringResource(R.plurals.words_count, value, value),
                Modifier.weight(
                    1f,
                ).clip(
                    RoundedCornerShape(12.dp),
                ).background(
                    if (on) accent else ink.page,
                ).border(1.dp, if (on) accent else ink.line, RoundedCornerShape(12.dp)).clickable(role = Role.Button) {
                    onCount(value)
                }.padding(vertical = 8.dp),
                color = if (on) onAccent else ink.muted,
                fontSize = 14.sp,
                fontWeight = FontWeight.SemiBold,
                textAlign = TextAlign.Center,
            )
        }
    }
}

/**
 * Pills in rows of three, left to right in Persian too, in the middle of the space they are given.
 * Twelve are the size of twenty-four; only when the space is too short do they shrink, to fit.
 */
@Composable
private fun ColumnScope.PillGrid(
    count: Int,
    pill: @Composable (Int, Modifier) -> Unit,
) {
    CompositionLocalProvider(LocalLayoutDirection provides LayoutDirection.Ltr) {
        BoxWithConstraints(Modifier.weight(1f).fillMaxWidth(), contentAlignment = Alignment.Center) {
            val rows = (count + 2) / 3
            val gap = 8.dp
            val height = if (rows == 0) 38.dp else ((maxHeight - gap * (rows - 1)) / rows).coerceIn(24.dp, 38.dp)
            Column(verticalArrangement = Arrangement.spacedBy(gap)) {
                (0 until count).chunked(3).forEach { row ->
                    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(gap)) {
                        row.forEach { at -> pill(at, Modifier.weight(1f).height(height)) }
                        repeat(3 - row.size) { Box(Modifier.weight(1f)) }
                    }
                }
            }
        }
    }
}

/** An outlined pill with its number in a small accent circle before the word, which may be empty. */
@Composable
private fun Pill(
    ink: Ink,
    number: Int,
    word: String,
    modifier: Modifier,
    edge: Color = ink.line,
    color: Color = ink.text,
    onClick: (() -> Unit)? = null,
) {
    val shape = RoundedCornerShape(50)
    Row(
        modifier
            .clip(shape)
            .border(1.dp, edge, shape)
            .then(if (onClick != null) Modifier.clickable(role = Role.Button, onClick = onClick) else Modifier)
            .padding(start = 4.dp, end = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(5.dp),
    ) {
        Box(Modifier.size(22.dp).background(accent, CircleShape), contentAlignment = Alignment.Center) {
            Text("$number", color = onAccent, fontSize = 10.5.sp, lineHeight = 12.sp, fontWeight = FontWeight.Bold)
        }
        Text(
            word,
            color = color,
            fontSize = if (word.length > 7) 13.sp else 14.sp,
            fontWeight = FontWeight.SemiBold,
            maxLines = 1,
            softWrap = false,
        )
    }
}

/** The one line a word is typed into, under the pills, naming which word it is. */
@Composable
private fun WordLine(
    ink: Ink,
    label: String,
    value: String,
    bad: Boolean,
    requester: FocusRequester,
    last: Boolean,
    onValue: (String) -> Unit,
    onNext: () -> Unit,
    onDone: () -> Unit,
) {
    Text(label, color = ink.muted, fontSize = 13.sp)
    BasicTextField(
        value = value,
        onValueChange = onValue,
        modifier =
            Modifier
                .fillMaxWidth()
                .padding(top = 6.dp)
                .focusRequester(requester)
                .border(1.dp, if (bad) ink.danger else accent, RoundedCornerShape(12.dp))
                .padding(horizontal = 14.dp, vertical = 12.dp),
        textStyle = TextStyle(color = ink.text, fontSize = 16.sp),
        singleLine = true,
        keyboardOptions =
            KeyboardOptions(
                capitalization = KeyboardCapitalization.None,
                autoCorrectEnabled = false,
                keyboardType = KeyboardType.Ascii,
                imeAction = if (last) ImeAction.Done else ImeAction.Next,
            ),
        keyboardActions = KeyboardActions(onNext = { onNext() }, onDone = { onDone() }),
        cursorBrush = SolidColor(accent),
    )
}

/** At most five words starting with what is typed, once two letters are in. */
@OptIn(androidx.compose.foundation.layout.ExperimentalLayoutApi::class)
@Composable
private fun Matches(
    ink: Ink,
    list: WordList,
    value: String,
    onPick: (String) -> Unit,
) {
    val found = list.matches(value)
    if (found.isEmpty() || list.word(value)) return
    FlowRow(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        found.forEach { word ->
            Text(
                word,
                Modifier
                    .padding(top = 8.dp)
                    .clip(RoundedCornerShape(999.dp))
                    .border(1.dp, ink.line, RoundedCornerShape(999.dp))
                    .clickable { onPick(word) }
                    .padding(horizontal = 10.dp, vertical = 4.dp),
                color = accent,
                fontSize = 14.sp,
            )
        }
    }
}

@Composable
private fun Go(
    label: String,
    enabled: Boolean,
    onClick: () -> Unit,
    color: Color = accent,
) {
    Box(
        Modifier
            .padding(top = 8.dp)
            .fillMaxWidth()
            .heightIn(min = 52.dp)
            .clip(RoundedCornerShape(14.dp))
            .background(if (enabled) color else color.copy(alpha = 0.45f))
            .clickable(enabled = enabled, role = Role.Button, onClick = onClick)
            .padding(horizontal = 12.dp, vertical = 12.dp),
        contentAlignment = Alignment.Center,
    ) {
        Text(label, color = onAccent, fontSize = 16.sp, fontWeight = FontWeight.SemiBold, textAlign = TextAlign.Center)
    }
}

@Composable
private fun LineButton(
    label: String,
    enabled: Boolean,
    onClick: () -> Unit,
) {
    Box(
        Modifier.padding(
            top = 8.dp,
        ).fillMaxWidth().height(
            52.dp,
        ).clip(
            RoundedCornerShape(14.dp),
        ).border(1.dp, accent, RoundedCornerShape(14.dp)).clickable(enabled = enabled, role = Role.Button, onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        Text(label, color = accent, fontSize = 16.sp, fontWeight = FontWeight.SemiBold)
    }
}

@Composable
private fun Quiet(
    label: String,
    onClick: () -> Unit,
) {
    Text(
        label,
        Modifier.fillMaxWidth().clickable(role = Role.Button, onClick = onClick).padding(vertical = 12.dp),
        color = accent,
        fontSize = 16.sp,
        fontWeight = FontWeight.SemiBold,
        textAlign = TextAlign.Center,
    )
}

/** The server, the username, then `1 word` on each line. */
private fun copiedWords(
    server: String,
    username: String,
    words: List<String>,
): String =
    buildString {
        append(server)
        append('\n')
        append(username)
        words.forEachIndexed { index, word ->
            append('\n')
            append(index + 1)
            append(' ')
            append(word)
        }
    }

private fun pickThree(length: Int): List<Int> {
    val bag = (0 until length).toMutableList()
    val random = SecureRandom()
    for (index in bag.lastIndex downTo 1) {
        val swapAt = random.nextInt(index + 1)
        val swap = bag[swapAt]
        bag[swapAt] = bag[index]
        bag[index] = swap
    }
    return bag.take(3).sorted()
}
