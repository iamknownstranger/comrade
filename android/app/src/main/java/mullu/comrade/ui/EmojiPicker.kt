package mullu.comrade.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.ScrollableTabRow
import androidx.compose.material3.Tab
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.TextRange
import androidx.compose.ui.text.input.TextFieldValue
import androidx.compose.ui.unit.dp
import mullu.comrade.ui.theme.ComradeRadii
import mullu.comrade.ui.theme.GlassElevation
import mullu.comrade.ui.theme.glassSurface

/**
 * A small, dependency-free emoji picker for the composer's left-hand button.
 *
 * Deliberately hand-rolled: the alternative is an emoji-keyboard library that
 * ships its own asset font — megabytes, and a second glyph set that disagrees
 * with the platform's, so what you pick is not what the person you send it to
 * sees. What the composer needs is a grid of characters that inserts one at the
 * cursor, which needs no dependency at all.
 *
 * Port of `app/lib/src/widgets/emoji_picker.dart`; the groups and their order
 * are the same on both frontends.
 *
 * The set is curated, not exhaustive: the emoji people actually reach for in a
 * messenger, grouped the way keyboards group them. A search field over 3,600
 * Unicode names would need that name table, which is exactly the asset this
 * avoids.
 */
data class EmojiGroup(val label: String, val emoji: List<String>)

/**
 * Space-separated emoji to a list. The format keeps the table readable — a list
 * of 400 single-character literals is one emoji per line once the formatter has
 * had it, which is unreviewable. No emoji here contains a space, so the split
 * is lossless, including the ZWJ sequences.
 */
private fun split(emoji: String): List<String> = emoji.split(" ")

/** The groups, in keyboard order. Kept in step with the Dart table. */
val EmojiGroups: List<EmojiGroup> = listOf(
    EmojiGroup(
        "Smileys",
        split(
            "😀 😃 😄 😁 😆 😅 🤣 😂 🙂 🙃 😉 😊 😇 🥰 😍 🤩 😘 😗 😚 😙 😋 😛 😜 🤪 😝 🤗 🤭 🤫 🤔 🤐 😐 😑 😶 😏 😒 🙄 " +
                "😬 😮‍💨 🤥 😌 😔 😪 😴 😷 🤒 🤕 🥱 😵 🤯 🥳 😎 🤓 🧐 😕 😟 🙁 😯 😦 😧 😢 😥 😭 😱 😖 😣 😞 😓 😩 🥺 😤 😠 " +
                "😡 🤬 💀 💩 🤡 👻 👽 🤖 😺",
        ),
    ),
    EmojiGroup(
        "People",
        split(
            "👋 🤚 ✋ 🖖 👌 🤌 ✌️ 🤞 🤟 🤘 👈 👉 👆 👇 ☝️ 👍 👎 ✊ 👊 🤛 🤜 👏 🙌 👐 🤲 🤝 🙏 ✍️ 💪 🦾 👀 👁️ 👄 🧠 " +
                "🫀 👶 🧑 👩 👨 🧓 🙋 🤷 🤦 💁 🙆 🙅 🧏 🚶 🏃 🧘 👫 👬 👭 🧑‍🤝‍🧑 👪 🫂 💃 🕺 👑 🎓",
        ),
    ),
    EmojiGroup(
        "Hearts",
        split("❤️ 🧡 💛 💚 💙 💜 🤎 🖤 🤍 💔 ❣️ 💕 💞 💓 💗 💖 💘 💝 💟 ♥️ ✨ ⭐ 🌟 💫 💥 🔥 🎉 🎊 🎈 🎁"),
    ),
    EmojiGroup(
        "Nature",
        split(
            "🐶 🐱 🐭 🐹 🐰 🦊 🐻 🐼 🐨 🐯 🦁 🐮 🐷 🐸 🐵 🐔 🐧 🐦 🦆 🦉 🦄 🐝 🦋 🐌 🐞 🐢 🐍 🐙 🐳 🐬 🌵 🌲 🌳 🌴 🌱 🌿 " +
                "☘️ 🍀 🍁 🍂 🌸 💐 🌹 🌻 🌼 🌷 🌙 ☀️ ⛅ 🌈 ❄️ ⚡ 🌊 🏔️ 🌍 🪐 🌞 🌝 🍄 🐚",
        ),
    ),
    EmojiGroup(
        "Food",
        split(
            "🍎 🍐 🍊 🍋 🍌 🍉 🍇 🍓 🫐 🍒 🥭 🍍 🥥 🥝 🍅 🥑 🍆 🥕 🌽 🌶️ 🍞 🥐 🥖 🧇 🧀 🍳 🥘 🍲 🍛 🍜 🍝 🍕 🍔 🌮 🌯 " +
                "🥗 🍿 🧂 🍦 🍩 🍪 🎂 🍰 🧁 🍫 🍬 ☕ 🍵 🧋 🥤",
        ),
    ),
    EmojiGroup(
        "Activity",
        split(
            "⚽ 🏀 🏈 ⚾ 🎾 🏐 🏓 🏸 🥊 ⛳ 🎯 🎮 🕹️ 🎲 ♟️ 🎸 🎹 🎺 🎻 🥁 🎤 🎧 🎬 🎨 📷 📚 ✏️ 🧵 🧩 🏆 🚴 🏊 🧗 ⛺ " +
                "🎿 🛹 🎣 🪄 🪘",
        ),
    ),
    EmojiGroup(
        "Travel",
        split(
            "🚗 🚕 🚌 🏎️ 🚓 🚑 🚒 🛵 🏍️ 🚲 🛺 🚂 ✈️ 🚀 🛸 🚁 ⛵ 🚢 🗺️ 🧭 🏠 🏢 🏥 🏦 🏨 🏫 🕌 ⛩️ 🛕 🌃 🌉 🎡 🎢 " +
                "🏝️ 🏖️ ⛱️ 🧳 🎫 🛎️ 🚏",
        ),
    ),
    EmojiGroup(
        "Objects",
        split(
            "📱 💻 ⌨️ 🖥️ 🖨️ 🔌 🔋 💡 🔍 🔒 🔓 🔑 🗝️ 🔨 🪛 ⚙️ 🧰 🧲 🪜 🧹 📎 📌 📏 ✂️ 🗒️ 📅 📆 🗓️ 📊 📈 💰 💳 " +
                "🧾 ✉️ 📤 📥 📦 📮 🗳️ 📢 ⌛ ⏰ ⏳ 🔔 🔕 🎙️ 🩺 💊 🧪 🔭",
        ),
    ),
    EmojiGroup(
        "Symbols",
        split(
            "✅ ❌ ❗ ❓ ‼️ ⁉️ 💯 🔴 🟠 🟡 🟢 🔵 🟣 ⚫ ⚪ 🟥 🟧 🟨 🟩 🟦 ➕ ➖ ✖️ ➗ ♻️ ⚠️ 🚫 🔞 📵 🔇 🔊 🎵 🎶 " +
                "➡️ ⬅️ ⬆️ ⬇️ 🔄 🔝 🆗 ☮️ ☯️ 🕉️ 🔱 ⚛️ 🈶 🅰️ 🆒 🆕 🆓",
        ),
    ),
)

