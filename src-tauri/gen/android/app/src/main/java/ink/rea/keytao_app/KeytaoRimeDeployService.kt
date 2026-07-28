package ink.rea.keytao_app

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.Service
import android.content.Intent
import android.content.pm.ServiceInfo
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

        // Compiling a dictionary takes tens of seconds; as a plain background
        // service it would be a candidate for reclaim as soon as the user leaves
        // the app, and the client could only report a timeout.
        startDeploymentForeground()

        Thread(
            {
                val result = runDeployment(intent.getStringExtra(KeytaoRimeDeployContract.extraSchemaId))
                sendResult(receiver, result)
                deploymentRunning.set(false)
                stopDeploymentForeground()
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

    private fun startDeploymentForeground() {
        runCatching {
            ensureNotificationChannel()
            val notification = buildNotification()
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                startForeground(
                    notificationId,
                    notification,
                    ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC,
                )
            } else {
                startForeground(notificationId, notification)
            }
        }.onFailure { error ->
            Log.w(tag, "Failed to enter foreground for deployment", error)
        }
    }

    private fun stopDeploymentForeground() {
        runCatching {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) {
                stopForeground(STOP_FOREGROUND_REMOVE)
            } else {
                @Suppress("DEPRECATION")
                stopForeground(true)
            }
        }
    }

    private fun ensureNotificationChannel() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
        val manager = getSystemService(NotificationManager::class.java) ?: return
        if (manager.getNotificationChannel(notificationChannelId) != null) return
        manager.createNotificationChannel(
            NotificationChannel(
                notificationChannelId,
                getString(R.string.keytao_deploy_channel_name),
                NotificationManager.IMPORTANCE_LOW,
            )
        )
    }

    private fun buildNotification(): Notification {
        val builder = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            Notification.Builder(this, notificationChannelId)
        } else {
            @Suppress("DEPRECATION")
            Notification.Builder(this)
        }
        return builder
            .setContentTitle(getString(R.string.keytao_deploy_notification_title))
            .setContentText(getString(R.string.keytao_deploy_notification_text))
            .setSmallIcon(android.R.drawable.stat_notify_sync)
            .setOngoing(true)
            .build()
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
        private const val notificationChannelId = "keytao_rime_deploy"
        private const val notificationId = 1001
    }
}
