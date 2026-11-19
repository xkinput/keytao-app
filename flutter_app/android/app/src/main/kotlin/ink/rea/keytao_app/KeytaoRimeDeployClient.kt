package ink.rea.keytao_app

import android.content.Context
import android.content.Intent
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.os.ResultReceiver
import org.json.JSONObject
import java.io.File
import java.util.ArrayDeque
import java.util.concurrent.atomic.AtomicBoolean

object KeytaoRimeDeployClient {
    data class Result(
        val success: Boolean,
        val path: String = "",
        val schemaName: String = "",
        val deployed: Boolean = false,
        val error: String = "",
        val warning: String = "",
    )

    internal fun timeoutMsForSchemas(schemas: List<String>) =
        if (schemas.any(::isAddonSchema)) addonTimeoutMs else defaultTimeoutMs

    fun deploy(
        context: Context,
        timeoutMs: Long = defaultTimeoutMs,
        callback: (Result) -> Unit,
    ) {
        val handler = Handler(Looper.getMainLooper())
        val finished = AtomicBoolean(false)
        val timeout = Runnable {
            if (finished.compareAndSet(false, true)) {
                callback(Result(success = false, error = "Android RIME deployment timed out"))
            }
        }
        val budget = timeoutMs.coerceAtLeast(minimumTimeoutMs)
        val receiver = object : ResultReceiver(handler) {
            override fun onReceiveResult(resultCode: Int, resultData: Bundle?) {
                if (finished.get()) return
                handler.removeCallbacks(timeout)
                if (resultCode == KeytaoRimeDeployContract.resultWaiting) {
                    handler.postDelayed(
                        timeout,
                        resultData?.getLong(KeytaoRimeDeployContract.keyTimeoutMs, budget)
                            ?.coerceAtLeast(minimumTimeoutMs) ?: budget,
                    )
                    return
                }
                if (!finished.compareAndSet(false, true)) return
                callback(Result(
                    success = resultCode == KeytaoRimeDeployContract.resultOk,
                    path = resultData?.getString(KeytaoRimeDeployContract.keyPath).orEmpty(),
                    schemaName = resultData?.getString(KeytaoRimeDeployContract.keySchemaName).orEmpty(),
                    deployed = resultData?.getBoolean(KeytaoRimeDeployContract.keyDeployed, false) == true,
                    error = resultData?.getString(KeytaoRimeDeployContract.keyError).orEmpty(),
                    warning = resultData?.getString(KeytaoRimeDeployContract.keyWarning).orEmpty(),
                ))
            }
        }
        handler.post {
            handler.postDelayed(timeout, budget)
            try {
                val intent = Intent(context, KeytaoRimeDeployService::class.java).apply {
                    putExtra(KeytaoRimeDeployContract.extraReceiver, receiver)
                    putExtra(KeytaoRimeDeployContract.keyTimeoutMs, budget)
                    KeytaoAndroidPaths.testRootOverride?.let {
                        putExtra(KeytaoRimeDeployContract.extraTestRoot, it.absolutePath)
                    }
                }
                check(context.applicationContext.startService(intent) != null) {
                    "Android RIME deployment service is unavailable"
                }
            } catch (error: Throwable) {
                handler.removeCallbacks(timeout)
                if (finished.compareAndSet(false, true)) {
                    callback(Result(success = false, error = error.message ?: "Android RIME 部署失败"))
                }
            }
        }
    }

    internal fun runDeployment(context: Context, timeoutMs: Long, callback: (Result) -> Unit) {
        val handler = Handler(Looper.getMainLooper())
        val root = runCatching { KeytaoAndroidPaths.userRoot(context) }.getOrElse { error ->
            handler.post { callback(Result(success = false, error = error.message ?: "存储尚未就绪")) }
            return
        }
        Deployment(
            context = context.applicationContext,
            userRoot = root,
            handler = handler,
            timeoutMs = timeoutMs.coerceAtLeast(minimumTimeoutMs),
            callback = callback,
        ).start()
    }

