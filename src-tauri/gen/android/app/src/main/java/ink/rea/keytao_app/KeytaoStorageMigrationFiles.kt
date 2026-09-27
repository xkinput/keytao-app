package ink.rea.keytao_app

import java.io.File
import java.io.FileOutputStream
import java.io.IOException
import java.io.RandomAccessFile
import java.util.Properties
import java.util.zip.CRC32

/** Filesystem transaction with an Android directory-sync adapter and JVM failure tests. */
internal object KeytaoStorageMigrationFiles {
    const val cleanupMarker = ".keytao-migration-cleanup"
    const val deployMarker = ".keytao-migration-deploy"
    private val ignored = setOf("build", ".keytao-migrated-from-shared-storage", cleanupMarker, deployMarker)

    private fun children(root: File): List<File> {
        if (!root.exists()) return emptyList()
        check(root.isDirectory) { "Not a directory: $root" }
        return root.listFiles()?.toList() ?: throw IOException("Cannot read $root")
    }

    private fun entries(root: File): List<File> = children(root).filter { it.name !in ignored }

    fun snapshot(root: File, hasInstalledSchema: (File) -> Boolean = KeytaoAndroidPaths::hasInstalledSchema): StorageRootSnapshot {
        val entries = entries(root).flatMap { child ->
            child.walkTopDown().onFail { _, error -> throw error }.toList()
        }
        val content = entries.filter { it.isFile || it.name.endsWith(".userdb") }
        val userData = content.filter { file ->
            file.isFile && file.relativeTo(root).path.split(File.separatorChar).any { it.endsWith(".userdb") }
        }
        val datedContent = userData + content.filter {
            it.isFile && (it.name.endsWith(".schema.yaml") || it.name.endsWith(".custom.yaml") ||
                it.name == "default-custom.yaml")
        }
        return StorageRootSnapshot(root, content.isNotEmpty(), datedContent.maxOfOrNull { it.lastModified() } ?: 0L,
            userData.isNotEmpty() || hasInstalledSchema(root))
    }

    fun needsCleanup(root: File): Boolean = File(root, cleanupMarker).exists()

    fun migrate(
        root: File,
        oldRoots: List<File>,
        backupSuffix: String,
        stopConsumers: () -> Unit,
        hasInstalledSchema: (File) -> Boolean,
        syncDirectory: (File) -> Unit,
        verify: (File, File) -> Unit = ::verifyCopy,
    ): Boolean {
        val parent = requireNotNull(root.parentFile)
        // A stable sibling inode stays locked even when the whole root is renamed.
        RandomAccessFile(File(parent, ".keytao-migration.lock"), "rw").use { file ->
            file.channel.use { channel -> channel.lock().use {
                if (needsCleanup(root)) {
                    stopConsumers()
                    finishCleanup(root, oldRoots, backupSuffix, syncDirectory)
                    return true
                }
                fun decision() = decideStorageMigration(children(root).isNotEmpty(),
                    oldRoots.map { snapshot(it, hasInstalledSchema) })
                if (decision().source == null) return false
                stopConsumers()
                // Re-read content after every old LevelDB writer has exited.
                val plan = decision()
                val source = requireNotNull(plan.source)
                val stage = File(parent, ".keytao-migration-stage")
                check(!stage.exists() || stage.deleteRecursively()) { "Cannot remove incomplete migration: $stage" }
                if (plan.renameExisting) {
                    check(root.renameTo(unusedBackup(parent, backupSuffix))) { "Cannot preserve existing data: $root" }
                    syncDirectory(parent)
                } else if (root.exists()) {
                    check(root.deleteRecursively()) { "Cannot remove empty data directory: $root" }
                }
                try {
                    check(stage.mkdirs()) { "Cannot create migration staging directory: $stage" }
                    entries(source).forEach { copySynced(it, File(stage, it.name)) }
                    verify(source, stage)
                    val cleanup = Properties().apply { setProperty("source", source.absolutePath) }
                    oldRoots.filter { it != source && it.exists() }.forEach { old ->
                        val backup = preserveOldRoot(old, parent, backupSuffix, oldRoots, syncDirectory)
                        cleanup.setProperty(old.absolutePath, backup.name)
                    }
                    writeSynced(File(stage, "keytao-ime.reload"), System.nanoTime().toString())
                    FileOutputStream(File(stage, cleanupMarker)).use { output ->
                        cleanup.store(output, null)
                        output.fd.sync()
                    }
                    if (hasInstalledSchema(stage)) writeSynced(File(stage, deployMarker), "pending")
                    syncTree(stage, syncDirectory)
                    check(stage.renameTo(root)) { "Cannot publish verified migration: $root" }
                    syncDirectory(parent)
                } catch (error: Exception) {
                    if (stage.exists() && !stage.deleteRecursively()) {
                        throw IOException("${error.message}; cannot remove partial files: $stage", error)
                    }
                    throw error
                }
                finishCleanup(root, oldRoots, backupSuffix, syncDirectory)
                return true
            } }
        }
    }

