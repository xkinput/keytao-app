package ink.rea.keytao_app

import java.io.File
import java.nio.file.Files
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import org.junit.After
import org.junit.Before
import org.junit.Assert.*
import org.junit.Test

class KeytaoAndroidPathsTest {
    private lateinit var temporary: File

    @Before fun setUp() {
        temporary = Files.createTempDirectory("keytao-android-paths").toFile()
        KeytaoAndroidPaths.resetUserRootCacheForTests()
    }

    @After fun tearDown() {
        KeytaoAndroidPaths.resetUserRootCacheForTests()
        temporary.deleteRecursively()
    }

    @Test fun `locked or unmounted storage never probes a root`() {
        assertNull(KeytaoAndroidPaths.resolveUserRoot(
            unlockedAndMounted = { false },
            hasAccess = { error("Permission must not be checked before unlock") },
        ) { error("Root must not be requested before unlock") })
        assertEquals(StorageStatus.NOT_UNLOCKED_OR_NOT_MOUNTED, KeytaoAndroidPaths.lastStatus)
        assertFalse(KeytaoAndroidPaths.isUserRootResolved())
    }

    @Test fun `missing access never creates a directory`() {
        assertNull(KeytaoAndroidPaths.resolveUserRoot(hasAccess = { false }) {
            error("Root must not be requested without access")
        })
        assertEquals(StorageStatus.PERMISSION_MISSING, KeytaoAndroidPaths.lastStatus)
    }

    @Test fun `permission and unlock failures recover after throttle`() {
        var now = 0L
        val root = temporary.resolve("keytao")
        assertNull(KeytaoAndroidPaths.resolveUserRoot({ now }, { false }) { root })
        now += 500_000_000L
        assertNull(KeytaoAndroidPaths.resolveUserRoot({ now }, hasAccess = { false }) { root })
        now += 500_000_000L
        assertEquals(root, KeytaoAndroidPaths.resolveUserRoot({ now }) { root })
        assertEquals(StorageStatus.READY, KeytaoAndroidPaths.lastStatus)
    }

    @Test fun `unwritable or unreadable root stays unresolved`() {
        val root = temporary.resolve("keytao")
        assertNull(KeytaoAndroidPaths.resolveUserRoot(usable = { false }) { root })
        assertEquals(StorageStatus.NOT_UNLOCKED_OR_NOT_MOUNTED, KeytaoAndroidPaths.lastStatus)
        assertFalse(root.exists())
    }

    @Test fun `a file cannot serve as a directory`() {
        val root = temporary.resolve("keytao").apply { writeText("occupied") }
        assertNull(KeytaoAndroidPaths.resolveUserRoot { root })
        assertEquals("occupied", root.readText())
    }

    @Test fun `unavailable root is throttled and only success is cached`() {
        var now = 0L
        var invocations = 0
        val clock = { now }
        assertNull(KeytaoAndroidPaths.resolveUserRoot(clock) { invocations++; null })
        now = 499_999_999L
        assertNull(KeytaoAndroidPaths.resolveUserRoot(clock) { invocations++; temporary })
        assertEquals(1, invocations)
        now = 500_000_000L
        assertNull(KeytaoAndroidPaths.resolveUserRoot(clock) { invocations++; error("Storage unavailable") })
        now = 999_999_999L
        assertNull(KeytaoAndroidPaths.resolveUserRoot(clock) { invocations++; temporary })
        assertEquals(2, invocations)
        now = 1_000_000_000L
        val root = temporary.resolve("keytao")
        assertEquals(root, KeytaoAndroidPaths.resolveUserRoot(clock) { invocations++; root })
        assertTrue(root.isDirectory)
        assertEquals(root, KeytaoAndroidPaths.resolveUserRoot(clock,
            unlockedAndMounted = { error("Success must be cached") },
        ) { invocations++; temporary.resolve("other") })
        assertEquals(3, invocations)
    }

    @Test fun `permission callback may immediately retry a failed resolution`() {
        assertNull(KeytaoAndroidPaths.resolveUserRoot({ 0L }, hasAccess = { false }) { temporary })
        KeytaoAndroidPaths.retryResolution()
        assertEquals(temporary, KeytaoAndroidPaths.resolveUserRoot({ 1L }) { temporary })
    }

    @Test fun `shared root is usable with existing data and a dangling log symlink`() {
        val root = temporary.resolve("keytao").apply { mkdirs() }
        val theme = root.resolve("theme.yaml").apply { writeText("existing theme") }
        val log = root.resolve("log").apply { mkdirs() }.resolve("current")
        Files.createSymbolicLink(log.toPath(), root.resolve("missing.log").toPath())
        assertEquals(root, KeytaoAndroidPaths.resolveUserRoot { root })
        assertEquals(StorageStatus.READY, KeytaoAndroidPaths.lastStatus)
        assertEquals("existing theme", theme.readText())
        assertTrue(Files.isSymbolicLink(log.toPath()))
    }

    @Test fun `slow filesystem probe holds no monitor and first success wins`() {
        val executor = Executors.newFixedThreadPool(2)
        val entered = CountDownLatch(1)
        val release = CountDownLatch(1)
        try {
            val slow = executor.submit<File?> {
                KeytaoAndroidPaths.resolveUserRoot(usable = {
                    entered.countDown()
                    check(release.await(5, TimeUnit.SECONDS))
                    false
                }) { temporary.resolve("slow") }
            }
            assertTrue(entered.await(5, TimeUnit.SECONDS))
            val expected = temporary.resolve("fast")
            val fast = executor.submit<File?> { KeytaoAndroidPaths.resolveUserRoot { expected } }
            assertEquals(expected, fast.get(5, TimeUnit.SECONDS))
            release.countDown()
            assertEquals(expected, slow.get(5, TimeUnit.SECONDS))
            assertEquals(StorageStatus.READY, KeytaoAndroidPaths.lastStatus)
        } finally {
            release.countDown()
            executor.shutdownNow()
            executor.awaitTermination(5, TimeUnit.SECONDS)
        }
    }

    @Test fun `scheme requires source and a fresh manual deployment`() {
        val root = temporary
        val build = root.resolve("build").apply { mkdirs() }
        build.resolve("keydo.schema.yaml").writeText("schema: {}\n")
        assertFalse(KeytaoAndroidPaths.hasInstalledSchema(root))
        assertFalse(KeytaoAndroidPaths.hasDeployedSchema(root))
        root.resolve("default.custom.yaml").writeText("patch:\n  schema_list:\n    - schema: keydo\n")
        root.resolve("keydo.schema.yaml").writeText("schema: {}\n")
        assertTrue(KeytaoAndroidPaths.hasInstalledSchema(root))
        assertTrue(KeytaoAndroidPaths.hasDeployedSchema(root))
        assertTrue(KeytaoAndroidPaths.invalidateDeployment(root))
        assertTrue(KeytaoAndroidPaths.hasInstalledSchema(root))
        assertFalse(KeytaoAndroidPaths.hasDeployedSchema(root))
    }
}