    private class Deployment(
        private val context: Context,
        private val userRoot: File,
        private val handler: Handler,
        private val timeoutMs: Long,
        private val callback: (Result) -> Unit,
    ) {
        private val finished = AtomicBoolean(false)
        private val pending = ArrayDeque<PendingSchema>()
        private val processed = linkedSetOf<String>()
        private var initialSchemas = emptyList<String>()
        private var current: PendingSchema? = null
        private var renderingLogos = false
        private var timingOut = false
        private var startedNs = 0L
        private var stepStartedNs = 0L
        private val timeout = Runnable {
            timingOut = true
            runCatching { context.stopService(Intent(context, KeytaoRimeDeployWorkerService::class.java)) }
            handler.postDelayed({
                if (renderingLogos) completeDeployment()
                else finish(Result(success = false, error = "Android RIME deployment timed out"))
            }, processExitGraceMs)
        }

        fun start() {
            startedNs = System.nanoTime()
            try {
                KeytaoAndroidPaths.beginDeployment(userRoot)
            } catch (error: Throwable) {
                finish(Result(success = false, error = error.message ?: "Android RIME 部署失败"))
                return
            }
            KeytaoRuntimeLog.event("rime", "deploy_start")
            handler.postDelayed(timeout, timeoutMs)
            startStep(null)
        }

        private fun startStep(schema: PendingSchema?, renderLogos: Boolean = false) {
            if (finished.get() || timingOut) return
            current = schema
            renderingLogos = renderLogos
            stepStartedNs = System.nanoTime()
            val delivered = AtomicBoolean(false)
            val receiver = object : ResultReceiver(handler) {
                override fun onReceiveResult(resultCode: Int, resultData: Bundle?) {
                    if (!delivered.compareAndSet(false, true) || finished.get() || timingOut) return
                    val success = resultCode == KeytaoRimeDeployContract.resultOk &&
                        resultData?.getBoolean(KeytaoRimeDeployContract.keySuccess, false) == true
                    val schemas = resultData
                        ?.getStringArrayList(KeytaoRimeDeployContract.keySchemas)
                        .orEmpty()
                    val error = resultData
                        ?.getString(KeytaoRimeDeployContract.keyError)
                        .orEmpty()
                    handler.postDelayed(
                        { handleStepResult(success, schemas, error) },
                        processExitGraceMs,
                    )
                }
            }

            try {
                val intent = Intent(context, KeytaoRimeDeployWorkerService::class.java).apply {
                    putExtra(KeytaoRimeDeployContract.extraReceiver, receiver)
                    schema?.let { putExtra(KeytaoRimeDeployContract.extraSchemaId, it.id) }
                    putExtra(KeytaoRimeDeployContract.extraRenderLogos, renderLogos)
                    KeytaoAndroidPaths.testRootOverride?.let {
                        putExtra(KeytaoRimeDeployContract.extraTestRoot, it.absolutePath)
                    }
                }
                if (context.startService(intent) == null) {
                    throw IllegalStateException("Android RIME deployment service is unavailable")
                }
            } catch (error: Throwable) {
                handleStepResult(
                    false,
                    emptyList(),
                    error.message ?: "Failed to start Android RIME deployment service",
                )
            }
        }

        private fun handleStepResult(success: Boolean, schemas: List<String>, error: String) {
            if (finished.get() || timingOut) return
            logStepResult(success, schemas.size)
            if (renderingLogos) {
                completeDeployment()
                return
            }
            if (!success) {
                finish(Result(success = false, error = error.ifBlank { "Android RIME 部署失败" }))
                return
            }
            val completed = current
            if (completed == null) {
                initialSchemas = schemas.distinct()
                if (initialSchemas.isEmpty()) {
                    finish(Result(success = false, error = "未找到要部署的 Android RIME 方案"))
                    return
                }
                initialSchemas.forEach { pending.addLast(PendingSchema(it, required = true)) }
            } else {
                schemas.forEach { dependency ->
                    if (dependency.isNotBlank() && dependency !in processed) {
                        pending.addLast(PendingSchema(dependency, required = false))
                    }
                }
            }
            deployNextSchema()
        }

        private fun deployNextSchema() {
            while (pending.isNotEmpty()) {
                val schema = pending.removeFirst()
                if (!processed.add(schema.id)) continue
                val source = File(userRoot, "${schema.id}.schema.yaml")
                if (!source.isFile && !schema.required) continue
                startStep(schema)
                return
            }
            startStep(null, renderLogos = true)
        }

        private fun completeDeployment() {
            try {
                val stamp = KeytaoAndroidPaths.reloadStampFile(context)
                check(KeytaoNativeBridge.completeDeployment(userRoot.absolutePath)) {
                    "Android RIME 部署未生成方案产物"
                }
                val schemas = parseSchemas(File(userRoot, "build/default.yaml").readText())
                    .filter(::isSafeSchemaId)
                val selected = runCatching {
                    Regex("""(?m)^\s*previously_selected_schema:\s*["']?([A-Za-z0-9_.-]+)["']?\s*(?:#.*)?$""")
                        .find(File(userRoot, "user.yaml").readText())?.groupValues?.get(1)
                }.getOrNull()
                val schemaId = selected?.takeIf { it in schemas } ?: schemas.first()
                val schemaName = RimeSchemaNameResolver.resolveDisplayName(
                    userRoot,
                    KeytaoAndroidPaths.rimeDataDir(context),
                    schemaId,
                )
                val warning = runCatching {
                    val skipped = JSONObject(File(userRoot, "build/.keytao-skipped-schemas.json").readText())
                    skipped.keys().asSequence().sorted().joinToString("\n") { skipped.getString(it) }
                }.getOrDefault("")
                stamp.parentFile?.mkdirs()
                stamp.writeText(System.nanoTime().toString())
                finish(
                    Result(
                        success = true,
                        path = stamp.absolutePath,
                        schemaName = schemaName,
                        deployed = true,
                        warning = warning,
                    )
                )
            } catch (error: Throwable) {
                runCatching { KeytaoAndroidPaths.beginDeployment(userRoot) }
                finish(Result(success = false, error = error.message ?: "Android RIME 部署失败"))
            }
        }

        private fun finish(result: Result) {
            if (!finished.compareAndSet(false, true)) return
            handler.removeCallbacks(timeout)
            if (!result.success) KeytaoNativeBridge.markDeploymentFailed(userRoot.absolutePath)
            runCatching {
                logStepResult(result.success, 0)
                val success = result.success
                val schemaCount = processed.size
                KeytaoRuntimeLog.event("rime", "deploy", KeytaoRuntimeLog.elapsedMs(startedNs)) {
                    put("success", success)
                    put("schema_count", schemaCount)
                }
            }
            callback(result)
        }

        private fun logStepResult(success: Boolean, schemaCount: Int) {
            if (stepStartedNs == 0L) return
            val duration = KeytaoRuntimeLog.elapsedMs(stepStartedNs)
            stepStartedNs = 0L
            val configStep = current == null
            val schemaIndex = processed.size
            KeytaoRuntimeLog.event("rime", "deploy_step", duration) {
                put("success", success)
                put("config_step", configStep)
                put("schema_index", schemaIndex)
                put("schema_count", schemaCount)
            }
        }
    }

    private data class PendingSchema(val id: String, val required: Boolean)

    private const val defaultTimeoutMs = 180_000L
    private const val addonTimeoutMs = 300_000L
    private const val minimumTimeoutMs = 1_000L
    private const val processExitGraceMs = 350L
}

internal object KeytaoRimeDeployContract {
    const val notificationId = 1002
    const val extraTestRoot = "ink.rea.keytao_app.extra.TEST_ROOT"
    const val extraReceiver = "ink.rea.keytao_app.extra.RIME_DEPLOY_RECEIVER"
    const val extraSchemaId = "ink.rea.keytao_app.extra.RIME_DEPLOY_SCHEMA_ID"
    const val extraRenderLogos = "ink.rea.keytao_app.extra.RIME_RENDER_LOGOS"
    const val keySuccess = "success"
    const val keySchemas = "schemas"
    const val keyError = "error"
    const val keyPath = "path"
    const val keySchemaName = "schemaName"
    const val keyDeployed = "deployed"
    const val keyWarning = "warning"
    const val keyTimeoutMs = "timeoutMs"
    const val resultWaiting = 2
    const val resultOk = 1
    const val resultError = 0
}
