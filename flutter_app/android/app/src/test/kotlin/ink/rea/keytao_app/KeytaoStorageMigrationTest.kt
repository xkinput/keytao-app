package ink.rea.keytao_app

import java.io.File
import java.nio.file.Files
import org.junit.After
import org.junit.Assert.*
import org.junit.Test

class KeytaoStorageMigrationTest {
    private val directory = Files.createTempDirectory("keytao-storage-migration").toFile()
    private val root = File(directory, "keytao")
    private val external = File(directory, "external/keytao")
    private val internal = File(directory, "internal/keytao")
    private val oldRoots = listOf(external, internal)

    @After fun tearDown() { directory.deleteRecursively() }

    @Test fun `empty roots require no migration`() {
        assertEquals(StorageMigrationDecision(), decideStorageMigration(false, emptyList()))
    }

    @Test fun `shared root alone remains untouched`() {
        assertEquals(StorageMigrationDecision(), decideStorageMigration(true,
            listOf(StorageRootSnapshot(external, false, 100))))
    }

    @Test fun `newest relevant timestamp wins over empty newer root`() {
        assertEquals(StorageMigrationDecision(external), decideStorageMigration(false, listOf(
            StorageRootSnapshot(external, true, 10), StorageRootSnapshot(internal, false, 20))))
        assertEquals(StorageMigrationDecision(internal), decideStorageMigration(false, listOf(
            StorageRootSnapshot(external, true, 10), StorageRootSnapshot(internal, true, 20))))
    }

    @Test fun `existing shared content is preserved when an old root wins`() {
        assertEquals(StorageMigrationDecision(internal, true), decideStorageMigration(true,
            listOf(StorageRootSnapshot(internal, true, 20))))
    }

    @Test fun `equal timestamps deterministically prefer supplied order`() {
        assertEquals(external, decideStorageMigration(false, listOf(
            StorageRootSnapshot(external, true, 10), StorageRootSnapshot(internal, true, 10))).source)
    }

    @Test fun `build and old marker alone are empty but empty userdb is content`() {
        write(external, "build/x", "generated")
        write(external, ".keytao-migrated-from-shared-storage", "1")
        assertFalse(KeytaoStorageMigrationFiles.snapshot(external).hasContent)
        File(external, "keydo.userdb").mkdirs()
        assertTrue(KeytaoStorageMigrationFiles.snapshot(external).hasContent)
    }

    @Test fun `migration verifies source and preserves the other old root before cleanup`() {
        write(external, "theme.yaml", "older", 1000)
        write(internal, "theme.yaml", "newer", 2000)
        write(internal, "keydo.userdb/00001.ldb", "learned", 2000)
        write(internal, "build/generated", "stale", 2000)
        var stopped = false
        assertTrue(migrate(stop = { stopped = true }, verify = { source, target ->
            assertTrue(stopped)
            assertTrue(external.exists())
            assertTrue(internal.exists())
            assertFalse(root.exists())
            KeytaoStorageMigrationFiles.verifyCopy(source, target)
        }))
        assertEquals("newer", File(root, "theme.yaml").readText())
        assertEquals("learned", File(root, "keydo.userdb/00001.ldb").readText())
        assertFalse(File(root, "build").exists())
        assertTrue(File(root, "keytao-ime.reload").isFile)
        assertFalse(File(root, KeytaoStorageMigrationFiles.deployMarker).exists())
        assertTrue(oldRoots.none { it.exists() })
        assertEquals("older", File(directory, "keytao-old-test-external/theme.yaml").readText())
        assertFalse(KeytaoStorageMigrationFiles.needsCleanup(root))
        assertFalse(migrate(stop = { fail("No consumers to stop on second run") }))
    }

    @Test fun `userdb beats newer config and logs in the internal root`() {
        write(external, "keydo.userdb/00001.ldb", "learned", 1000)
        File(external, "keydo.userdb").setLastModified(1000)
        write(internal, "keyboard.yaml", "recent config", 9000)
        write(internal, "rime-data/log", "recent log", 10000)
        migrate()
        assertEquals("learned", File(root, "keydo.userdb/00001.ldb").readText())
        assertEquals("recent config", File(directory, "keytao-old-test-internal/keyboard.yaml").readText())
    }

    @Test fun `installed schema beats newer config only root`() {
        write(external, "default.custom.yaml", "patch:\n  schema_list:\n    - schema: keydo\n", 1000)
        write(external, "keydo.schema.yaml", "schema: {}", 1000)
        write(internal, "keyboard.yaml", "recent config", 9000)
        assertTrue(KeytaoAndroidPaths.hasInstalledSchema(external))
        migrate()
        assertTrue(File(root, "keydo.schema.yaml").isFile)
        assertTrue(File(root, KeytaoStorageMigrationFiles.deployMarker).isFile)
    }