    private fun unusedBackup(parent: File, suffix: String, label: String = ""): File {
        val ending = if (label.isEmpty()) "" else "-$label"
        var backup = File(parent, "keytao-old-$suffix$ending")
        var collision = 0
        while (backup.exists()) backup = File(parent, "keytao-old-$suffix-${++collision}$ending")
        return backup
    }

    private fun preserveOldRoot(
        old: File, parent: File, suffix: String, oldRoots: List<File>, syncDirectory: (File) -> Unit,
    ): File {
        // oldRoots is always ordered external, internal by the app coordinator.
        val label = if (oldRoots.indexOf(old) == 0) "external" else "internal"
        val backup = unusedBackup(parent, suffix, label)
        val stage = File(parent, ".keytao-migration-stage-$label")
        check(!stage.exists() || stage.deleteRecursively()) { "Cannot remove incomplete backup: $stage" }
        copySynced(old, stage)
        verifyTree(old, stage, children(old))
        syncTree(stage, syncDirectory)
        check(stage.renameTo(backup)) { "Cannot publish backup: $backup" }
        syncDirectory(parent)
        return backup
    }

    private fun finishCleanup(root: File, oldRoots: List<File>, suffix: String, syncDirectory: (File) -> Unit) {
        val parent = requireNotNull(root.parentFile)
        val marker = File(root, cleanupMarker)
        val cleanup = Properties().apply { marker.inputStream().use { load(it) } }
        val source = cleanup.getProperty("source")
        check(source == null || oldRoots.any { it.absolutePath == source }) { "Invalid migration source" }
        // Unknown/older markers cannot identify the source: preserve every remaining old root.
        oldRoots.filter { it.exists() && it.absolutePath != source }.forEach { old ->
            val name = cleanup.getProperty(old.absolutePath)
            val backup = if (name == null) preserveOldRoot(old, parent, suffix, oldRoots, syncDirectory) else {
                check(File(name).name == name && name.startsWith("keytao-old-")) { "Invalid migration backup" }
                File(parent, name)
            }
            verifyTree(old, backup, children(old))
            syncTree(backup, syncDirectory)
        }
        // Also retry these barriers after a crash between publish and parent fsync.
        syncTree(root, syncDirectory)
        syncDirectory(parent)
        oldRoots.forEach { old ->
            check(!old.exists() || old.deleteRecursively()) { "Cannot remove migrated directory: $old" }
            // Do not clear the recovery marker before old-root removals are durable.
            old.parentFile?.takeIf { it.isDirectory }?.let(syncDirectory)
        }
        check(marker.delete()) { "Cannot finish migration cleanup" }
        syncDirectory(root)
    }

    private fun copySynced(source: File, target: File) {
        if (source.isDirectory) {
            check(target.mkdirs()) { "Cannot create $target" }
            children(source).forEach { copySynced(it, File(target, it.name)) }
        } else {
            check(source.isFile) { "Not a regular file: $source" }
            source.inputStream().use { input ->
                FileOutputStream(target).use { output ->
                    input.copyTo(output)
                    output.fd.sync()
                }
            }
        }
        // Best effort: some shared-storage mounts refuse utime, and integrity is proven by CRC, not mtime.
        target.setLastModified(source.lastModified())
    }

    private fun writeSynced(file: File, text: String) {
        FileOutputStream(file).use { output ->
            output.write(text.toByteArray(Charsets.UTF_8))
            output.fd.sync()
        }
    }

    private fun syncTree(root: File, syncDirectory: (File) -> Unit) {
        root.walkBottomUp().onFail { _, error -> throw error }.filter { it.isDirectory }.forEach(syncDirectory)
    }

    internal fun verifyCopy(source: File, target: File) = verifyTree(source, target, entries(source))

    private fun verifyTree(source: File, target: File, entries: List<File>) {
        check(target.isDirectory) { "Missing migration directory: $target" }
        entries.forEach { child ->
            child.walkTopDown().onFail { _, error -> throw error }.forEach { entry ->
                val copy = File(target, entry.relativeTo(source).path)
                check(if (entry.isDirectory) copy.isDirectory else
                    entry.isFile && copy.isFile && entry.length() == copy.length() && checksum(entry) == checksum(copy)) {
                    "Migration verification failed: $entry"
                }
            }
        }
    }

    private fun checksum(file: File): Long {
        val crc = CRC32()
        file.inputStream().use { input ->
            val buffer = ByteArray(DEFAULT_BUFFER_SIZE)
            while (true) {
                val count = input.read(buffer)
                if (count < 0) break
                crc.update(buffer, 0, count)
            }
        }
        return crc.value
    }
}
