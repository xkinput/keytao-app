package ink.rea.keytao_app

import java.io.File
import java.io.RandomAccessFile
import java.nio.file.Files
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import org.junit.After
import org.junit.Assert.*
import org.junit.Before
import org.junit.Test

class KeytaoImeUpgradeTest {
    private lateinit var root: File
    private lateinit var shared: File

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

}