    @Test fun `both userdb roots include schema and custom file timestamps in comparison`() {
        write(external, "keydo.userdb/00001.ldb", "external data", 2000)
        write(internal, "keydo.userdb/00001.ldb", "internal data", 1000)
        write(internal, "keydo.schema.yaml", "schema: {}", 3000)
        fun source() = decideStorageMigration(false, oldRoots.map { KeytaoStorageMigrationFiles.snapshot(it) }).source
        assertEquals(internal, source())
        write(external, "keydo.custom.yaml", "patch: {}", 4000)
        assertEquals(external, source())
        write(internal, "log/latest", "recent log", 10000)
        assertEquals(external, source())
    }

    @Test fun `userdb contents rather than directory timestamps determine freshness`() {
        write(external, "keydo.userdb/00001.ldb", "external data", 2000)
        write(internal, "keydo.userdb/00001.ldb", "internal data", 1000)
        File(internal, "keydo.userdb").setLastModified(10000)
        assertEquals(1000, KeytaoStorageMigrationFiles.snapshot(internal).modifiedAt)
        migrate()
        assertEquals("external data", File(root, "keydo.userdb/00001.ldb").readText())
    }

    @Test fun `legacy custom config timestamp participates when both roots have userdb`() {
        write(external, "keydo.userdb/00001.ldb", "external data", 2000)
        write(internal, "keydo.userdb/00001.ldb", "internal data", 1000)
        write(internal, "default-custom.yaml", "patch: {}", 3000)
        migrate()
        assertEquals("internal data", File(root, "keydo.userdb/00001.ldb").readText())
        assertEquals("external data", File(directory, "keytao-old-test-external/keydo.userdb/00001.ldb").readText())
    }

    @Test fun `non source backup includes build files and survives name collisions`() {
        write(external, "keydo.userdb/00001.ldb", "learned", 2000)
        write(internal, "build/compiled", "keep build")
        write(internal, ".keytao-migrated-from-shared-storage", "keep marker")
        write(File(directory, "keytao-old-test-internal"), "keep", "previous backup")
        migrate()
        assertEquals("keep build", File(directory, "keytao-old-test-1-internal/build/compiled").readText())
        assertEquals("keep marker", File(directory, "keytao-old-test-1-internal/.keytao-migrated-from-shared-storage").readText())
        assertEquals("previous backup", File(directory, "keytao-old-test-internal/keep").readText())
    }

    @Test fun `staged directories and published parent are synced before cleanup`() {
        write(external, "keydo.userdb/00001.ldb", "learned")
        write(internal, "nested/notes", "other data")
        val synced = mutableListOf<File>()
        var sawPublishedBarrier = false
        migrate(sync = { dir ->
            synced.add(dir)
            if (dir == directory && root.exists()) {
                sawPublishedBarrier = true
                assertTrue(oldRoots.all { it.exists() })
                assertEquals("other data", File(directory, "keytao-old-test-internal/nested/notes").readText())
            }
        })
        assertTrue(sawPublishedBarrier)
        val stage = File(directory, ".keytao-migration-stage")
        assertTrue(synced.indexOf(File(stage, "keydo.userdb")) < synced.indexOf(stage))
        assertTrue(synced.contains(File(directory, ".keytao-migration-stage-internal/nested")))
        assertTrue(oldRoots.none { it.exists() })
    }

    @Test fun `directory sync failure before publish leaves old roots untouched`() {
        write(external, "keydo.userdb/00001.ldb", "learned")
        write(internal, "theme.yaml", "other")
        assertThrows(java.io.IOException::class.java) {
            migrate(sync = { dir ->
                if (dir.name == ".keytao-migration-stage") throw java.io.IOException("directory EIO")
            })
        }
        assertTrue(oldRoots.all { it.exists() })
        assertFalse(root.exists())
    }

    @Test fun `publish parent sync failure retains old roots and retries the barrier before cleanup`() {
        write(external, "keydo.userdb/00001.ldb", "learned")
        write(internal, "theme.yaml", "other")
        failPublishedBarrier()
        assertTrue(oldRoots.all { it.exists() })
        assertTrue(KeytaoStorageMigrationFiles.needsCleanup(root))
        var retriedBarrier = false
        assertTrue(migrate(verify = { _, _ -> fail("Published data must not be recopied") }, sync = { dir ->
            if (dir == directory) {
                assertTrue(oldRoots.all { it.exists() })
                retriedBarrier = true
            }
        }))
        assertTrue(retriedBarrier)
        assertTrue(oldRoots.none { it.exists() })
        assertEquals("learned", File(root, "keydo.userdb/00001.ldb").readText())
    }

