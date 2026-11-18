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
import android.os.ResultReceiver
import android.os.SystemClock
import android.util.Log
import java.io.File

class KeytaoRimeDeployService : Service() {
    private data class Request(val receiver: ResultReceiver, val timeoutMs: Long, val testRoot: String?)

    private val deployments = KeytaoRimeDeploymentQueue<Request>()
    private var lastStartId = 0
    private var deadlineMs = 0L

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        lastStartId = startId
        val receiver = intent?.resultReceiver()
        if (receiver == null) {
            if (!deployments.running) stopSelf(startId)
            return START_NOT_STICKY
        }
        val request = Request(
            receiver,
            intent.getLongExtra(KeytaoRimeDeployContract.keyTimeoutMs, 180_000L).coerceAtLeast(1_000L),
            intent.getStringExtra(KeytaoRimeDeployContract.extraTestRoot),
        )
        if (deployments.enqueue(request)) {
            startDeploymentForeground()
            startBatch()
        } else {
            // Each waiter gets the remaining current run plus its own run budget.
            sendWaiting(request, (deadlineMs - SystemClock.elapsedRealtime()).coerceAtLeast(0) + request.timeoutMs)
        }
        return START_NOT_STICKY
    }

    private fun startBatch() {
        val requests = deployments.requests
        val timeoutMs = requests.maxOf { it.timeoutMs }
        deadlineMs = SystemClock.elapsedRealtime() + timeoutMs + resultGraceMs
        requests.forEach { sendWaiting(it, timeoutMs) }
        try {
            requests.first().testRoot?.let {
                KeytaoAndroidPaths.setUserRootForTests(this, File(it))
            }
            KeytaoRimeDeployClient.runDeployment(applicationContext, timeoutMs, ::completeBatch)
        } catch (error: Throwable) {
            completeBatch(KeytaoRimeDeployClient.Result(
                success = false,
                error = error.message ?: "Android RIME 部署失败",
            ))
        }
    }

    private fun completeBatch(result: KeytaoRimeDeployClient.Result) {
        val completed = deployments.complete()
        completed.forEach { request ->
            val data = Bundle().apply {
                putString(KeytaoRimeDeployContract.keyPath, result.path)
                putString(KeytaoRimeDeployContract.keySchemaName, result.schemaName)
                putBoolean(KeytaoRimeDeployContract.keyDeployed, result.deployed)
                putString(KeytaoRimeDeployContract.keyError, result.error)
                putString(KeytaoRimeDeployContract.keyWarning, result.warning)
            }
            send(
                request.receiver,
                if (result.success) KeytaoRimeDeployContract.resultOk else KeytaoRimeDeployContract.resultError,
                data,
            )
        }
        if (deployments.running) {
            startBatch()
        } else {
            stopDeploymentForeground()
            stopSelfResult(lastStartId)
        }
    }

    private fun sendWaiting(request: Request, timeoutMs: Long) {
        send(request.receiver, KeytaoRimeDeployContract.resultWaiting, Bundle().apply {
            putLong(KeytaoRimeDeployContract.keyTimeoutMs, timeoutMs + resultGraceMs)
        })
    }

    private fun send(receiver: ResultReceiver, resultCode: Int, data: Bundle) {
        runCatching { receiver.send(resultCode, data) }
            .onFailure { Log.w(tag, "Failed to deliver deployment result", it) }
    }

    private fun startDeploymentForeground() {
        runCatching {
            ensureNotificationChannel()
            val notification = buildNotification()
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                startForeground(notificationId, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)
            } else {
                startForeground(notificationId, notification)
            }
        }.onFailure { Log.w(tag, "Failed to enter foreground for deployment", it) }
    }

    private fun stopDeploymentForeground() {
        runCatching { stopForeground(STOP_FOREGROUND_REMOVE) }
    }

    private fun ensureNotificationChannel() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
        val manager = getSystemService(NotificationManager::class.java) ?: return
        if (manager.getNotificationChannel(notificationChannelId) != null) return
        manager.createNotificationChannel(NotificationChannel(
            notificationChannelId,
            getString(R.string.keytao_deploy_channel_name),
            NotificationManager.IMPORTANCE_LOW,
        ))
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

    @Suppress("DEPRECATION")
    private fun Intent.resultReceiver(): ResultReceiver? =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            getParcelableExtra(KeytaoRimeDeployContract.extraReceiver, ResultReceiver::class.java)
        } else {
            getParcelableExtra(KeytaoRimeDeployContract.extraReceiver)
        }

    companion object {
        private const val tag = "KeytaoRimeDeploy"
        private const val resultGraceMs = 1_000L
        private const val notificationChannelId = "keytao_rime_deploy"
        private const val notificationId = 1002
    }
}
