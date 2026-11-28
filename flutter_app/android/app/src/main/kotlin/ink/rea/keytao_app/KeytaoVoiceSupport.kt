package ink.rea.keytao_app

import android.Manifest
import android.app.Activity
import android.content.ActivityNotFoundException
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.content.pm.ResolveInfo
import android.os.Build
import android.provider.Settings
import android.speech.RecognitionService
import android.speech.SpeechRecognizer
import org.xmlpull.v1.XmlPullParser
import java.io.File

internal data class VoiceEngineInfo(
    val label: String,
    val packageName: String,
    val component: String,
    val settingsActivity: String? = null,
) {
    fun toMap(): Map<String, Any?> = mapOf(
        "label" to label,
        "packageName" to packageName,
        "component" to component,
        "hasSettings" to !settingsActivity.isNullOrBlank(),
    )
}

internal data class VoiceInputStatus(
    val enabled: Boolean,
    val recognitionAvailable: Boolean,
    val engineInstalled: Boolean,
    val engineSelected: Boolean,
    val usesSystemDefault: Boolean,
    val selectionInvalid: Boolean,
    val selectedEngineName: String?,
    val selectedEnginePackage: String?,
    val selectedEngineComponent: String?,
    val hasModelSettings: Boolean,
    val permissionGranted: Boolean,
    val permissionPermanentlyDenied: Boolean,
    val engines: List<VoiceEngineInfo>,
) {
    val ready: Boolean
        get() = enabled && recognitionAvailable && engineInstalled && engineSelected && permissionGranted

    fun toMap(): Map<String, Any?> = mapOf(
        "enabled" to enabled,
        "ready" to ready,
        "recognitionAvailable" to recognitionAvailable,
        "engineInstalled" to engineInstalled,
        "engineSelected" to engineSelected,
        "usesSystemDefault" to usesSystemDefault,
        "selectionInvalid" to selectionInvalid,
        "selectedEngineName" to selectedEngineName,
        "selectedEnginePackage" to selectedEnginePackage,
        "selectedEngineComponent" to selectedEngineComponent,
        "hasModelSettings" to hasModelSettings,
        "permissionGranted" to permissionGranted,
        "permissionPermanentlyDenied" to permissionPermanentlyDenied,
        "engines" to engines.map(VoiceEngineInfo::toMap),
    )
}

internal object KeytaoVoiceSupport {
    private const val disabledFlagFileName = "voice-input-disabled"
    private const val voiceRecognitionServiceSetting = "voice_recognition_service"
    private const val androidXmlNamespace = "http://schemas.android.com/apk/res/android"
    private const val actionVoiceInputSettings = "android.settings.VOICE_INPUT_SETTINGS"
    private const val legacyActionVoiceInputSettings = "com.android.settings.VOICE_INPUT_SETTINGS"

    fun isVoiceInputEnabled(context: Context): Boolean {
        return !File(context.filesDir, disabledFlagFileName).isFile
    }

    fun setVoiceInputEnabled(context: Context, enabled: Boolean) {
        val file = File(context.filesDir, disabledFlagFileName)
        if (enabled) {
            if (file.isFile) {
                check(file.delete()) { "无法保存语音输入开关" }
            }
        } else {
            check(file.createNewFile() || file.isFile) { "无法保存语音输入开关" }
        }
    }

    fun isRecognitionAvailable(context: Context): Boolean {
        return runCatching { SpeechRecognizer.isRecognitionAvailable(context) }.getOrDefault(false)
    }

