package ink.rea.keytao_app

import android.app.ActivityManager
import android.content.Context
import android.os.Process
import android.system.ErrnoException
import android.system.Os
import android.system.OsConstants
import java.io.File
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

/** Migration errors gate storage; deployment errors only describe a deployment attempt. */
internal class StorageMigrationState {
    private var completed = false
    @Volatile var error: String? = null
        private set
    @Volatile var deployError: String? = null
        private set

    fun prepare(retryFailure: Boolean, migrate: () -> Unit, postDeploy: () -> Unit) {
        if (completed || (error != null && !retryFailure)) return
        error = null
        try {
            migrate()
            completed = true
        } catch (failure: Exception) {
            error = "数据迁移失败：${failure.message ?: failure.javaClass.simpleName}"
            return
        }
        try {
            postDeploy()
        } catch (failure: Exception) {
            deployError = failure.message ?: "Android RIME 部署失败"
        }
    }

    fun requireRoot(resolve: () -> File): File {
        error?.let { throw IllegalStateException(it) }
        return resolve()
    }

    fun deploymentSucceeded() { deployError = null }
}

/** Only MainActivity / app plugin commands may run migration. Never called by the IME. */
internal object KeytaoStorageMigration {
    private val mutex = Any()
    private val state = StorageMigrationState()
    val error: String? get() = state.error
    val deployError: String? get() = state.deployError

    private fun oldRoots(context: Context): List<File> = listOf(
        File(requireNotNull(context.getExternalFilesDir(null)) { "存储尚未就绪" }, "keytao"),
        File(context.filesDir, "keytao"),
    )

    fun blocksRoot(context: Context, root: File): Boolean = runCatching {
        KeytaoStorageMigrationFiles.needsCleanup(root) ||
            oldRoots(context).any { KeytaoStorageMigrationFiles.snapshot(it).hasContent }
    }.getOrDefault(true)

    fun start(context: Context) {
        Thread({ prepare(context.applicationContext, retryFailure = true) }, "KeyTao-Migrate").start()
    }

    /** Blocking app-only gate shared by startup and commands; callers use a background thread. */
    fun prepare(context: Context, retryFailure: Boolean = false) = synchronized(mutex) {
        if (KeytaoAndroidPaths.testRootOverride != null) return@synchronized
        if (!KeytaoAndroidPaths.isUnlockedAndMounted(context) || !KeytaoAndroidPaths.hasStorageAccess(context)) {
            return@synchronized
        }
        val root = KeytaoAndroidPaths.sharedRoot()
        state.prepare(retryFailure, migrate = {
            val manager = requireNotNull(context.getSystemService(ActivityManager::class.java))
            val ownProcess = manager.runningAppProcesses?.firstOrNull { it.pid == Process.myPid() }
            check(ownProcess?.processName == context.packageName) { "Migration requires the app process" }
            KeytaoStorageMigrationFiles.migrate(
                root, oldRoots(context), SimpleDateFormat("yyyyMMdd-HHmmss", Locale.ROOT).format(Date()),
                stopConsumers = { stopConsumers(context, manager) },
                hasInstalledSchema = KeytaoAndroidPaths::hasInstalledSchema,
                syncDirectory = ::syncDirectory,
            )
            KeytaoAndroidPaths.retryResolution()
        }, postDeploy = {
            val pendingDeploy = File(root, KeytaoStorageMigrationFiles.deployMarker)
            if (pendingDeploy.exists()) {
                if (KeytaoAndroidPaths.hasInstalledSchema(root)) {
                    val done = CountDownLatch(1)
                    var result: KeytaoRimeDeployClient.Result? = null
                    val schemas = sequenceOf("default.custom.yaml", "default-custom.yaml")
                        .map { File(root, it) }.firstOrNull { it.isFile }?.readText()?.let(::parseSchemas).orEmpty()
                    val timeout = KeytaoRimeDeployClient.timeoutMsForSchemas(schemas)
                    KeytaoRimeDeployClient.deploy(context, timeoutMs = timeout) { result = it; done.countDown() }
                    check(done.await(timeout + 5_000L, TimeUnit.MILLISECONDS)) { "迁移后部署超时" }
                    check(result?.success == true) { result?.error ?: "迁移后部署失败" }
                }
                check(pendingDeploy.delete()) { "Cannot clear pending migration deployment" }
            }
        })
    }

    fun requireRoot(context: Context): File {
        if (!KeytaoAndroidPaths.hasStorageAccess(context)) throw IllegalStateException("需要文件访问权限")
        prepare(context)
        return state.requireRoot { KeytaoAndroidPaths.userRoot(context) }
    }

    fun deploymentSucceeded(root: File) = synchronized(mutex) {
        File(root, KeytaoStorageMigrationFiles.deployMarker).delete()
        state.deploymentSucceeded()
    }

    private fun syncDirectory(directory: File) {
        try {
            val fd = Os.open(directory.absolutePath, OsConstants.O_RDONLY, 0)
            try {
                Os.fsync(fd)
            } finally {
                Os.close(fd)
            }
        } catch (failure: ErrnoException) {
            if (failure.errno != OsConstants.EINVAL && failure.errno != OsConstants.EROFS) throw failure
        }
    }

    private fun stopConsumers(context: Context, manager: ActivityManager) {
        val names = setOf("${context.packageName}:ime", "${context.packageName}:rime_deployer")
        fun consumers() = requireNotNull(manager.runningAppProcesses) { "Cannot inspect app processes" }
            .filter { it.uid == Process.myUid() && it.pid != Process.myPid() && it.processName in names }
        val deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(5)
        while (true) {
            val running = consumers()
            if (running.isEmpty()) return
            check(System.nanoTime() < deadline) { "Cannot stop IME/deployer before migration" }
            running.forEach { Process.killProcess(it.pid) }
            Thread.sleep(50)
        }
    }
}
