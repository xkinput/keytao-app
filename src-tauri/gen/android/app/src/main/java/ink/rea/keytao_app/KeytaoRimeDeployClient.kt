package ink.rea.keytao_app

import android.content.Context
import android.content.Intent
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.os.ResultReceiver
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
    )

    private val deploymentRunning = AtomicBoolean(false)

    fun deploy(
        context: Context,
        timeoutMs: Long = defaultTimeoutMs,
        callback: (Result) -> Unit,
    ) {
        val handler = Handler(Looper.getMainLooper())
        if (!deploymentRunning.compareAndSet(false, true)) {
            handler.post {
                callback(Result(success = false, error = "Android RIME deployment is already running"))
            }
            return
        }

        Deployment(
            context = context.applicationContext,
            handler = handler,
            timeoutMs = timeoutMs.coerceAtLeast(minimumTimeoutMs),
            callback = callback,
        ).start()
    }

    private class Deployment(
        private val context: Context,
        private val handler: Handler,
        private val timeoutMs: Long,
        private val callback: (Result) -> Unit,
    ) {
        private val finished = AtomicBoolean(false)
        private val pending = ArrayDeque<PendingSchema>()
        private val processed = linkedSetOf<String>()
        private var initialSchemas = emptyList<String>()
        private var current: PendingSchema? = null
        private val timeout = Runnable {
            finish(Result(success = false, error = "Android RIME deployment timed out"))
        }

        fun start() {
            handler.postDelayed(timeout, timeoutMs)
            startStep(null)
        }

        private fun startStep(schema: PendingSchema?) {
            if (finished.get()) return
            current = schema
            val delivered = AtomicBoolean(false)
            val receiver = object : ResultReceiver(handler) {
                override fun onReceiveResult(resultCode: Int, resultData: Bundle?) {
                    if (!delivered.compareAndSet(false, true) || finished.get()) return
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
                val intent = Intent(context, KeytaoRimeDeployService::class.java).apply {
                    putExtra(KeytaoRimeDeployContract.extraReceiver, receiver)
                    schema?.let { putExtra(KeytaoRimeDeployContract.extraSchemaId, it.id) }
                }
                if (context.startService(intent) == null) {
                    throw IllegalStateException("Android RIME deployment service is unavailable")
                }
            } catch (error: Throwable) {
                finish(
                    Result(
                        success = false,
                        error = error.message ?: "Failed to start Android RIME deployment service",
                    )
                )
            }
        }

        private fun handleStepResult(success: Boolean, schemas: List<String>, error: String) {
            if (finished.get()) return
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
                val source = File(KeytaoAndroidPaths.userRoot(), "${schema.id}.schema.yaml")
                if (!source.isFile) {
                    if (schema.required) {
                        finish(Result(success = false, error = "缺少方案文件：${source.name}"))
                        return
                    }
                    continue
                }
                startStep(schema)
                return
            }
            completeDeployment()
        }

        private fun completeDeployment() {
            if (!KeytaoAndroidPaths.hasDeployedSchema()) {
                finish(Result(success = false, error = "Android RIME 部署未生成方案产物"))
                return
            }
            try {
                val stamp = KeytaoAndroidPaths.reloadStampFile()
                stamp.parentFile?.mkdirs()
                stamp.writeText(System.currentTimeMillis().toString())
                val schemaId = initialSchemas.first()
                val schemaName = RimeSchemaNameResolver.resolveDisplayName(
                    KeytaoAndroidPaths.userRoot(),
                    KeytaoAndroidPaths.rimeDataDir(),
                    schemaId,
                )
                finish(
                    Result(
                        success = true,
                        path = stamp.absolutePath,
                        schemaName = schemaName,
                        deployed = true,
                    )
                )
            } catch (error: Throwable) {
                finish(Result(success = false, error = error.message ?: "Android RIME 部署失败"))
            }
        }

        private fun finish(result: Result) {
            if (!finished.compareAndSet(false, true)) return
            handler.removeCallbacks(timeout)
            deploymentRunning.set(false)
            callback(result)
        }
    }

    private data class PendingSchema(val id: String, val required: Boolean)

    private const val defaultTimeoutMs = 180_000L
    private const val minimumTimeoutMs = 1_000L
    private const val processExitGraceMs = 350L
}

internal object KeytaoRimeDeployContract {
    const val extraReceiver = "ink.rea.keytao_app.extra.RIME_DEPLOY_RECEIVER"
    const val extraSchemaId = "ink.rea.keytao_app.extra.RIME_DEPLOY_SCHEMA_ID"
    const val keySuccess = "success"
    const val keySchemas = "schemas"
    const val keyError = "error"
    const val resultOk = 1
    const val resultError = 0
}
