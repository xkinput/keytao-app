package ink.rea.keytao_app

import java.io.File
import java.io.RandomAccessFile
import java.nio.file.Files
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import org.junit.After
import org.junit.Assert.*
import org.junit.Assume.assumeTrue
import org.junit.Before
import org.junit.Test

class KeytaoImeUpgradeTest {
    private lateinit var root: File
    private lateinit var shared: File
    private val schemas = listOf("keydo")

    @Before fun setUp() {
        root = Files.createTempDirectory("keytao-ime-upgrade").toFile()
        shared = root.resolve("rime-data").apply { mkdirs() }
    }

    @After fun tearDown() {
        root.deleteRecursively()
    }

    @Test fun `APK upgrade refreshes existing shared data before stamping its version`() {
        shared.resolve("default.yaml").writeText("old")
        shared.resolve(".bundled-version").writeText("1/old")
        shared.resolve("obsolete.lua").writeText("removed from bundle")
        val userFile = root.resolve("keydo.custom.yaml").apply { writeText("user settings") }
        KeytaoImeEngine.refreshBundledSharedData(shared, "2/new") { staging ->
            assertEquals("1/old", shared.resolve(".bundled-version").readText())
            assertEquals("old", shared.resolve("default.yaml").readText())
            assertFalse(staging.resolve(".bundled-version").exists())
            staging.resolve("default.yaml").writeText("new")
            staging.resolve("lua").mkdirs()
            staging.resolve("lua/helper.lua").writeText("new lua")
        }
        assertEquals("new", shared.resolve("default.yaml").readText())
        assertEquals("new lua", shared.resolve("lua/helper.lua").readText())
        assertEquals("2/new", shared.resolve(".bundled-version").readText())
        assertFalse(shared.resolve("obsolete.lua").exists())
        assertEquals("user settings", userFile.readText())
        assertFalse(root.resolve(".rime-data.staging").exists())
        assertFalse(root.resolve(".rime-data.previous").exists())
        KeytaoImeEngine.refreshBundledSharedData(shared, "2/new") {
            fail("Matching bundled version must not be copied again")
        }
    }

    @Test fun `unstamped legacy shared data is copied again`() {
        shared.resolve("default.yaml").writeText("old")
        var copied = false
        KeytaoImeEngine.refreshBundledSharedData(shared, "2/new") { staging ->
            copied = true
            staging.resolve("default.yaml").writeText("new")
        }
        assertTrue(copied)
        assertEquals("new", shared.resolve("default.yaml").readText())
        assertEquals("2/new", shared.resolve(".bundled-version").readText())
    }

    @Test fun `failed copy keeps the old shared data and its version stamp`() {
        shared.resolve("default.yaml").writeText("old")
        shared.resolve(".bundled-version").writeText("1/old")
        try {
            KeytaoImeEngine.refreshBundledSharedData(shared, "2/new") { staging ->
                staging.resolve("default.yaml").writeText("partial")
                error("Copy failed")
            }
            fail("Copy error must propagate")
        } catch (_: IllegalStateException) {
            assertEquals("old", shared.resolve("default.yaml").readText())
            assertEquals("1/old", shared.resolve(".bundled-version").readText())
            assertFalse(root.resolve(".rime-data.staging").exists())
        }
    }

    @Test fun `missing default prevents a successful version stamp`() {
        shared.resolve("default.yaml").writeText("old")
        shared.resolve(".bundled-version").writeText("1/old")
        try {
            KeytaoImeEngine.refreshBundledSharedData(shared, "2/new") { }
            fail("Missing shared data must not be stamped")
        } catch (_: IllegalStateException) {
            assertEquals("old", shared.resolve("default.yaml").readText())
            assertEquals("1/old", shared.resolve(".bundled-version").readText())
            assertFalse(root.resolve(".rime-data.staging").exists())
        }
    }

    @Test fun `interrupted replacement restores old data before retrying a failed copy`() {
        shared.resolve("default.yaml").writeText("old")
        shared.resolve(".bundled-version").writeText("1/old")
        assertTrue(shared.renameTo(root.resolve(".rime-data.previous")))
        root.resolve(".rime-data.staging").mkdirs()
        root.resolve(".rime-data.staging/default.yaml").writeText("partial")
        try {
            KeytaoImeEngine.refreshBundledSharedData(shared, "2/new") { staging ->
                assertFalse(staging.resolve("default.yaml").exists())
                error("Copy failed")
            }
            fail("Copy error must propagate")
        } catch (_: IllegalStateException) {
            assertEquals("old", shared.resolve("default.yaml").readText())
            assertEquals("1/old", shared.resolve(".bundled-version").readText())
            assertFalse(root.resolve(".rime-data.staging").exists())
            assertFalse(root.resolve(".rime-data.previous").exists())
        }
    }

    @Test fun `interrupted replacement without a stamp is copied again`() {
        shared.resolve("default.yaml").writeText("incomplete")
        root.resolve(".rime-data.previous").mkdirs()
        root.resolve(".rime-data.previous/default.yaml").writeText("old")
        KeytaoImeEngine.refreshBundledSharedData(shared, "2/new") { staging ->
            staging.resolve("default.yaml").writeText("new")
        }
        assertEquals("new", shared.resolve("default.yaml").readText())
        assertEquals("2/new", shared.resolve(".bundled-version").readText())
        assertFalse(root.resolve(".rime-data.previous").exists())
    }

