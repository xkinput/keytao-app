package ink.rea.keytao_app

import android.app.Service
import android.content.Intent
import android.os.Build
import android.os.Bundle
import android.os.IBinder
import android.os.Process
import android.os.ResultReceiver
import android.util.Log
import java.util.concurrent.atomic.AtomicBoolean

class KeytaoRimeDeployService : Service() {
    private val deploymentRunning = AtomicBoolean(false)

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val receiver = intent?.resultReceiver()
        if (receiver == null) {
            stopSelf(startId)
            return START_NOT_STICKY
        }
        if (!deploymentRunning.compareAndSet(false, true)) {
            sendResult(receiver, DeploymentResult(error = "Android RIME deployment is already running"))
            return START_NOT_STICKY
        }

        Thread(
            {
                val result = runDeployment(intent.getStringExtra(KeytaoRimeDeployContract.extraSchemaId))
                sendResult(receiver, result)
                deploymentRunning.set(false)
                stopSelfResult(startId)

                // librime's compiler allocator retains a large native heap after finalize.
                // Ending this dedicated process is the only reliable way to return it to Android.
                Thread.sleep(processExitDelayMs)
                Process.killProcess(Process.myPid())
            },
            "KeyTao-Rime-Deployer",
        ).start()
        return START_NOT_STICKY
    }

    private fun runDeployment(schemaId: String?): DeploymentResult {
        var engine: KeytaoImeEngine? = null
        return try {
            Log.i(tag, "Starting deployment step: ${schemaId ?: "default"}")
            engine = KeytaoImeEngine(applicationContext)
            if (!engine.hasInstalledSchema()) {
                return DeploymentResult(error = "请先安装键道方案")
            }
            val step = engine.deployStep(schemaId)
            if (!step.success) {
                return DeploymentResult(error = step.error.ifBlank { "Android RIME 部署失败" })
            }
            Log.i(tag, "Completed deployment step: ${schemaId ?: "default"}")
            DeploymentResult(
                success = true,
                schemas = step.schemas,
            )
        } catch (error: Throwable) {
            Log.e(tag, "Android RIME deployment failed", error)
            DeploymentResult(error = error.message ?: "Android RIME 部署失败")
        } finally {
            engine?.close()
        }
    }

    private fun sendResult(receiver: ResultReceiver, result: DeploymentResult) {
        val data = Bundle().apply {
            putBoolean(KeytaoRimeDeployContract.keySuccess, result.success)
            putStringArrayList(KeytaoRimeDeployContract.keySchemas, ArrayList(result.schemas))
            putString(KeytaoRimeDeployContract.keyError, result.error)
        }
        receiver.send(
            if (result.success) KeytaoRimeDeployContract.resultOk else KeytaoRimeDeployContract.resultError,
            data,
        )
    }

    @Suppress("DEPRECATION")
    private fun Intent.resultReceiver(): ResultReceiver? {
        return if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            getParcelableExtra(KeytaoRimeDeployContract.extraReceiver, ResultReceiver::class.java)
        } else {
            getParcelableExtra(KeytaoRimeDeployContract.extraReceiver)
        }
    }

    private data class DeploymentResult(
        val success: Boolean = false,
        val schemas: List<String> = emptyList(),
        val error: String = "",
    )

    companion object {
        private const val tag = "KeytaoRimeDeploy"
        private const val processExitDelayMs = 150L
    }
}
