package ink.rea.keytao_app

import java.util.Locale

internal object KeytaoMnemonicHints {
    enum class Tone { OUTER, SPECIAL, BRAND }

    data class Hint(
        val keyName: String,
        val initial: String? = null,
        val finals: List<String> = emptyList(),
        val roots: List<String> = emptyList(),
        val rootKey: String? = null,
        val tone: Tone = Tone.OUTER,
    ) {
        val topText: String? = when (tone) {
            Tone.OUTER -> initial
            Tone.SPECIAL -> rootKey
            Tone.BRAND -> null
        }

        val bottomLines: List<String> = (if (tone == Tone.SPECIAL) roots else finals)
            .chunked(2).map { it.joinToString(" ") }
    }

    private val hints = listOf(
        Hint("Q", initial = "zh", finals = listOf("iu", "ua")),
        Hint("W", initial = "ch", finals = listOf("ei", "un")),
        Hint("E", initial = "sh", finals = listOf("e")),
        Hint("R", finals = listOf("eng")),
        Hint("T", finals = listOf("uan")),
        Hint("Y", finals = listOf("iong", "ong")),
        Hint("U", roots = listOf("月", "十o"), rootKey = "丿", tone = Tone.SPECIAL),
        Hint("I", roots = listOf("人", "手u", "草i", "金o"), rootKey = "丨", tone = Tone.SPECIAL),
        Hint("O", roots = listOf("口", "日i"), rootKey = "丶", tone = Tone.SPECIAL),
        Hint("P", finals = listOf("ang")),
        Hint("A", roots = listOf("水", "贝o"), rootKey = "㇕", tone = Tone.SPECIAL),
        Hint("S", finals = listOf("a", "ia")),
        Hint("D", finals = listOf("ie", "ou")),
        Hint("F", initial = "zh", finals = listOf("an")),
        Hint("G", finals = listOf("ing", "uai")),
        Hint("H", finals = listOf("ai", "üe")),
        Hint("J", initial = "ch", finals = listOf("er", "u")),
        Hint("K", finals = listOf("i")),
        Hint("L", finals = listOf("o", "uo", "ü")),
        Hint(";"),
        Hint("Z", finals = listOf("ao")),
        Hint("X", finals = listOf("iang", "uang"), rootKey = "∅"),
        Hint("C", finals = listOf("iao")),
        Hint("V", roots = listOf("木", "土o"), rootKey = "一", tone = Tone.SPECIAL),
        Hint("B", finals = listOf("in", "ui")),
        Hint("N", finals = listOf("en")),
        Hint("M", finals = listOf("ian", "uang")),
        Hint(","),
        Hint("."),
        Hint("/", finals = listOf("键道6"), tone = Tone.BRAND),
    ).associateBy { it.keyName }

    fun forKey(key: KeySpec): Hint? {
        if (key.action.type !in listOf(KeyCommandTypes.INPUT, KeyCommandTypes.RIME_INPUT, KeyCommandTypes.DIRECT_INPUT)) {
            return null
        }
        return hints[key.value.uppercase(Locale.ROOT)]
            ?: key.asciiValue?.let { hints[it.uppercase(Locale.ROOT)] }
    }
}
