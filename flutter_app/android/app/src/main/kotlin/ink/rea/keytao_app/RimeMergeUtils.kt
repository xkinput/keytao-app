package ink.rea.keytao_app

import org.json.JSONArray

data class MergeResult(val mergedContent: String, val userSchemas: List<String>)
data class RimeLuaMergeResult(val mergedContent: String, val renames: List<Pair<String, String>>)

private val managedSchemaPrefixes = listOf("keytao", "txjx", "xmjd6", "keydo")
private val addonSchemaIds = setOf("easy_en", "wanxiang")

fun isDefaultCustom(filename: String) =
    filename == "default.custom.yaml" || filename == "default-custom.yaml"

fun isManagedSchema(schema: String) =
    managedSchemaPrefixes.any { schema.startsWith(it) }

fun isAddonSchema(schema: String) = schema in addonSchemaIds

fun isSafeSchemaId(schema: String) =
    schema.isNotEmpty() && schema != "." && schema != ".." &&
        schema.all { it in 'a'..'z' || it in 'A'..'Z' || it in '0'..'9' || it in "_.-" }

fun parseSchemas(content: String): List<String> {
    val schemas = mutableListOf<String>()
    var inList = false
    for (line in content.lines()) {
        val t = line.trim()
        when {
            t.contains("schema_list:") -> inList = true
            inList -> {
                val m = Regex("^- schema:\\s*(.+)$").find(t)
                if (m != null) {
                    val raw = m.groupValues[1].trim()
                    val schema = when {
                        raw.startsWith('"') -> {
                            val quoted = Regex("""^("(?:[^"\\]|\\.)*")(?:\s+#.*)?$""").find(raw)
                            quoted?.let {
                                runCatching { JSONArray("[${it.groupValues[1]}]").getString(0) }.getOrNull()
                            }
                        }
                        raw.startsWith('\'') -> Regex("""^'((?:[^']|'')*)'(?:\s+#.*)?$""")
                            .find(raw)?.groupValues?.get(1)?.replace("''", "'")
                        else -> raw.substringBefore(" #").trim().takeIf { value ->
                            value.none(Char::isWhitespace)
                        }
                    }
                    if (schema != null) schemas.add(schema)
                } else if (t.isNotEmpty() && !t.startsWith('#') && !t.startsWith('-')) {
                    inList = false
                }
            }
        }
    }
    return schemas
}