    @Test fun `concurrent refreshes create the parent and copy shared data once`() {
        val target = root.resolve("new-parent/rime-data")
        val executor = Executors.newFixedThreadPool(8)
        val start = CountDownLatch(1)
        try {
            val attempts = (1..16).map {
                executor.submit<Boolean> {
                    check(start.await(5, TimeUnit.SECONDS))
                    var copied = false
                    KeytaoImeEngine.refreshBundledSharedData(target, "2/new") { staging ->
                        copied = true
                        staging.resolve("default.yaml").writeText("new")
                    }
                    copied
                }
            }
            start.countDown()
            assertEquals(1, attempts.count { it.get(5, TimeUnit.SECONDS) })
            assertEquals("new", target.resolve("default.yaml").readText())
            assertEquals("2/new", target.resolve(".bundled-version").readText())
            val lockFile = root.resolve("new-parent/.rime-data.lock")
            assertTrue(lockFile.isFile)
            RandomAccessFile(lockFile, "rw").use { probe ->
                val lock = probe.channel.tryLock()
                assertNotNull(lock)
                lock?.release()
            }
        } finally {
            start.countDown()
            executor.shutdownNow()
            executor.awaitTermination(5, TimeUnit.SECONDS)
        }
    }

    @Test fun `only a previous deployment can claim an automatic upgrade`() {
        assertFalse(KeytaoImeEngine.claimUpgradeDeployment(root, "2/new", schemas))
        val build = root.resolve("build").apply { mkdirs() }
        build.resolve("unselected.schema.yaml").writeText("schema: {}")
        assertFalse(KeytaoImeEngine.claimUpgradeDeployment(root, "2/new", schemas))
        build.resolve("keydo.schema.yaml").writeText("schema: {}")
        assertTrue(KeytaoImeEngine.claimUpgradeDeployment(root, "2/new", schemas))
        assertFalse(KeytaoImeEngine.claimUpgradeDeployment(root, "2/new", schemas))
        assertTrue(KeytaoImeEngine.claimUpgradeDeployment(root, "3/newer", schemas))
    }

    @Test fun `deployment marker is sufficient but a fresh live sentinel defers the attempt`() {
        assumeTrue("Live PID checks require procfs", File("/proc/${ProcessHandle.current().pid()}").isDirectory)
        val build = root.resolve("build").apply { mkdirs() }
        build.resolve(".keytao-deployed").writeText("keydo\n")
        val sentinel = build.resolve(".keytao-deploying").apply {
            writeText("${System.currentTimeMillis()}/${ProcessHandle.current().pid()}")
        }
        assertFalse(KeytaoImeEngine.claimUpgradeDeployment(root, "2/new", schemas))
        assertTrue(sentinel.delete())
        assertTrue(KeytaoImeEngine.claimUpgradeDeployment(root, "2/new", schemas))
    }

    @Test fun `sentinel from a dead process allows an automatic upgrade`() {
        val build = root.resolve("build").apply { mkdirs() }
        build.resolve(".keytao-deployed").writeText("keydo\n")
        build.resolve(".keytao-deploying").writeText("${System.currentTimeMillis()}/${Long.MAX_VALUE}")
        assertTrue(KeytaoImeEngine.claimUpgradeDeployment(root, "2/new", schemas))
    }

    @Test fun `sentinel older than ten minutes allows an automatic upgrade`() {
        assumeTrue("Live PID checks require procfs", File("/proc/${ProcessHandle.current().pid()}").isDirectory)
        val build = root.resolve("build").apply { mkdirs() }
        build.resolve(".keytao-deployed").writeText("keydo\n")
        val startedAt = System.currentTimeMillis() - TimeUnit.MINUTES.toMillis(11)
        build.resolve(".keytao-deploying").writeText("$startedAt/${ProcessHandle.current().pid()}")
        assertTrue(KeytaoImeEngine.claimUpgradeDeployment(root, "2/new", schemas))
    }

    @Test fun `unparsable sentinel allows an automatic upgrade`() {
        val build = root.resolve("build").apply { mkdirs() }
        build.resolve(".keytao-deployed").writeText("keydo\n")
        build.resolve(".keytao-deploying").writeText("deploying")
        assertTrue(KeytaoImeEngine.claimUpgradeDeployment(root, "2/new", schemas))
    }

    @Test fun `automatic upgrade attempts are scoped to the user root`() {
        root.resolve("build").mkdirs()
        root.resolve("build/.keytao-deployed").writeText("keydo\n")
        val other = root.resolve("other").apply { mkdirs() }
        other.resolve("build").mkdirs()
        other.resolve("build/.keytao-deployed").writeText("keydo\n")
        assertTrue(KeytaoImeEngine.claimUpgradeDeployment(root, "2/new", schemas))
        assertTrue(KeytaoImeEngine.claimUpgradeDeployment(other, "2/new", schemas))
    }

    @Test fun `concurrent readiness probes claim a single upgrade deployment`() {
        root.resolve("build").mkdirs()
        root.resolve("build/.keytao-deployed").writeText("keydo\n")
        val executor = Executors.newFixedThreadPool(8)
        val start = CountDownLatch(1)
        try {
            val attempts = (1..16).map {
                executor.submit<Boolean> {
                    check(start.await(5, TimeUnit.SECONDS))
                    KeytaoImeEngine.claimUpgradeDeployment(root, "2/new", schemas)
                }
            }
            start.countDown()
            assertEquals(1, attempts.count { it.get(5, TimeUnit.SECONDS) })
        } finally {
            start.countDown()
            executor.shutdownNow()
            executor.awaitTermination(5, TimeUnit.SECONDS)
        }
    }
}
