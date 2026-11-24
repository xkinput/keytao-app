package ink.rea.keytao_app

import android.Manifest
import android.app.Activity
import android.content.ActivityNotFoundException
import android.content.ClipData
import android.content.ContentUris
import android.content.ContentValues
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.media.MediaScannerConnection
import android.net.Uri
import android.os.Build
import android.os.Environment
import android.os.Handler
import android.os.Looper
import android.provider.DocumentsContract
import android.provider.MediaStore
import android.provider.Settings
import android.view.inputmethod.InputMethodInfo
import android.view.inputmethod.InputMethodManager
import androidx.core.app.ActivityCompat
import androidx.core.content.FileProvider
import androidx.documentfile.provider.DocumentFile
import io.flutter.plugin.common.MethodCall
import io.flutter.plugin.common.MethodChannel
import java.io.File
import java.io.OutputStream
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale
import java.util.UUID
import java.util.concurrent.atomic.AtomicBoolean
import java.util.zip.ZipEntry
import java.util.zip.ZipOutputStream

class KeytaoAndroidChannel(
    private val activity: Activity,
    private val channel: MethodChannel,
) : MethodChannel.MethodCallHandler {
    private val mainHandler = Handler(Looper.getMainLooper())
    // Both legacy permission flows share one dialog without losing pending replies.
    private val pendingStoragePermissions = mutableListOf<Pair<Reply, (Reply) -> Unit>>()
    private var pendingDirectoryPick: Reply? = null
    private var pendingDirectoryImport = false

    override fun onMethodCall(call: MethodCall, result: MethodChannel.Result) {
        val reply = Reply(result, mainHandler)
        try {
            when (call.method) {
                "paths" -> paths(reply)
                "imeStatus" -> imeStatus(reply)
                "storagePermissionStatus" -> storagePermissionStatus(reply)
                "openStoragePermissionSettings" -> openStoragePermissionSettings(reply)
                "openInputMethodSettings" -> openInputMethodSettings(reply)
                "showInputMethodPicker" -> showInputMethodPicker(reply)
                "resetKeyboardLayout" -> resetKeyboardLayout(reply)
                "setKeyboardFlushBottom" -> {
                    val enabled = requireNotNull(call.argument<Boolean>("enabled"))
                    KeytaoKeyboardLayoutStateStore(activity).flushBottom = enabled
                    reply.success()
                }
                "importLocalSchemeDirectory" -> pickDirectory(reply, importLocalScheme = true)
                "smartExtractZipToPrivate" -> smartExtractZipToPrivate(call, reply)
                "prepareRimeData" -> prepareRimeData(reply)
                "deployImeData" -> deployImeData(reply)
                "adoptAppLogger" -> adoptAppLogger(reply)
                "openUrl" -> openUrl(call, reply)
                "appUpdateSource" -> reply.success(appUpdateSource())
                "openAppUpdatePackage" -> openAppUpdatePackage(call, reply)
                "shareRuntimeLog" -> shareRuntimeLog(reply)
                "shareThemeFile" -> shareThemeFile(call, reply)
                else -> reply.notImplemented()
            }
        } catch (ex: Exception) {
            reply.error(ex.message ?: "Failed to execute ${call.method}")
        }
    }

    private class Reply(
        private val result: MethodChannel.Result,
        private val handler: Handler,
    ) {
        private val completed = AtomicBoolean(false)

        private fun complete(deliver: () -> Unit) {
            if (completed.compareAndSet(false, true)) {
                handler.post { deliver() }
            }
        }

        fun success(value: Any? = null) = complete { result.success(value) }
        fun error(message: String) = complete { result.error("keytao", message, null) }
        fun notImplemented() = complete { result.notImplemented() }
    }

    private fun paths(reply: Reply) {
        KeytaoNativeBridge.configureLibjd(activity.dataDir.absolutePath)
        // These are path lookups only; never resolve or probe the shared directory.
        reply.success(mapOf(
            "dataDir" to activity.dataDir.absolutePath,
            "cacheDir" to activity.cacheDir.absolutePath,
            "keytaoRoot" to KeytaoAndroidPaths.sharedRoot().absolutePath,
        ))
    }

    private fun adoptAppLogger(reply: Reply) {
        KeytaoRuntimeLog.adoptAppLoggerIfEnabled()
        reply.success()
    }

    private fun resetKeyboardLayout(reply: Reply) {
        Thread {
            try {
                KeytaoAndroidImeConfig.resetDefaultKeyboard(activity)
                KeytaoKeyboardLayoutStateStore(activity).flushBottom = false
                reply.success()
            } catch (ex: Exception) {
                reply.error(ex.message ?: "恢复默认键盘失败")
            }
        }.start()
    }

    private fun openUrl(call: MethodCall, reply: Reply) {
        val url = call.argument<String>("url") ?: return reply.error("Missing url")
        try {
            val intent = Intent(Intent.ACTION_VIEW, Uri.parse(url))
            intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            // Starting directly also works with Android package-visibility restrictions.
            activity.startActivity(intent)
            reply.success()
        } catch (ex: Exception) {
            reply.error(ex.message ?: "Failed to open URL")
        }
    }

    private fun updatePackageArch(): String? =
        when (File(activity.applicationInfo.nativeLibraryDir).name) {
            "arm" -> "arm"
            "arm64" -> "arm64"
            "x86_64" -> "x86_64"
            else -> null
        }

    @Suppress("DEPRECATION")
    private fun appUpdateSource(): String {
        if (BuildConfig.GOOGLE_PLAY_DISTRIBUTION) return "play"
        val packageManager = activity.packageManager
        val packageName = activity.packageName
        val installer = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            packageManager.getInstallSourceInfo(packageName).installingPackageName
        } else {
            packageManager.getInstallerPackageName(packageName)
        }
        if (installer == "com.android.vending") return "play"
        val permissions = packageManager.getPackageInfo(packageName, PackageManager.GET_PERMISSIONS)
            .requestedPermissions.orEmpty()
        return if (updatePackageArch() != null && Manifest.permission.REQUEST_INSTALL_PACKAGES in permissions) {
            "package"
        } else {
            "link"
        }
    }

    private fun openAppUpdatePackage(call: MethodCall, reply: Reply) {
        val path = requireNotNull(call.argument<String>("path"))
        val version = requireNotNull(call.argument<String>("version"))
        require(Regex("[0-9]+\\.[0-9]+\\.[0-9]+(?:-[0-9A-Za-z.-]+)?").matches(version)) {
            "升级版本无效"
        }
        Thread {
            try {
                check(appUpdateSource() == "package") { "当前安装来源不支持打开升级包" }
                val file = File(path)
                val name = "keytao-app-$version-android-${updatePackageArch()}-release.apk"
                val cache = activity.cacheDir
                val expected = File(cache, "app-updates/$name")
                require(file == expected &&
                    file.canonicalFile == File(cache.canonicalFile, "app-updates/$name") &&
                    file.isFile) { "升级包路径无效" }
                verifyAppUpdatePackage(file, version)

                mainHandler.post {
                    try {
                        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O &&
                            !activity.packageManager.canRequestPackageInstalls()) {
                            val source = Uri.fromParts("package", activity.packageName, null)
                            activity.startActivity(Intent(Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES, source))
                            reply.success(false)
                        } else {
                            val uri = FileProvider.getUriForFile(
                                activity, "${activity.packageName}.fileprovider", file,
                            )
                            activity.startActivity(Intent(Intent.ACTION_VIEW).apply {
                                setDataAndType(uri, "application/vnd.android.package-archive")
                                addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
                            })
                            reply.success(true)
                        }
                    } catch (error: Exception) {
                        reply.error(error.message ?: "无法打开升级包")
                    }
                }
            } catch (error: Exception) {
                reply.error(error.message ?: "无法验证升级包")
            }
        }.start()
    }

    @Suppress("DEPRECATION")
    private fun verifyAppUpdatePackage(file: File, version: String) {
        val packageManager = activity.packageManager
        val useSigningInfo = Build.VERSION.SDK_INT >= Build.VERSION_CODES.P
        val flags = if (useSigningInfo) PackageManager.GET_SIGNING_CERTIFICATES else PackageManager.GET_SIGNATURES
        val archive = requireNotNull(packageManager.getPackageArchiveInfo(file.absolutePath, flags)) {
            "升级包无法读取"
        }
        val installed = packageManager.getPackageInfo(activity.packageName, flags)
        require(archive.packageName == installed.packageName && archive.versionName == version &&
            archive.versionName != installed.versionName) { "升级包应用标识或版本不匹配" }
        val archiveCode = if (useSigningInfo) archive.longVersionCode else archive.versionCode.toLong()
        val installedCode = if (useSigningInfo) installed.longVersionCode else installed.versionCode.toLong()
        // Prereleases and their final release share the same version code.
        require(archiveCode >= installedCode) { "升级包版本低于已安装版本" }
        val archiveSigners = if (useSigningInfo) archive.signingInfo?.apkContentsSigners else archive.signatures
        val installedSigners = if (useSigningInfo) installed.signingInfo?.apkContentsSigners else installed.signatures
        require(!archiveSigners.isNullOrEmpty() && !installedSigners.isNullOrEmpty() &&
            archiveSigners.toSet() == installedSigners.toSet()) { "升级包签名与已安装应用不一致" }
    }

    private fun requestStoragePermission(reply: Reply, callback: (Reply) -> Unit) {
        val request = reply to callback
        pendingStoragePermissions.add(request)
        if (pendingStoragePermissions.size > 1) return
        try {
            ActivityCompat.requestPermissions(
                activity,
                arrayOf(Manifest.permission.WRITE_EXTERNAL_STORAGE),
                storagePermissionRequestCode,
            )
        } catch (ex: Exception) {
            pendingStoragePermissions.remove(request)
            throw ex
        }
    }

    fun onRequestPermissionsResult(requestCode: Int): Boolean {
        if (requestCode != storagePermissionRequestCode) return false
        val pending = pendingStoragePermissions.toList()
        pendingStoragePermissions.clear()
        for ((reply, callback) in pending) {
            try {
                // Re-check the actual permission, matching the original callbacks.
                callback(reply)
            } catch (ex: Exception) {
                reply.error(ex.message ?: "Failed to open Android storage permission settings")
            }
        }
        return true
    }

    private fun imeStatus(reply: Reply) {
        try {
            reply.success(resolveImeStatus())
        } catch (ex: Exception) {
            reply.error(ex.message ?: "Failed to read Android input method status")
        }
    }

    private fun shareRuntimeLog(reply: Reply) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q &&
            activity.checkSelfPermission(Manifest.permission.WRITE_EXTERNAL_STORAGE) != PackageManager.PERMISSION_GRANTED
        ) {
            requestStoragePermission(reply, ::handleLogExportPermission)
            return
        }
        prepareRuntimeLogShare(reply)
    }

    private fun shareThemeFile(call: MethodCall, reply: Reply) {
        val fileName = call.argument<String>("fileName") ?: return reply.error("缺少主题文件名")
        val json = call.argument<String>("json") ?: return reply.error("缺少主题内容")
        val id = fileName.removeSuffix(".keytao-theme.json")
        require(fileName.endsWith(".keytao-theme.json") && id.length in 3..64 &&
            id.matches(Regex("[a-z0-9][a-z0-9_-]*(\\.[a-z0-9_-]+)*"))) { "主题文件名无效" }
        val bytes = json.toByteArray(Charsets.UTF_8)
        require(bytes.size <= 64 * 1024) { "主题内容超过 64 KiB" }
        val directory = File(activity.cacheDir, "theme-share/${UUID.randomUUID()}")
        check(directory.mkdirs()) { "无法创建主题分享文件" }
        val file = File(directory, fileName)
        try {
            file.writeBytes(bytes)
            val uri = runtimeLogFileUri(file)
            val intent = Intent(Intent.ACTION_SEND).apply {
                type = "application/json"
                putExtra(Intent.EXTRA_STREAM, uri)
                clipData = ClipData.newUri(activity.contentResolver, fileName, uri)
                addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
            }
            activity.startActivity(Intent.createChooser(intent, "分享主题").apply {
                addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
            })
            reply.success()
        } catch (ex: Exception) {
            file.delete()
            directory.delete()
            throw ex
        }
    }

    private fun handleLogExportPermission(reply: Reply) {
        if (activity.checkSelfPermission(Manifest.permission.WRITE_EXTERNAL_STORAGE) != PackageManager.PERMISSION_GRANTED) {
            reply.error("保存到下载目录需要存储权限")
            return
        }
        prepareRuntimeLogShare(reply)
    }

    private fun prepareRuntimeLogShare(reply: Reply) {
        Thread {
            try {
                cleanOldRuntimeLogExports()
                val logDir = File(KeytaoAndroidPaths.requireRoot(activity), "log")
                val logFiles = logDir.listFiles()
                    ?.filter { it.isFile && it.name.matches(Regex("""keytao-.*\.log.*""")) }
                    ?.sortedBy { it.name }
                    .orEmpty()
                if (logFiles.isEmpty()) {
                    return@Thread reply.error("No runtime logs available")
                }
                val name = "keytao-runtime-log-${SimpleDateFormat("yyyyMMdd-HHmm", Locale.ROOT).format(Date())}.zip"
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                    val values = ContentValues().apply {
                        put(MediaStore.MediaColumns.DISPLAY_NAME, name)
                        put(MediaStore.MediaColumns.MIME_TYPE, "application/zip")
                        put(MediaStore.MediaColumns.RELATIVE_PATH, "Download/KeyTao")
                        put(MediaStore.MediaColumns.IS_PENDING, 1)
                    }
                    val resolver = activity.contentResolver
                    // Only insertion failure uses the cache/FileProvider fallback.
                    val uri = runCatching {
                        resolver.insert(MediaStore.Downloads.EXTERNAL_CONTENT_URI, values)
                    }.getOrNull()
                    if (uri == null) {
                        val cacheDir = File(activity.cacheDir, "runtime-log-share")
                        val archive = writeRuntimeLogFile(cacheDir, name, logFiles)
                        presentRuntimeLogShare(reply, archive.absolutePath, runtimeLogFileUri(archive))
                        return@Thread
                    }
                    val savedName = try {
                        val output = resolver.openOutputStream(uri, "w")
                            ?: throw IllegalStateException("Failed to open runtime log export")
                        writeRuntimeLogZip(output, logFiles)
                        // MediaStore can rename a second export made in the same minute.
                        val actualName = resolver.query(uri, arrayOf(MediaStore.MediaColumns.DISPLAY_NAME), null, null, null)
                            ?.use { cursor -> if (cursor.moveToFirst()) cursor.getString(0) else null }
                            ?: throw IllegalStateException("Failed to read saved runtime log name")
                        val published = resolver.update(uri, ContentValues().apply {
                            put(MediaStore.MediaColumns.IS_PENDING, 0)
                        }, null, null)
                        check(published == 1) { "Failed to publish runtime log export" }
                        actualName
                    } catch (ex: Exception) {
                        runCatching { resolver.delete(uri, null, null) }
                        throw ex
                    }
                    presentRuntimeLogShare(reply, "Download/KeyTao/$savedName", uri)
                } else {
                    val directory = File(Environment.getExternalStoragePublicDirectory(Environment.DIRECTORY_DOWNLOADS), "KeyTao")
                    val archive = writeRuntimeLogFile(directory, name, logFiles)
                    MediaScannerConnection.scanFile(activity, arrayOf(archive.absolutePath), arrayOf("application/zip")) { _, uri ->
                        try {
                            // A null scan URI means the legacy MediaStore insertion failed.
                            presentRuntimeLogShare(reply, "Download/KeyTao/${archive.name}", uri ?: runtimeLogFileUri(archive))
                        } catch (ex: Exception) {
                            reply.error(ex.message ?: "Failed to share saved runtime logs")
                        }
                    }
                }
            } catch (ex: Exception) {
                reply.error(ex.message ?: "Failed to prepare runtime log archive")
            }
        }.start()
    }

    private fun writeRuntimeLogZip(output: OutputStream, logFiles: List<File>) {
        ZipOutputStream(output.buffered()).use { zip ->
            for (file in logFiles) {
                file.inputStream().use { input ->
                    zip.putNextEntry(ZipEntry(file.name))
                    input.copyTo(zip)
                    zip.closeEntry()
                }
            }
        }
    }

    private fun writeRuntimeLogFile(directory: File, name: String, logFiles: List<File>): File {
        check(directory.isDirectory || directory.mkdirs()) { "Failed to create runtime log export directory" }
        var archive = File(directory, name)
        var suffix = 1
        // Reserve a unique path so repeated shares never overwrite an in-flight attachment.
        while (!archive.createNewFile()) {
            archive = File(directory, "${name.removeSuffix(".zip")} (${suffix++}).zip")
        }
        try {
            writeRuntimeLogZip(archive.outputStream(), logFiles)
        } catch (ex: Exception) {
            archive.delete()
            throw ex
        }
        return archive
    }

    private fun runtimeLogFileUri(file: File): Uri = FileProvider.getUriForFile(
        activity, "${activity.packageName}.fileprovider", file,
    )

    private fun presentRuntimeLogShare(reply: Reply, path: String, uri: Uri) {
        activity.runOnUiThread {
            val result = mutableMapOf<String, Any?>().apply {
                put("path", path)
                put("uri", uri.toString())
            }
            try {
                val intent = Intent(Intent.ACTION_SEND).apply {
                    type = "application/zip"
                    putExtra(Intent.EXTRA_STREAM, uri)
                    clipData = ClipData.newUri(activity.contentResolver, "KeyTao runtime log", uri)
                    addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
                }
                val chooser = Intent.createChooser(intent, "分享运行日志").apply {
                    addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
                }
                activity.startActivity(chooser)
            } catch (ex: Exception) {
                // Keep the saved file and its path available even without a share target.
                result.put("shareError", ex.message ?: "Failed to open runtime log share sheet")
            }
            reply.success(result)
        }
    }

    private fun cleanOldRuntimeLogExports() {
        val cutoff = System.currentTimeMillis() - 7L * 24 * 60 * 60 * 1000
        val exportName = Regex("""keytao-runtime-log-\d{8}-\d{4}( \(\d+\))?\.zip""")
        // Retention is best effort; inaccessible files must not block a fresh export.
        runCatching {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                val collection = MediaStore.Downloads.EXTERNAL_CONTENT_URI
                val resolver = activity.contentResolver
                resolver.query(collection, arrayOf(MediaStore.MediaColumns._ID, MediaStore.MediaColumns.DISPLAY_NAME),
                    "${MediaStore.MediaColumns.RELATIVE_PATH} IN (?, ?) AND ${MediaStore.MediaColumns.DATE_ADDED} < ? AND ${MediaStore.MediaColumns.OWNER_PACKAGE_NAME} = ?",
                    arrayOf("Download/KeyTao", "Download/KeyTao/", (cutoff / 1000).toString(), activity.packageName), null,
                )?.use { cursor ->
                    while (cursor.moveToNext()) {
                        if (exportName.matches(cursor.getString(1).orEmpty())) {
                            runCatching { resolver.delete(ContentUris.withAppendedId(collection, cursor.getLong(0)), null, null) }
                        }
                    }
                }
            } else {
                val directory = File(Environment.getExternalStoragePublicDirectory(Environment.DIRECTORY_DOWNLOADS), "KeyTao")
                directory.listFiles()?.filter { it.isFile && exportName.matches(it.name) && it.lastModified() < cutoff }
                    ?.forEach { file ->
                        if (file.delete()) {
                            runCatching { activity.contentResolver.delete(MediaStore.Files.getContentUri("external"),
                                "${MediaStore.MediaColumns.DATA} = ?", arrayOf(file.absolutePath)) }
                        }
                    }
            }
        }
        File(activity.cacheDir, "runtime-log-share").listFiles()
            ?.filter { it.isFile && exportName.matches(it.name) && it.lastModified() < cutoff }
            ?.forEach { it.delete() }
    }

    private fun storagePermissionStatus(reply: Reply) {
        Thread {
            try {
                reply.success(resolveStoragePermissionStatus())
            } catch (ex: Exception) {
                reply.error(ex.message ?: "Failed to read Android storage permission status")
            }
        }.start()
    }

    private fun openStoragePermissionSettings(reply: Reply) {
        try {
            if (KeytaoAndroidPaths.hasStorageAccess(activity)) {
                KeytaoAndroidPaths.retryResolution()
                reply.success()
                return
            }
            if (Build.VERSION.SDK_INT < Build.VERSION_CODES.R) {
                val preferences = activity.getSharedPreferences("keytao-storage-permission", Context.MODE_PRIVATE)
                if (preferences.getBoolean("requested", false) &&
                    !activity.shouldShowRequestPermissionRationale(Manifest.permission.WRITE_EXTERNAL_STORAGE)) {
                    openApplicationDetailsSettings()
                    reply.success()
                    return
                }
                preferences.edit().putBoolean("requested", true).apply()
                requestStoragePermission(reply, ::handleStoragePermission)
                return
            }
            openStoragePermissionSettings()
            reply.success()
        } catch (ex: Exception) {
            reply.error(ex.message ?: "Failed to open Android storage permission settings")
        }
    }

    private fun handleStoragePermission(reply: Reply) {
        try {
            if (KeytaoAndroidPaths.hasStorageAccess(activity)) {
                KeytaoAndroidPaths.retryResolution()
            } else if (!activity.shouldShowRequestPermissionRationale(Manifest.permission.WRITE_EXTERNAL_STORAGE)) {
                openApplicationDetailsSettings()
            }
            reply.success()
        } catch (ex: Exception) {
            reply.error(ex.message ?: "Failed to open Android storage permission settings")
        }
    }

    private fun openInputMethodSettings(reply: Reply) {
        try {
            val intent = Intent(Settings.ACTION_INPUT_METHOD_SETTINGS)
            intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            activity.startActivity(intent)
            reply.success()
        } catch (ex: Exception) {
            reply.error(ex.message ?: "Failed to open Android input method settings")
        }
    }

    private fun showInputMethodPicker(reply: Reply) {
        try {
            val status = resolveImeStatus()
            if (status["enabled"] != true) {
                return reply.error("KeyTao 输入法尚未启用")
            }
            val imm = inputMethodManager()
                ?: return reply.error("InputMethodManager is unavailable")
            imm.showInputMethodPicker()
            reply.success()
        } catch (ex: Exception) {
            reply.error(ex.message ?: "Failed to show Android input method picker")
        }
    }

    private fun pickDirectory(reply: Reply, importLocalScheme: Boolean = false) {
        if (pendingDirectoryPick != null) {
            return reply.error("Directory picker is already open")
        }
        pendingDirectoryPick = reply
        pendingDirectoryImport = importLocalScheme
        try {
            activity.startActivityForResult(Intent(Intent.ACTION_OPEN_DOCUMENT_TREE), directoryPickRequestCode)
        } catch (ex: Exception) {
            pendingDirectoryPick = null
            pendingDirectoryImport = false
            throw ex
        }
    }

    fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?): Boolean {
        if (requestCode != directoryPickRequestCode) return false
        val reply = pendingDirectoryPick ?: return false
        val importLocalScheme = pendingDirectoryImport
        pendingDirectoryPick = null
        pendingDirectoryImport = false
        try {
            if (importLocalScheme) {
                importLocalSchemeDirectory(reply, resultCode, data)
            } else {
                handleDirectoryPicked(reply, resultCode, data)
            }
        } catch (ex: Exception) {
            reply.error(ex.message ?: "Failed to pick directory")
        }
        return true
    }

    private fun importLocalSchemeDirectory(reply: Reply, resultCode: Int, data: Intent?) {
        if (resultCode != Activity.RESULT_OK) return reply.success(null)
        val uri = data?.data ?: return reply.error("未返回所选目录")
        Thread {
            val temporaryRoot = File(activity.cacheDir, "local-scheme-${UUID.randomUUID()}")
            try {
                val root = DocumentFile.fromTreeUri(activity, uri)
                    ?: throw IllegalStateException("无法读取所选目录")
                val name = root.name ?: throw IllegalStateException("无法读取目录名称")
                validateDirectoryEntryName(name)
                check(temporaryRoot.mkdir()) { "无法创建临时目录" }
                val destination = safePrivateFile(temporaryRoot, name)
                check(destination.mkdir()) { "无法创建临时目录" }
                copyTreeDirectory(uri, DocumentsContract.getTreeDocumentId(uri), destination, mutableSetOf())
                reply.success(mapOf(
                    "path" to destination.absolutePath,
                    "name" to name,
                    "temporaryDirectory" to temporaryRoot.absolutePath,
                ))
            } catch (ex: Exception) {
                temporaryRoot.deleteRecursively()
                reply.error(ex.message ?: "无法导入所选目录")
            }
        }.start()
    }

    private fun copyTreeDirectory(
        treeUri: Uri,
        documentId: String,
        destination: File,
        visited: MutableSet<String>,
        totalBytes: Long = 0,
        topLevel: Boolean = true,
    ): Long {
        var copiedBytes = totalBytes
        check(visited.add(documentId)) { "所选目录包含循环引用" }
        val children = DocumentsContract.buildChildDocumentsUriUsingTree(treeUri, documentId)
        val resolver = activity.contentResolver
        val cursor = resolver.query(children, arrayOf(
            DocumentsContract.Document.COLUMN_DOCUMENT_ID,
            DocumentsContract.Document.COLUMN_DISPLAY_NAME,
            DocumentsContract.Document.COLUMN_MIME_TYPE,
        ), null, null, null) ?: throw IllegalStateException("无法读取所选目录")
        cursor.use {
            while (cursor.moveToNext()) {
                val childId = cursor.getString(0) ?: throw IllegalStateException("无法读取目录项")
                val name = cursor.getString(1) ?: throw IllegalStateException("无法读取目录项名称")
                validateDirectoryEntryName(name)
                if ((topLevel && name in setOf("build", "sync", "installation.yaml", "user.yaml")) ||
                    name in setOf(".git", ".DS_Store", "default-custom.yaml", "user_dict.txt", ".keytao-active-scheme.json") ||
                    name.endsWith(".userdb") || name.endsWith(".custom.yaml") ||
                    name.endsWith(".user.dict.yaml") || name.endsWith(".user.yaml") ||
                    name.endsWith("_user.txt") ||
                    (name.startsWith(".keytao-sync-base-") && name.endsWith(".json"))) continue
                val output = safePrivateFile(destination, name)
                if (cursor.getString(2) == DocumentsContract.Document.MIME_TYPE_DIR) {
                    check(output.mkdir()) { "无法创建目录：$name" }
                    copiedBytes = copyTreeDirectory(treeUri, childId, output, visited, copiedBytes, false)
                } else {
                    check(output.createNewFile()) { "目录中存在重名文件：$name" }
                    val childUri = DocumentsContract.buildDocumentUriUsingTree(treeUri, childId)
                    val input = resolver.openInputStream(childUri)
                        ?: throw IllegalStateException("无法读取文件：$name")
                    input.use { source ->
                        output.outputStream().use { target ->
                            val buffer = ByteArray(8192)
                            while (true) {
                                val count = source.read(buffer)
                                if (count < 0) break
                                check(count.toLong() <= 512L * 1024 * 1024 - copiedBytes) { "方案包解压后过大" }
                                copiedBytes += count
                                target.write(buffer, 0, count)
                            }
                        }
                    }
                }
            }
        }
        return copiedBytes
    }

    private fun validateDirectoryEntryName(name: String) {
        require(name.isNotEmpty() && name != "." && name != ".." &&
            !name.contains('/') && !name.contains('\\') && !name.contains('\u0000')) {
            "目录中存在无效文件名"
        }
    }

    private fun handleDirectoryPicked(reply: Reply, resultCode: Int, data: Intent?) {
        if (resultCode == Activity.RESULT_OK) {
            val uri = data?.data ?: return reply.error("No URI returned")
            activity.contentResolver.takePersistableUriPermission(
                uri,
                Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION
            )
            reply.success(mapOf("uri" to uri.toString()))
        } else {
            reply.error("User cancelled")
        }
    }

    private fun smartExtractZipToPrivate(call: MethodCall, reply: Reply) {
        val zipPath = call.argument<String>("zipPath") ?: return reply.error("Missing zipPath")
        val root = KeytaoAndroidPaths.requireRoot(activity)
        if (!KeytaoAndroidPaths.isWritable(root)) {
            return reply.error("无法写入 ${root.absolutePath}，请检查设备存储空间后重试")
        }
        // Core owns extraction, generated files, and rollback as one transaction.
        reply.success(mapOf(
            "coreInstall" to true,
            "packageZipPath" to zipPath,
            "packageRoot" to root.absolutePath,
        ))
    }

    private fun prepareRimeData(reply: Reply) {
        Thread {
            try {
                val root = KeytaoAndroidPaths.requireRoot(activity)
                KeytaoImeEngine.prepareBundledSharedData(activity, root)
                reply.success()
            } catch (ex: Exception) {
                reply.error(ex.message ?: "Android RIME 资源准备失败")
            }
        }.start()
    }

    private fun deployImeData(reply: Reply) {
        Thread {
            try {
                val root = KeytaoAndroidPaths.requireRoot(activity)
                KeytaoImeEngine.prepareBundledSharedData(activity, root)
                KeytaoRimeDeployClient.deploy(activity) { result ->
                    try {
                        if (result.success) {
                            reply.success(mutableMapOf<String, Any?>().apply {
                                put("path", result.path)
                                put("schemaName", result.schemaName)
                                put("deployed", result.deployed)
                                put("warning", result.warning)
                            })
                        } else {
                            reply.error(result.error.ifBlank { "Android RIME 部署失败" })
                        }
                    } catch (ex: Exception) {
                        reply.error(ex.message ?: "Android RIME 部署失败")
                    }
                }
            } catch (ex: Exception) {
                reply.error(ex.message ?: "Android RIME 部署失败")
            }
        }.start()
    }

    companion object {
        private const val storagePermissionRequestCode = 0x4B54
        private const val directoryPickRequestCode = 0x4B55
    }

    private fun safePrivateFile(root: File, relativePath: String): File {
        val rootPath = root.canonicalFile.toPath()
        val output = File(root, relativePath).canonicalFile
        if (!output.toPath().startsWith(rootPath)) {
            throw Exception("Unsafe zip entry: $relativePath")
        }
        return output
    }

    private fun inputMethodManager(): InputMethodManager? {
        return activity.getSystemService(Context.INPUT_METHOD_SERVICE) as? InputMethodManager
    }

    private fun isKeytaoInputMethod(info: InputMethodInfo): Boolean {
        val serviceName = KeytaoInputMethodService::class.java.name
        return info.packageName == activity.packageName &&
            (info.serviceName == serviceName || info.serviceName.endsWith(".KeytaoInputMethodService"))
    }

    private fun resolveImeStatus(): Map<String, Any?> {
        val imm = inputMethodManager()
        val installedInfo = imm?.inputMethodList?.firstOrNull(::isKeytaoInputMethod)
        val enabledInfo = imm?.enabledInputMethodList?.firstOrNull(::isKeytaoInputMethod)
        val info = installedInfo ?: enabledInfo
        val serviceName = KeytaoInputMethodService::class.java.name
        val inputMethodId = info?.id
        val defaultInputMethod = Settings.Secure.getString(
            activity.contentResolver,
            Settings.Secure.DEFAULT_INPUT_METHOD
        )
        val enabled = enabledInfo != null
        val selected = enabled && inputMethodId != null && inputMethodId == defaultInputMethod
        val message = when {
            selected -> "KeyTao 输入法已启用并正在使用"
            enabled -> "KeyTao 输入法已启用，尚未切换为当前输入法"
            installedInfo != null -> "KeyTao 输入法已随应用安装，尚未在系统中启用"
            else -> "系统尚未识别 KeyTao 输入法服务"
        }

        return mutableMapOf<String, Any?>().apply {
            put("packageName", activity.packageName)
            put("serviceName", serviceName)
            if (inputMethodId != null) put("inputMethodId", inputMethodId)
            if (defaultInputMethod != null) put("defaultInputMethod", defaultInputMethod)
            put("enabled", enabled)
            put("selected", selected)
            put("canShowPicker", enabled)
            put("keyboardFlushBottom", KeytaoKeyboardLayoutStateStore(activity).flushBottom)
            put("message", message)
        }
    }

    private fun resolveStoragePermissionStatus(): Map<String, Any?> {
        val granted = KeytaoAndroidPaths.hasStorageAccess(activity)
        val writable = KeytaoAndroidPaths.userRootOrNull(activity) != null
        return mutableMapOf<String, Any?>().apply {
            put("path", "/sdcard/keytao")
            put("granted", granted)
            put("writable", writable)
            put("requiresManageAllFiles", Build.VERSION.SDK_INT >= Build.VERSION_CODES.R)
            put("canOpenSettings", true)
            put("message", if (granted) "" else "需要文件访问权限")
        }
    }

    private fun openStoragePermissionSettings() {
        val packageUri = Uri.parse("package:${activity.packageName}")
        val intent = Intent(Settings.ACTION_MANAGE_APP_ALL_FILES_ACCESS_PERMISSION, packageUri)
        try {
            activity.startActivity(intent)
        } catch (_: ActivityNotFoundException) {
            try {
                activity.startActivity(Intent(Settings.ACTION_MANAGE_ALL_FILES_ACCESS_PERMISSION))
            } catch (_: ActivityNotFoundException) {
                openApplicationDetailsSettings()
            }
        }
    }

    private fun openApplicationDetailsSettings() {
        activity.startActivity(Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS,
            Uri.parse("package:${activity.packageName}")))
    }

}
