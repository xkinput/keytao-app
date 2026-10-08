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
import java.io.FileOutputStream
import java.io.OutputStream
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale
import java.util.concurrent.atomic.AtomicBoolean
import java.util.zip.ZipFile as JZipFile
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
                "pickDirectory" -> pickDirectory(reply)
                "listFiles" -> listFiles(call, reply)
                "readLocalSchemas" -> readLocalSchemas(call, reply)
                "smartExtractZip" -> smartExtractZip(call, reply)
                "smartExtractZipToPrivate" -> smartExtractZipToPrivate(call, reply)
                "copyAddonSchemaAssets" -> copyAddonSchemaAssets(call, reply)
                "deployImeData" -> deployImeData(reply)
                "adoptAppLogger" -> adoptAppLogger(reply)
                "openUrl" -> openUrl(call, reply)
                "shareRuntimeLog" -> shareRuntimeLog(reply)
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

    private fun pickDirectory(reply: Reply) {
        if (pendingDirectoryPick != null) {
            return reply.error("Directory picker is already open")
        }
        pendingDirectoryPick = reply
        try {
            activity.startActivityForResult(Intent(Intent.ACTION_OPEN_DOCUMENT_TREE), directoryPickRequestCode)
        } catch (ex: Exception) {
            pendingDirectoryPick = null
            throw ex
        }
    }

    fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?): Boolean {
        if (requestCode != directoryPickRequestCode) return false
        val reply = pendingDirectoryPick ?: return false
        pendingDirectoryPick = null
        try {
            handleDirectoryPicked(reply, resultCode, data)
        } catch (ex: Exception) {
            reply.error(ex.message ?: "Failed to pick directory")
        }
        return true
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

    private fun listFiles(call: MethodCall, reply: Reply) {
        val treeUriString = call.argument<String>("treeUri") ?: return reply.error("Missing treeUri")

        try {
            val uri = Uri.parse(treeUriString)
            val root = DocumentFile.fromTreeUri(activity, uri)
                ?: return reply.error("Invalid directory URI")
            val items = root.listFiles()
                .map { Pair(it.name ?: "", it.isDirectory) }
                .sortedWith(compareByDescending<Pair<String, Boolean>> { it.second }.thenBy { it.first })
            val files = items.map { (name, isDir) -> mapOf("name" to name, "isDir" to isDir) }
            reply.success(mapOf("files" to files))
        } catch (ex: Exception) {
            reply.error(ex.message ?: "Failed to list files")
        }
    }

    private fun readLocalSchemas(call: MethodCall, reply: Reply) {
        val treeUriString = call.argument<String>("treeUri") ?: return reply.error("Missing treeUri")

        try {
            val uri = Uri.parse(treeUriString)
            val root = DocumentFile.fromTreeUri(activity, uri)
                ?: return reply.error("Invalid directory URI")
            val content = readFileFromTree(root, "default.custom.yaml")
                ?: readFileFromTree(root, "default-custom.yaml")
                ?: ""
            val schemas = parseSchemas(content).filter(::isManagedSchema)
            val version = readFileFromTree(root, "version.txt")?.trim()?.takeIf { it.isNotEmpty() }
            val installed = schemas.isNotEmpty() && schemas.all { schema ->
                root.findFile("$schema.schema.yaml")?.isFile == true
            }
            val build = root.findFile("build")?.takeIf { it.isDirectory }
            val deployed = installed && build != null && schemas.all { schema ->
                build.findFile("$schema.schema.yaml")?.isFile == true
            }
            reply.success(mutableMapOf<String, Any?>().apply {
                put("schemas", schemas)
                if (version != null) put("version", version)
                put("installed", installed)
                put("deployed", deployed)
            })
        } catch (ex: Exception) {
            reply.success(mapOf("schemas" to emptyList<String>(), "installed" to false, "deployed" to false))
        }
    }

    private fun smartExtractZip(call: MethodCall, reply: Reply) {
        val zipPath = call.argument<String>("zipPath") ?: return reply.error("Missing zipPath")
        val treeUriString = call.argument<String>("treeUri") ?: return reply.error("Missing treeUri")

        Thread {
            try {
                val uri = Uri.parse(treeUriString)
                val root = DocumentFile.fromTreeUri(activity, uri)
                    ?: return@Thread reply.error("Invalid directory URI")
                val zipFile = File(zipPath)
                val logs = mutableListOf<String>()

                // Cache directories and load their file URIs in one query per directory.
                val dirCache = mutableMapOf<String, DocumentFile>()
                val fileUriCache = mutableMapOf<String, MutableMap<String, Uri>>()
                fun getCachedFileMap(dir: DocumentFile): MutableMap<String, Uri> {
                    return fileUriCache.getOrPut(dir.uri.toString()) {
                        val map = mutableMapOf<String, Uri>()
                        val docId = if (DocumentsContract.isTreeUri(dir.uri) && !DocumentsContract.isDocumentUri(activity, dir.uri))
                            DocumentsContract.getTreeDocumentId(dir.uri)
                        else
                            DocumentsContract.getDocumentId(dir.uri)
                        val childrenUri = DocumentsContract.buildChildDocumentsUriUsingTree(uri, docId)
                        activity.contentResolver.query(
                            childrenUri,
                            arrayOf(
                                DocumentsContract.Document.COLUMN_DOCUMENT_ID,
                                DocumentsContract.Document.COLUMN_DISPLAY_NAME,
                                DocumentsContract.Document.COLUMN_MIME_TYPE
                            ), null, null, null
                        )?.use { cursor ->
                            while (cursor.moveToNext()) {
                                val childDocId = cursor.getString(0) ?: continue
                                val name = cursor.getString(1) ?: continue
                                val mime = cursor.getString(2) ?: continue
                                if (mime != DocumentsContract.Document.MIME_TYPE_DIR) {
                                    map[name] = DocumentsContract.buildDocumentUriUsingTree(uri, childDocId)
                                }
                            }
                        }
                        map
                    }
                }
                fun getOrCreateFileUri(dir: DocumentFile, filename: String): Uri? {
                    val map = getCachedFileMap(dir)
                    return map[filename] ?: run {
                        val newUri = dir.createFile("application/octet-stream", filename)?.uri ?: return null
                        map[filename] = newUri
                        newUri
                    }
                }
                fun getOrCreateDir(path: String): DocumentFile {
                    if (path.isEmpty()) return root
                    dirCache[path]?.let { return it }
                    val parts = path.split('/')
                    var current = root
                    val sb = StringBuilder()
                    for (part in parts) {
                        if (part.isEmpty()) continue
                        if (sb.isNotEmpty()) sb.append('/')
                        sb.append(part)
                        val key = sb.toString()
                        current = dirCache[key] ?: run {
                            val dir = current.findFile(part)?.takeIf { it.isDirectory }
                                ?: current.createDirectory(part)
                                ?: throw Exception("Failed to create directory: $part")
                            dirCache[key] = dir
                            dir
                        }
                    }
                    return current
                }
                fun writeToDir(dir: DocumentFile, filename: String, content: ByteArray, relative: String, merged: Boolean = false) {
                    val outUri = getOrCreateFileUri(dir, filename)
                        ?: throw Exception("Failed to create: $filename")
                    val pfd = activity.contentResolver.openFileDescriptor(outUri, "rwt")
                        ?: throw Exception("Failed to open: $filename")
                    pfd.use { FileOutputStream(it.fileDescriptor).use { out -> out.write(content) } }
                    logs.add(if (merged) "[MERGED] $relative" else "[OK] $relative")
                }

                JZipFile(zipFile).use { zip ->
                    val allEntries = zip.entries().toList()
                    val totalFiles = allEntries.count { !it.isDirectory }
                    var processed = 0
                    val emitProgress: (String) -> Unit = { fname ->
                        if (totalFiles > 0) {
                            val pct = (61 + processed * 38 / totalFiles).coerceAtMost(99)
                            val payload = mapOf(
                                "stage" to "extracting",
                                "percent" to pct,
                                "message" to "正在安装... $processed/$totalFiles: $fname",
                            )
                            mainHandler.post {
                                channel.invokeMethod("installProgress", payload)
                            }
                        }
                    }
                    val zipLuaFilenames = mutableSetOf<String>()
                    var dcEntry: java.util.zip.ZipEntry? = null
                    var rimeLuaEntry: java.util.zip.ZipEntry? = null
                    for (entry in allEntries) {
                        val relative = entry.name.trimEnd('/')
                        val filename = relative.substringAfterLast('/')
                        when {
                            !entry.isDirectory && isDefaultCustom(filename) && dcEntry == null -> dcEntry = entry
                            !entry.isDirectory && filename == "rime.lua" && !relative.contains('/') && rimeLuaEntry == null -> rimeLuaEntry = entry
                            !entry.isDirectory && relative.startsWith("lua/") && !relative.substring(4).contains('/') -> zipLuaFilenames.add(filename)
                        }
                    }
                    val zipDcContent = dcEntry?.let { zip.getInputStream(it).bufferedReader().readText() }
                    val zipRimeLuaContent = rimeLuaEntry?.let { zip.getInputStream(it).bufferedReader().readText() }
                    val dcMergeResult = zipDcContent?.let {
                        val existing = readFileFromTree(root, "default.custom.yaml")
                            ?: readFileFromTree(root, "default-custom.yaml")
                        mergeDefaultCustom(existing, it)
                    }
                    val rimeLuaMergeResult = zipRimeLuaContent?.let { zipRl ->
                        val localRl = readFileFromTree(root, "rime.lua")
                        if (localRl != null) mergeRimeLua(localRl, zipRl, zipLuaFilenames)
                        else RimeLuaMergeResult(zipRl, emptyList())
                    }

                    // Preserve conflicting user Lua files before extraction overwrites them.
                    val renamedLuaFiles: List<Pair<String, ByteArray>> =
                        rimeLuaMergeResult?.renames?.mapNotNull { (oldName, newName) ->
                            val luaDir = root.findFile("lua") ?: return@mapNotNull null
                            val bytes = luaDir.findFile("$oldName.lua")?.uri?.let { fileUri ->
                                activity.contentResolver.openInputStream(fileUri)?.use { it.readBytes() }
                            } ?: return@mapNotNull null
                            newName to bytes
                        } ?: emptyList()

                    for (entry in allEntries) {
                        val relative = entry.name.trimEnd('/')
                        if (relative.isEmpty()) continue
                        val filename = relative.substringAfterLast('/')
                        val dirPart = relative.substringBeforeLast('/', "")
                        when {
                            entry.isDirectory -> {
                                try { getOrCreateDir(relative) } catch (e: Exception) {
                                    logs.add("[WARN] mkdir $relative: ${e.message}")
                                }
                            }
                            isDefaultCustom(filename) && dcMergeResult != null -> {
                                val dir = getOrCreateDir(dirPart)
                                try { writeToDir(dir, filename, dcMergeResult.mergedContent.toByteArray(), relative, merged = true) }
                                catch (e: Exception) { return@Thread reply.error(e.message ?: "Write failed: $relative") }
                                processed++; emitProgress(filename)
                            }
                            filename == "rime.lua" && !relative.contains('/') && rimeLuaMergeResult != null -> {
                                val dir = getOrCreateDir(dirPart)
                                try { writeToDir(dir, filename, rimeLuaMergeResult.mergedContent.toByteArray(), relative, merged = true) }
                                catch (e: Exception) { return@Thread reply.error(e.message ?: "Write failed: $relative") }
                                processed++; emitProgress(filename)
                            }
                            else -> {
                                val dir = try { getOrCreateDir(dirPart) } catch (e: Exception) {
                                    logs.add("[ERROR] mkdir for $relative: ${e.message}")
                                    return@Thread reply.error("Failed to create directory for: $relative")
                                }
                                val outUri = getOrCreateFileUri(dir, filename)
                                    ?: run { logs.add("[ERROR] createFile: $relative"); return@Thread reply.error("Failed to create: $filename") }
                                val pfd = activity.contentResolver.openFileDescriptor(outUri, "rwt")
                                if (pfd == null) { logs.add("[ERROR] openFileDescriptor: $relative"); return@Thread reply.error("Failed to open output stream: $relative") }
                                pfd.use { FileOutputStream(it.fileDescriptor).buffered(65536).use { out -> zip.getInputStream(entry).copyTo(out, 65536) } }
                                logs.add("[OK] $relative")
                                processed++; emitProgress(filename)
                            }
                        }
                    }

                    if (renamedLuaFiles.isNotEmpty()) {
                        val luaDir = try { getOrCreateDir("lua") } catch (e: Exception) {
                            return@Thread reply.error("Failed to ensure lua dir: ${e.message}")
                        }
                        for ((newName, bytes) in renamedLuaFiles) {
                            val fname = "$newName.lua"
                            val existing = luaDir.findFile(fname)
                            val outUri = if (existing != null) existing.uri
                            else luaDir.createFile("application/octet-stream", fname)?.uri
                            if (outUri == null) { logs.add("[ERROR] createFile for renamed: lua/$fname"); continue }
                            val pfd = activity.contentResolver.openFileDescriptor(outUri, "rwt")
                            if (pfd != null) { pfd.use { FileOutputStream(it.fileDescriptor).use { out -> out.write(bytes) } }; logs.add("[RENAMED] lua/$fname") }
                            else logs.add("[ERROR] openFileDescriptor for renamed: lua/$fname")
                        }
                    }

                    root.findFile("build")?.let { build ->
                        if (!build.delete()) {
                            return@Thread reply.error("无法清理旧的 Android RIME 部署产物")
                        }
                        logs.add("[INVALIDATED] build")
                    }

                    val verifyArray = mutableListOf<Map<String, Any?>>()
                    fun addVerify(path: String, ok: Boolean, note: String) {
                        verifyArray.add(mapOf("path" to path, "ok" to ok, "note" to note))
                    }
                    dcMergeResult?.mergedContent?.let { expected ->
                        val actual = readFileFromTree(root, "default.custom.yaml")
                        if (actual == expected) addVerify("default.custom.yaml", true, "内容一致")
                        else if (actual != null) addVerify("default.custom.yaml", false, "内容与写入时不符，可能写入不完整")
                        else addVerify("default.custom.yaml", false, "文件不存在或无法读取")
                    }
                    rimeLuaMergeResult?.mergedContent?.let { expected ->
                        val actual = readFileFromTree(root, "rime.lua")
                        if (actual == expected) addVerify("rime.lua", true, "内容一致")
                        else if (actual != null) addVerify("rime.lua", false, "内容与写入时不符，可能写入不完整")
                        else addVerify("rime.lua", false, "文件不存在或无法读取")
                    }
                    for (entry in allEntries) {
                        if (entry.isDirectory) continue
                        val relative = entry.name.trimEnd('/')
                        val filename = relative.substringAfterLast('/')
                        val isKey = filename.endsWith(".schema.yaml")
                            || filename.endsWith(".dict.yaml")
                            || (filename.endsWith(".lua") && !relative.contains('/'))
                            || relative.startsWith("lua/")
                            || relative.startsWith("opencc/")
                        if (!isKey || isDefaultCustom(filename) || filename == "rime.lua") continue
                        val dirPart = relative.substringBeforeLast('/', "")
                        val dir = if (dirPart.isEmpty()) root else dirCache[dirPart]
                        val exists = dir != null && (fileUriCache[dir.uri.toString()]?.containsKey(filename) == true || dir.findFile(filename) != null)
                        if (exists) addVerify(relative, true, "文件存在")
                        else addVerify(relative, false, "文件不存在")
                    }

                    zipFile.delete()
                    reply.success(mapOf(
                        "mergedSchemas" to dcMergeResult?.userSchemas.orEmpty(),
                        "logs" to logs.toList(),
                        "verify" to verifyArray,
                    ))
                }
            } catch (ex: Exception) {
                reply.error(ex.message ?: "Extraction failed")
            }
        }.start()
    }

    private fun readFileFromTree(root: DocumentFile, filename: String): String? {
        return try {
            root.findFile(filename)?.uri?.let { uri ->
                activity.contentResolver.openInputStream(uri)?.use { it.bufferedReader().readText() }
            }
        } catch (e: Exception) {
            null
        }
    }

    private fun smartExtractZipToPrivate(call: MethodCall, reply: Reply) {
        val zipPath = call.argument<String>("zipPath") ?: return reply.error("Missing zipPath")

        Thread {
            try {
                val root = KeytaoAndroidPaths.requireRoot(activity)
                if (!KeytaoAndroidPaths.isWritable(root)) {
                    return@Thread reply.error("无法写入 ${root.absolutePath}，请检查设备存储空间后重试")
                }
                val zipFile = File(zipPath)
                val logs = mutableListOf<String>()

                JZipFile(zipFile).use { zip ->
                    val allEntries = zip.entries().toList()
                    val zipLuaFilenames = mutableSetOf<String>()
                    var dcEntry: java.util.zip.ZipEntry? = null
                    var rimeLuaEntry: java.util.zip.ZipEntry? = null

                    for (entry in allEntries) {
                        val relative = entry.name.trimEnd('/')
                        val filename = relative.substringAfterLast('/')
                        when {
                            !entry.isDirectory && isDefaultCustom(filename) && dcEntry == null -> dcEntry = entry
                            !entry.isDirectory && filename == "rime.lua" && !relative.contains('/') && rimeLuaEntry == null -> rimeLuaEntry = entry
                            !entry.isDirectory && relative.startsWith("lua/") && !relative.substring(4).contains('/') -> zipLuaFilenames.add(filename)
                        }
                    }

                    val dcMergeResult = dcEntry?.let { entry ->
                        val zipContent = zip.getInputStream(entry).bufferedReader().readText()
                        val existing = readPrivateText(root, "default.custom.yaml")
                            ?: readPrivateText(root, "default-custom.yaml")
                        mergeDefaultCustom(existing, zipContent)
                    }

                    val rimeLuaMergeResult = rimeLuaEntry?.let { entry ->
                        val zipContent = zip.getInputStream(entry).bufferedReader().readText()
                        val localContent = readPrivateText(root, "rime.lua")
                        if (localContent != null) mergeRimeLua(localContent, zipContent, zipLuaFilenames)
                        else RimeLuaMergeResult(zipContent, emptyList())
                    }

                    val renamedLuaFiles = rimeLuaMergeResult?.renames?.mapNotNull { (oldName, newName) ->
                        val oldFile = File(root, "lua/$oldName.lua")
                        if (oldFile.isFile) newName to oldFile.readBytes() else null
                    } ?: emptyList()

                    for (entry in allEntries) {
                        val relative = entry.name.trimEnd('/')
                        if (relative.isEmpty()) continue
                        val filename = relative.substringAfterLast('/')
                        val output = safePrivateFile(root, relative)
                        if (entry.isDirectory) {
                            output.mkdirs()
                            continue
                        }
                        output.parentFile?.mkdirs()
                        when {
                            isDefaultCustom(filename) && dcMergeResult != null -> {
                                output.writeText(dcMergeResult.mergedContent)
                                logs.add("[MERGED] $relative")
                            }
                            filename == "rime.lua" && !relative.contains('/') && rimeLuaMergeResult != null -> {
                                output.writeText(rimeLuaMergeResult.mergedContent)
                                logs.add("[MERGED] $relative")
                            }
                            else -> {
                                zip.getInputStream(entry).use { input ->
                                    FileOutputStream(output).buffered(65536).use { out ->
                                        input.copyTo(out, 65536)
                                    }
                                }
                                logs.add("[OK] $relative")
                            }
                        }
                    }

                    if (renamedLuaFiles.isNotEmpty()) {
                        val luaDir = File(root, "lua").apply { mkdirs() }
                        for ((newName, bytes) in renamedLuaFiles) {
                            File(luaDir, "$newName.lua").writeBytes(bytes)
                            logs.add("[RENAMED] lua/$newName.lua")
                        }
                    }

                    if (!KeytaoAndroidPaths.invalidateDeployment(root)) {
                        return@Thread reply.error("无法清理旧的 Android RIME 部署产物")
                    }
                    logs.add("[INVALIDATED] build")

                    val mergedArray = dcMergeResult?.userSchemas.orEmpty()
                    val logsArray = logs.toList()

                    val verifyArray = mutableListOf<Map<String, Any?>>()
                    fun addVerify(path: String, ok: Boolean, note: String) {
                        verifyArray.add(mapOf("path" to path, "ok" to ok, "note" to note))
                    }
                    addVerify("default.yaml", File(root, "default.yaml").isFile, "KeyTao IME shared data marker")
                    addVerify("*.schema.yaml", root.listFiles()?.any { it.isFile && it.name.endsWith(".schema.yaml") } == true, "Rime schema")

                    reply.success(mutableMapOf<String, Any?>().apply {
                        put("mergedSchemas", mergedArray)
                        put("logs", logsArray)
                        put("verify", verifyArray)
                    })
                }
            } catch (ex: Exception) {
                reply.error(ex.message ?: "Private extraction failed")
            }
        }.start()
    }

    private fun copyAddonSchemaAssets(call: MethodCall, reply: Reply) {
        val id = call.argument<String>("id")
            ?: return reply.error("Missing add-on schema id")
        if (id != easyEnglishAddonId) {
            return reply.error("Unsupported add-on schema: $id")
        }

        Thread {
            try {
                val root = KeytaoAndroidPaths.requireRoot(activity)
                if (!KeytaoAndroidPaths.isWritable(root)) {
                    return@Thread reply.error("无法写入 ${root.absolutePath}，请检查设备存储空间后重试")
                }
                val files = listOf(
                    "easy_en.schema.yaml" to "easy_en.schema.yaml",
                    "easy_en.dict.yaml" to "easy_en.dict.yaml",
                    "easy_en.custom.yaml" to "easy_en.custom.yaml",
                    "lua/easy_en.lua" to "lua/easy_en.lua",
                )
                files.forEach { (assetRelative, destinationRelative) ->
                    val destination = File(root, destinationRelative)
                    destination.parentFile?.mkdirs()
                    val temporary = File(destination.parentFile, ".${destination.name}.${System.nanoTime()}.tmp")
                    try {
                        activity.assets.open("addon-schemas/$id/$assetRelative").use { input ->
                            temporary.outputStream().use { output -> input.copyTo(output) }
                        }
                        if (destination.exists() && !destination.delete()) {
                            throw IllegalStateException("Cannot replace ${destination.absolutePath}")
                        }
                        if (!temporary.renameTo(destination)) {
                            temporary.copyTo(destination, overwrite = true)
                            temporary.delete()
                        }
                    } finally {
                        temporary.delete()
                    }
                }
                reply.success(mutableMapOf<String, Any?>().apply { put("copied", files.size) })
            } catch (ex: Exception) {
                reply.error(ex.message ?: "Failed to copy add-on schema assets")
            }
        }.start()
    }

    private fun deployImeData(reply: Reply) {
        Thread {
            try {
                val root = KeytaoAndroidPaths.requireRoot(activity)
                if (!KeytaoAndroidPaths.hasInstalledSchema(root)) {
                    return@Thread reply.error("请先安装键道方案")
                }
                val configuredSchemas = runCatching {
                    val content = readPrivateText(root, "default.custom.yaml")
                        ?: readPrivateText(root, "default-custom.yaml")
                    content?.let(::parseSchemas).orEmpty()
                }.getOrDefault(emptyList())
                KeytaoRimeDeployClient.deploy(
                    activity,
                    timeoutMs = KeytaoRimeDeployClient.timeoutMsForSchemas(configuredSchemas),
                ) { result ->
                    try {
                        if (result.success) {
                            reply.success(mutableMapOf<String, Any?>().apply {
                                put("path", result.path)
                                put("schemaName", result.schemaName)
                                put("deployed", result.deployed)
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
        private const val easyEnglishAddonId = "easy_en"
        private const val storagePermissionRequestCode = 0x4B54
        private const val directoryPickRequestCode = 0x4B55
    }

    private fun readPrivateText(root: File, relativePath: String): String? {
        return try {
            File(root, relativePath).takeIf { it.isFile }?.readText()
        } catch (e: Exception) {
            null
        }
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