    @Test fun `corrupted non source backup aborts cleanup on recovery`() {
        write(external, "keydo.userdb/00001.ldb", "learned")
        write(internal, "theme.yaml", "other")
        failPublishedBarrier()
        File(directory, "keytao-old-test-internal/theme.yaml").writeText("wrong")
        assertThrows(IllegalStateException::class.java) { migrate() }
        assertTrue(oldRoots.all { it.exists() })
        assertTrue(KeytaoStorageMigrationFiles.needsCleanup(root))
    }

    @Test fun `cleanup recovery does not select a partially deleted source`() {
        write(external, "keydo.userdb/00001.ldb", "learned")
        write(external, "keydo.userdb/00002.ldb", "more data")
        write(internal, "theme.yaml", "other")
        failPublishedBarrier()
        File(external, "keydo.userdb/00002.ldb").delete()
        migrate(verify = { _, _ -> fail("Must not recopy partial source") })
        assertEquals("more data", File(root, "keydo.userdb/00002.ldb").readText())
        assertEquals("other", File(directory, "keytao-old-test-internal/theme.yaml").readText())
    }

    @Test fun `old root removal sync failure retains recovery marker and published data`() {
        write(external, "keydo.userdb/00001.ldb", "learned")
        write(internal, "theme.yaml", "other")
        assertThrows(java.io.IOException::class.java) {
            migrate(sync = { dir ->
                if (dir == external.parentFile) throw java.io.IOException("old parent EIO")
            })
        }
        assertTrue(KeytaoStorageMigrationFiles.needsCleanup(root))
        assertEquals("learned", File(root, "keydo.userdb/00001.ldb").readText())
        assertEquals("other", File(directory, "keytao-old-test-internal/theme.yaml").readText())
        migrate(verify = { _, _ -> fail("Must not recopy after partial cleanup") })
        assertFalse(KeytaoStorageMigrationFiles.needsCleanup(root))
        assertTrue(oldRoots.none { it.exists() })
    }

    private fun failPublishedBarrier() {
        assertThrows(java.io.IOException::class.java) {
            migrate(sync = { dir ->
                if (dir == directory && root.exists()) throw java.io.IOException("parent EIO")
            })
        }
    }

    @Test fun `explicit retry clears failure before retrying while status polling does not`() {
        val state = StorageMigrationState()
        state.prepare(true, { error("disk full") }, { fail("No deploy after a failed migration") })
        assertTrue(state.error!!.contains("disk full"))
        assertThrows(IllegalStateException::class.java) { state.requireRoot { root } }
        state.prepare(false, { fail("Status polling must not repeat failed migration") }, {})
        state.prepare(true, {
            assertNull(state.error)
            migrate()
        }, {})
        assertNull(state.error)
        assertEquals(root, state.requireRoot { root })
    }

    @Test fun `post deploy failure leaves the root available and retries once next app start`() {
        write(internal, "keydo.schema.yaml", "schema: {}")
        write(internal, "default.custom.yaml", "patch:\n  schema_list:\n    - schema: keydo\n")
        val state = StorageMigrationState()
        var deploys = 0
        state.prepare(true, { migrate() }, { deploys++; error("deploy failed") })
        assertNull(state.error)
        assertEquals("deploy failed", state.deployError)
        assertEquals(root, state.requireRoot { root })
        assertTrue(oldRoots.none { it.exists() })
        assertTrue(File(root, KeytaoStorageMigrationFiles.deployMarker).exists())
        state.prepare(true, { fail("Migration is already complete") }, { deploys++ })
        assertEquals(1, deploys)
        val restarted = StorageMigrationState()
        restarted.prepare(true, { assertFalse(migrate()) }, {
            if (File(root, KeytaoStorageMigrationFiles.deployMarker).exists()) deploys++
        })
        assertEquals(2, deploys)
        state.deploymentSucceeded()
        assertNull(state.deployError)
    }

    @Test fun `both userdb roots compare data timestamps and ignore config timestamps`() {
        write(external, "keydo.userdb/00001.ldb", "newer data", 2000)
        write(internal, "keydo.userdb/00001.ldb", "older data", 1000)
        write(internal, "keyboard.yaml", "recent config", 9000)
        migrate()
        assertEquals("newer data", File(root, "keydo.userdb/00001.ldb").readText())
    }

    @Test fun `same size corruption aborts before either old root is deleted`() {
        write(external, "theme.yaml", "other", 1000)
        write(internal, "keydo.userdb/00001.ldb", "learned", 2000)
        assertThrows(IllegalStateException::class.java) {
            migrate(verify = { source, target ->
                File(target, "keydo.userdb/00001.ldb").writeText("damaged")
                KeytaoStorageMigrationFiles.verifyCopy(source, target)
            })
        }
        assertEquals("learned", File(internal, "keydo.userdb/00001.ldb").readText())
        assertEquals("other", File(external, "theme.yaml").readText())
        assertFalse(root.exists())
    }

