package ink.rea.keytao_app

import android.content.Context
import android.content.Intent
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.os.ResultReceiver
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
        val finished = AtomicBoolean(false)
        val started = System.nanoTime()
        fun finish(result: Result) {
            if (!finished.compareAndSet(false, true)) return
            KeytaoRuntimeLog.event("rime", "deploy", KeytaoRuntimeLog.elapsedMs(started)) {
                put("success", result.success)
            }
            callback(result)
        }
        val timeout = Runnable {
            runCatching { context.stopService(Intent(context, KeytaoRimeDeployWorkerService::class.java)) }
            finish(Result(success = false, error = "Android RIME deployment timed out"))
        }
        val receiver = object : ResultReceiver(handler) {
            override fun onReceiveResult(resultCode: Int, resultData: Bundle?) {
                if (finished.get()) return
                handler.removeCallbacks(timeout)
                val deployed = resultCode == KeytaoRimeDeployContract.resultOk
                handler.postDelayed({
                    finish(Result(
                        success = deployed,
                        path = KeytaoAndroidPaths.reloadStampFile(context).absolutePath,
                        deployed = deployed,
                        error = resultData?.getString(KeytaoRimeDeployContract.keyError).orEmpty(),
                    ))
                }, processExitGraceMs)
            }
        }
        try {
            val intent = Intent(context, KeytaoRimeDeployWorkerService::class.java).apply {
                putExtra(KeytaoRimeDeployContract.extraReceiver, receiver)
                KeytaoAndroidPaths.testRootOverride?.let {
                    putExtra(KeytaoRimeDeployContract.extraTestRoot, it.absolutePath)
                }
            }
            handler.postDelayed(timeout, timeoutMs)
            check(context.startService(intent) != null) {
                "Android RIME deployment service is unavailable"
            }
        } catch (error: Throwable) {
            handler.removeCallbacks(timeout)
            finish(Result(success = false, error = error.message ?: "Android RIME 部署失败"))
        }
    }

    private const val defaultTimeoutMs = 600_000L
    private const val minimumTimeoutMs = 1_000L
    private const val processExitGraceMs = 350L
}

internal object KeytaoRimeDeployContract {
    const val notificationId = 1002
    const val extraTestRoot = "ink.rea.keytao_app.extra.TEST_ROOT"
    const val extraReceiver = "ink.rea.keytao_app.extra.RIME_DEPLOY_RECEIVER"
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
