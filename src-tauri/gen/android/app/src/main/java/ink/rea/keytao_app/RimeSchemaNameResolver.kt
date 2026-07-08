package ink.rea.keytao_app

import java.io.File

object RimeSchemaNameResolver {
    fun resolveDisplayName(userDir: File, sharedDir: File?, rawName: String): String {
        val normalized = rawName.trim().trimStart('\uFEFF')
        if (normalized.isEmpty() || normalized.startsWith(".")) return normalized

        val schemaFile = findSchemaFile(userDir, sharedDir, normalized) ?: return normalized
        val parsed = parseSchemaMetadata(schemaFile.readText())
        return if (parsed.id == normalized && parsed.name.isNotBlank()) parsed.name else normalized
    }

    fun parseSchemaMetadata(content: String): SchemaMetadata {
        var inSchema = false
        var schemaIndent = -1
        var schemaId = ""
        var schemaName = ""

        for (rawLine in content.lineSequence()) {
            val line = rawLine.trimStart('\uFEFF')
            val trimmed = line.trim()
            if (trimmed.isEmpty() || trimmed.startsWith("#")) continue

            val indent = line.length - line.trimStart().length
            if (trimmed == "schema:") {
                inSchema = true
                schemaIndent = indent
                continue
            }
            if (inSchema && indent <= schemaIndent) {
                inSchema = false
            }
            if (!inSchema) continue

            val keyValue = trimmed.split(":", limit = 2)
            if (keyValue.size != 2) continue
            val value = cleanYamlScalar(keyValue[1])
            when (keyValue[0].trim()) {
                "schema_id" -> schemaId = value
                "name" -> schemaName = value
            }
        }

        return SchemaMetadata(schemaId, schemaName)
    }

    private fun findSchemaFile(userDir: File, sharedDir: File?, schemaId: String): File? {
        val relative = "$schemaId.schema.yaml"
        return listOfNotNull(
            File(userDir, relative),
            File(userDir, "build/$relative"),
            sharedDir?.let { File(it, relative) },
        ).firstOrNull { it.isFile }
    }

    private fun cleanYamlScalar(raw: String): String {
        val trimmed = raw.trim()
        if (trimmed.isEmpty()) return ""
        if (trimmed.first() == '"' || trimmed.first() == '\'') {
            val quote = trimmed.first()
            val end = trimmed.indexOf(quote, startIndex = 1)
            return if (end > 0) trimmed.substring(1, end) else trimmed.drop(1)
        }
        return trimmed.substringBefore("#").trim()
    }
}

data class SchemaMetadata(
    val id: String = "",
    val name: String = "",
)