/**
 * Insert [emoji] at the caret (replacing any selection) and leave the caret
 * after it, so picking three in a row reads as typing three in a row.
 *
 * Pure, so `EmojiPickerTest` can pin it without a UI: appending at the end
 * instead of at the caret is the classic emoji-picker bug, and it is invisible
 * until someone moves the cursor.
 */
fun insertEmoji(value: TextFieldValue, emoji: String): TextFieldValue {
    val start = value.selection.start.coerceIn(0, value.text.length)
    val end = value.selection.end.coerceIn(start, value.text.length)
    val next = value.text.replaceRange(start, end, emoji)
    return TextFieldValue(text = next, selection = TextRange(start + emoji.length))
}

/**
 * The picker as a modal bottom sheet.
 *
 * It deliberately does *not* close on a pick: choosing three emoji in a row is
 * the common case, so it stays open and reports each one. It closes on the
 * scrim, the back gesture, or Done.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun EmojiPickerSheet(onPick: (String) -> Unit, onDismiss: () -> Unit) {
    val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    var group by remember { mutableStateOf(0) }
    val sheetShape = RoundedCornerShape(topStart = ComradeRadii.xl, topEnd = ComradeRadii.xl)
    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = sheetState,
        modifier = Modifier.testTag("emoji-sheet").glassSurface(GlassElevation.Sheet, shape = sheetShape),
        shape = sheetShape,
        containerColor = Color.Transparent,
    ) {
        Column(Modifier.fillMaxWidth()) {
            ScrollableTabRow(selectedTabIndex = group, edgePadding = 8.dp) {
                EmojiGroups.forEachIndexed { index, g ->
                    Tab(
                        selected = index == group,
                        onClick = { group = index },
                        text = { Text(g.label, style = MaterialTheme.typography.labelSmall) },
                    )
                }
            }
            LazyVerticalGrid(
                columns = GridCells.Adaptive(minSize = 44.dp),
                modifier = Modifier
                    .fillMaxWidth()
                    .height(280.dp),
                contentPadding = PaddingValuesCompat,
            ) {
                items(EmojiGroups[group].emoji) { emoji ->
                    TextButton(onClick = { onPick(emoji) }) {
                        Text(emoji, style = MaterialTheme.typography.headlineSmall)
                    }
                }
            }
            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(horizontal = 8.dp, vertical = 4.dp),
                horizontalArrangement = Arrangement.End,
            ) {
                TextButton(onClick = onDismiss) { Text("Done") }
            }
        }
    }
}

/** Grid padding, hoisted so the composable body stays readable. */
private val PaddingValuesCompat = androidx.compose.foundation.layout.PaddingValues(
    horizontal = 8.dp,
    vertical = 4.dp,
)