    @Test fun `pre July shared data is kept as is without old data`() {
        write(root, "theme.yaml", "keep")
        assertFalse(migrate(stop = { fail("Must not stop consumers without a migration") }))
        assertEquals("keep", File(root, "theme.yaml").readText())
    }

    @Test fun `backup collisions never overwrite existing data`() {
        write(root, "theme.yaml", "pre July")
        write(File(directory, "keytao-old-test"), "keep", "older backup")
        write(internal, "theme.yaml", "current")
        migrate()
        assertEquals("pre July", File(directory, "keytao-old-test-1/theme.yaml").readText())
        assertEquals("older backup", File(directory, "keytao-old-test/keep").readText())
        assertEquals("current", File(root, "theme.yaml").readText())
    }

    @Test fun `even build only shared content is preserved as a backup`() {
        write(root, "build/compiled", "preexisting")
        write(internal, "theme.yaml", "current")
        migrate()
        assertEquals("preexisting", File(directory, "keytao-old-test/build/compiled").readText())
    }

    @Test fun `unreadable old directory fails closed without deleting anything`() {
        write(internal, "theme.yaml", "current")
        val unreadable = object : File(internal.path) {
            override fun listFiles(): Array<File>? = null
        }
        assertThrows(java.io.IOException::class.java) { KeytaoStorageMigrationFiles.snapshot(unreadable) }
        assertEquals("current", File(internal, "theme.yaml").readText())
    }

    @Test fun `missing nested file fails recursive verification`() {
        write(internal, "nested/user.txt", "learned")
        assertThrows(IllegalStateException::class.java) {
            migrate(verify = { source, target ->
                File(target, "nested/user.txt").delete()
                KeytaoStorageMigrationFiles.verifyCopy(source, target)
            })
        }
        assertTrue(File(internal, "nested/user.txt").isFile)
        assertFalse(root.exists())
    }

    @Test fun `verification failure removes staging and retains old data and backup`() {
        write(root, "theme.yaml", "pre July")
        write(internal, "nested/user.txt", "complete")
        assertThrows(IllegalStateException::class.java) {
            migrate(verify = { source, target ->
                File(target, "nested/user.txt").writeText("bad")
                KeytaoStorageMigrationFiles.verifyCopy(source, target)
            })
        }
        assertEquals("complete", File(internal, "nested/user.txt").readText())
        assertEquals("pre July", File(directory, "keytao-old-test/theme.yaml").readText())
        assertFalse(root.exists())
        assertFalse(File(directory, ".keytao-migration-stage").exists())
        assertTrue(migrate())
    }

    @Test fun `process stop failure leaves all roots untouched`() {
        write(root, "theme.yaml", "pre July")
        write(internal, "theme.yaml", "current")
        assertThrows(IllegalStateException::class.java) { migrate(stop = { error("still running") }) }
        assertEquals("pre July", File(root, "theme.yaml").readText())
        assertEquals("current", File(internal, "theme.yaml").readText())
    }

    @Test fun `published cleanup resumes without selecting partially deleted old data`() {
        write(root, "theme.yaml", "verified full copy")
        write(root, KeytaoStorageMigrationFiles.cleanupMarker, "verified")
        write(internal, "theme.yaml", "partial old copy")
        assertTrue(migrate(verify = { _, _ -> fail("Must not recopy a partially deleted source") }))
        assertEquals("verified full copy", File(root, "theme.yaml").readText())
        assertFalse(internal.exists())
        assertEquals("partial old copy", File(directory, "keytao-old-test-internal/theme.yaml").readText())
    }

    @Test fun `only installed schemas schedule post migration deploy`() {
        write(internal, "theme.yaml", "theme")
        assertTrue(KeytaoStorageMigrationFiles.migrate(root, oldRoots, "test", {}, { true }, {}))
        assertTrue(File(root, KeytaoStorageMigrationFiles.deployMarker).isFile)
    }

    private fun migrate(
        stop: () -> Unit = {},
        verify: (File, File) -> Unit = KeytaoStorageMigrationFiles::verifyCopy,
        sync: (File) -> Unit = {},
    ) = KeytaoStorageMigrationFiles.migrate(root, oldRoots, "test", stop,
        KeytaoAndroidPaths::hasInstalledSchema, sync, verify)

    private fun write(root: File, path: String, text: String, modified: Long? = null) {
        File(root, path).apply {
            parentFile!!.mkdirs()
            writeText(text)
            modified?.let { check(setLastModified(it)) }
        }
    }
}
