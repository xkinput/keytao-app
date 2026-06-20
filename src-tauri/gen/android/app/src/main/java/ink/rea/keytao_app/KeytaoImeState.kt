package ink.rea.keytao_app

import org.json.JSONArray
import org.json.JSONObject

data class KeytaoCandidate(
    val index: Int = 0,
    val text: String,
    val comment: String? = null,
)

data class KeytaoPanelCandidate(
    val index: Int,
    val label: String,
    val text: String,
    val comment: String? = null,
    val selected: Boolean = false,
)

data class KeytaoPanelNavigation(
    val canGoPrevious: Boolean = false,
    val canGoNext: Boolean = false,
)

data class KeytaoPanelModel(
    val preedit: String? = null,
    val candidates: List<KeytaoPanelCandidate> = emptyList(),
    val navigation: KeytaoPanelNavigation = KeytaoPanelNavigation(),
)

data class KeytaoModeHintModel(
    val asciiMode: Boolean = false,
    val text: String = "",
)

data class KeytaoImeState(
    val preedit: String = "",
    val cursor: Int = 0,
    val candidates: List<KeytaoCandidate> = emptyList(),
    val allCandidates: List<KeytaoCandidate> = emptyList(),
    val highlightedCandidateIndex: Int = 0,
    val pageSize: Int = 0,
    val page: Int = 0,
    val isLastPage: Boolean = true,
    val committed: String = "",
    val selectKeys: String = "",
    val asciiMode: Boolean = false,
    val schemaName: String = "",
    val accepted: Boolean = false,
    val candidatePanel: KeytaoPanelModel = KeytaoPanelModel(),
    val modeHint: KeytaoModeHintModel = KeytaoModeHintModel(),
) {
    val hasComposition: Boolean
        get() = preedit.isNotEmpty() || candidates.isNotEmpty()

    fun withoutTransientCommit() = copy(committed = "", accepted = false)

    companion object {
        fun empty(asciiMode: Boolean = false) = KeytaoImeState(asciiMode = asciiMode)

        fun fromJson(json: String?): KeytaoImeState? {
            if (json.isNullOrBlank()) return null
            return runCatching {
                val root = JSONObject(json)
                val candidates = parseCandidateArray(root.optJSONArray("candidates"))
                val allCandidates = parseCandidateArray(root.optJSONArray("allCandidates"))

                KeytaoImeState(
                    preedit = root.safeString("preedit"),
                    cursor = root.optInt("cursor"),
                    candidates = candidates,
                    allCandidates = allCandidates,
                    highlightedCandidateIndex = root.optInt("highlightedCandidateIndex"),
                    pageSize = root.optInt("pageSize"),
                    page = root.optInt("page"),
                    isLastPage = root.optBoolean("isLastPage", true),
                    committed = root.safeString("committed"),
                    selectKeys = root.safeString("selectKeys"),
                    asciiMode = root.optBoolean("asciiMode"),
                    schemaName = root.safeString("schemaName"),
                    accepted = root.optBoolean("accepted"),
                    candidatePanel = parsePanelModel(root.optJSONObject("candidatePanel")),
                    modeHint = parseModeHint(root.optJSONObject("modeHint")),
                )
            }.getOrNull()
        }

        fun parseCandidateArray(json: String?): List<KeytaoCandidate> {
            if (json.isNullOrBlank()) return emptyList()
            return runCatching { parseCandidateArray(JSONArray(json)) }.getOrDefault(emptyList())
        }

        fun parseCandidateArray(array: JSONArray?): List<KeytaoCandidate> {
            return buildList {
                if (array == null) return@buildList
                for (index in 0 until array.length()) {
                    val candidate = array.optJSONObject(index) ?: continue
                    add(
                        KeytaoCandidate(
                            index = index,
                            text = candidate.safeString("text"),
                            comment = candidate.optionalString("comment"),
                        )
                    )
                }
            }
        }

        private fun parsePanelModel(json: JSONObject?): KeytaoPanelModel {
            if (json == null) return KeytaoPanelModel()
            val candidates = buildList {
                val array = json.optJSONArray("candidates")
                if (array != null) {
                    for (index in 0 until array.length()) {
                        val candidate = array.optJSONObject(index) ?: continue
                        add(
                            KeytaoPanelCandidate(
                                index = candidate.optInt("index", index),
                                label = candidate.safeString("label"),
                                text = candidate.safeString("text"),
                                comment = candidate.optionalString("comment"),
                                selected = candidate.optBoolean("selected"),
                            )
                        )
                    }
                }
            }
            val navigationJson = json.optJSONObject("navigation")
            return KeytaoPanelModel(
                preedit = json.optionalString("preedit"),
                candidates = candidates,
                navigation = KeytaoPanelNavigation(
                    canGoPrevious = navigationJson?.optBoolean("canGoPrevious") ?: false,
                    canGoNext = navigationJson?.optBoolean("canGoNext") ?: false,
                ),
            )
        }

        private fun parseModeHint(json: JSONObject?): KeytaoModeHintModel {
            if (json == null) return KeytaoModeHintModel()
            return KeytaoModeHintModel(
                asciiMode = json.optBoolean("asciiMode"),
                text = json.safeString("text"),
            )
        }

        private fun JSONObject.safeString(name: String): String {
            return optionalString(name).orEmpty()
        }

        private fun JSONObject.optionalString(name: String): String? {
            if (!has(name) || isNull(name)) return null
            return optString(name).takeIf { it.isNotBlank() && it != "null" }
        }
    }
}