    fun hasRecordAudioPermission(context: Context): Boolean {
        return context.checkSelfPermission(Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED
    }

    fun inspectStatus(context: Context, permissionPermanentlyDenied: Boolean = false): VoiceInputStatus {
        val engines = queryInstalledEngines(context)
        val configuredComponent = runCatching {
            Settings.Secure.getString(context.contentResolver, voiceRecognitionServiceSetting)
        }.getOrNull()
        val recognitionAvailable = isRecognitionAvailable(context)
        val permissionGranted = hasRecordAudioPermission(context)
        return resolveStatus(
            enabled = isVoiceInputEnabled(context),
            recognitionAvailable = recognitionAvailable,
            engines = engines,
            configuredComponent = configuredComponent,
            permissionGranted = permissionGranted,
            permissionPermanentlyDenied = !permissionGranted && permissionPermanentlyDenied,
        )
    }

    internal fun resolveStatus(
        enabled: Boolean,
        recognitionAvailable: Boolean,
        engines: List<VoiceEngineInfo>,
        configuredComponent: String?,
        permissionGranted: Boolean,
        permissionPermanentlyDenied: Boolean,
    ): VoiceInputStatus {
        val normalizedConfigured = normalizeComponentName(configuredComponent)
        val matched = normalizedConfigured?.let { target ->
            engines.firstOrNull { normalizeComponentName(it.component) == target }
        }
        val engineInstalled = engines.isNotEmpty() || recognitionAvailable
        val selectionInvalid = normalizedConfigured != null && matched == null && engines.isNotEmpty()
        val usesSystemDefault = normalizedConfigured == null && recognitionAvailable && engineInstalled
        val selectedEngine = when {
            matched != null -> matched
            usesSystemDefault && engines.size == 1 -> engines.first()
            else -> null
        }
        val engineSelected = when {
            matched != null -> true
            usesSystemDefault -> true
            else -> false
        }
        val modelSettingsEngine = selectedEngine?.takeIf { !it.settingsActivity.isNullOrBlank() }
            ?: engines.firstOrNull { !it.settingsActivity.isNullOrBlank() }
        return VoiceInputStatus(
            enabled = enabled,
            recognitionAvailable = recognitionAvailable,
            engineInstalled = engineInstalled,
            engineSelected = engineSelected,
            usesSystemDefault = usesSystemDefault,
            selectionInvalid = selectionInvalid,
            selectedEngineName = selectedEngine?.label,
            selectedEnginePackage = selectedEngine?.packageName,
            selectedEngineComponent = selectedEngine?.component ?: normalizedConfigured,
            hasModelSettings = modelSettingsEngine != null,
            permissionGranted = permissionGranted,
            permissionPermanentlyDenied = !permissionGranted && permissionPermanentlyDenied,
            engines = engines,
        )
    }

    internal fun normalizeComponentName(raw: String?): String? {
        val trimmed = raw?.trim().orEmpty()
        if (trimmed.isEmpty()) return null
        val slash = trimmed.indexOf('/')
        if (slash <= 0 || slash >= trimmed.lastIndex) return trimmed
        val pkg = trimmed.substring(0, slash)
        val cls = trimmed.substring(slash + 1)
        val fullCls = if (cls.startsWith('.')) pkg + cls else cls
        return "$pkg/$fullCls"
    }

    internal fun resolveSettingsActivityClass(packageName: String, rawActivity: String?): String? {
        val trimmed = rawActivity?.trim().orEmpty()
        if (trimmed.isEmpty()) return null
        return if (trimmed.startsWith('.')) packageName + trimmed else trimmed
    }

    @Suppress("DEPRECATION")
    private fun queryInstalledEngines(context: Context): List<VoiceEngineInfo> {
        val pm = context.packageManager
        val intent = Intent(RecognitionService.SERVICE_INTERFACE)
        val resolved: List<ResolveInfo> = runCatching {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                pm.queryIntentServices(
                    intent,
                    PackageManager.ResolveInfoFlags.of(PackageManager.GET_META_DATA.toLong()),
                )
            } else {
                pm.queryIntentServices(intent, PackageManager.GET_META_DATA)
            }
        }.getOrDefault(emptyList())
        val seen = LinkedHashSet<String>()
        val engines = mutableListOf<VoiceEngineInfo>()
        for (info in resolved) {
            val service = info.serviceInfo ?: continue
            val component = ComponentName(service.packageName, service.name).flattenToString()
            if (!seen.add(component)) continue
            val label = runCatching {
                info.loadLabel(pm).toString().trim().takeIf { it.isNotEmpty() }
            }.getOrNull() ?: runCatching {
                service.applicationInfo?.let { pm.getApplicationLabel(it).toString().trim() }
                    ?.takeIf { it.isNotEmpty() }
            }.getOrNull() ?: service.packageName
            val settingsActivity = parseSettingsActivity(pm, info)
            engines.add(
                VoiceEngineInfo(
                    label = label,
                    packageName = service.packageName,
                    component = component,
                    settingsActivity = settingsActivity,
                ),
            )
        }
        return engines
    }

    private fun parseSettingsActivity(pm: PackageManager, info: ResolveInfo): String? {
        val service = info.serviceInfo ?: return null
        return runCatching {
            val parser = service.loadXmlMetaData(pm, RecognitionService.SERVICE_META_DATA) ?: return null
            parser.use { xml ->
                var event = xml.eventType
                while (event != XmlPullParser.END_DOCUMENT) {
                    if (event == XmlPullParser.START_TAG && xml.name == "recognition-service") {
                        val raw = xml.getAttributeValue(androidXmlNamespace, "settingsActivity")
                        return resolveSettingsActivityClass(service.packageName, raw)
                    }
                    event = xml.next()
                }
            }
            null
        }.getOrNull()
    }

    fun openVoiceInputSettings(activity: Activity) {
        val candidates = listOf(
            Intent(actionVoiceInputSettings),
            Intent(legacyActionVoiceInputSettings),
            Intent(Settings.ACTION_INPUT_METHOD_SETTINGS),
            Intent(Settings.ACTION_SETTINGS),
        )
        for (intent in candidates) {
            try {
                intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                activity.startActivity(intent)
                return
            } catch (_: ActivityNotFoundException) {
                continue
            } catch (_: SecurityException) {
                continue
            }
        }
        throw IllegalStateException("无法打开系统语音输入设置")
    }

    fun openVoiceModelSettings(activity: Activity) {
        val engines = queryInstalledEngines(activity)
        val configured = normalizeComponentName(
            runCatching {
                Settings.Secure.getString(activity.contentResolver, voiceRecognitionServiceSetting)
            }.getOrNull(),
        )
        val preferred = configured?.let { target ->
            engines.firstOrNull { normalizeComponentName(it.component) == target }
        }?.takeIf { !it.settingsActivity.isNullOrBlank() }
            ?: engines.firstOrNull { !it.settingsActivity.isNullOrBlank() }

        if (preferred != null && !preferred.settingsActivity.isNullOrBlank()) {
            try {
                val intent = Intent().apply {
                    setClassName(preferred.packageName, preferred.settingsActivity)
                    addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                }
                activity.startActivity(intent)
                return
            } catch (_: ActivityNotFoundException) {
                // Fall through to system voice input settings.
            } catch (_: SecurityException) {
                // Fall through to system voice input settings.
            }
        }
        openVoiceInputSettings(activity)
    }
}
