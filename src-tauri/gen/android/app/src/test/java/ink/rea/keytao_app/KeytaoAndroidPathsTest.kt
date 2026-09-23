package ink.rea.keytao_app

import java.io.File
import java.io.IOException
import java.nio.file.Files
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class KeytaoAndroidPathsTest {
    @Test
    fun `unavailable root is retried and only the first external root is cached`() {
        val temporary = Files.createTempDirectory("keytao-android-root-cache").toFile()
        KeytaoAndroidPaths.resetUserRootCacheForTests()
        try {
            var nowNs = 0L
            val clock = { nowNs }
            var invocations = 0
            assertFalse(KeytaoAndroidPaths.isUserRootResolved())
            assertNull(KeytaoAndroidPaths.resolveUserRoot(clock) { invocations++; null })
            nowNs = 499_999_999L
            assertNull(KeytaoAndroidPaths.resolveUserRoot(clock) { invocations++; temporary })
            assertEquals(1, invocations)
            nowNs = 500_000_000L
            assertNull(KeytaoAndroidPaths.resolveUserRoot(clock) {
                invocations++
                throw IllegalStateException("Storage not ready")
            })
            assertEquals(2, invocations)
            nowNs = 999_999_999L
            assertNull(KeytaoAndroidPaths.resolveUserRoot(clock) { invocations++; temporary })
            assertEquals(2, invocations)
            assertFalse(KeytaoAndroidPaths.isUserRootResolved())
            nowNs = 1_000_000_000L
            val first = temporary.resolve("first")
            val expected = first.resolve("keytao")
            assertEquals(expected, KeytaoAndroidPaths.resolveUserRoot(clock) { invocations++; first })
            assertTrue(KeytaoAndroidPaths.isUserRootResolved())
            nowNs = 2_000_000_000L
            assertEquals(expected, KeytaoAndroidPaths.resolveUserRoot(clock) {
                invocations++
                temporary.resolve("second")
            })
            assertEquals(3, invocations)
        } finally {
            KeytaoAndroidPaths.resetUserRootCacheForTests()
            temporary.deleteRecursively()
        }
    }

    @Test
    fun `blocked provider holds no monitor and the first published root wins`() {
        val temporary = Files.createTempDirectory("keytao-android-root-race").toFile()
        val executor = Executors.newFixedThreadPool(2)
        val providerEntered = CountDownLatch(1)
        val releaseProvider = CountDownLatch(1)
        KeytaoAndroidPaths.resetUserRootCacheForTests()
        try {
            val slow = executor.submit<File?> {
                KeytaoAndroidPaths.resolveUserRoot {
                    providerEntered.countDown()
                    check(releaseProvider.await(5, TimeUnit.SECONDS))
                    temporary.resolve("slow")
                }
            }
            assertTrue(providerEntered.await(5, TimeUnit.SECONDS))
            val expected = temporary.resolve("fast/keytao")
            val fast = executor.submit<File?> {
                KeytaoAndroidPaths.resolveUserRoot { temporary.resolve("fast") }
            }
            assertEquals(expected, fast.get(5, TimeUnit.SECONDS))
            releaseProvider.countDown()
            assertEquals(expected, slow.get(5, TimeUnit.SECONDS))
        } finally {
            releaseProvider.countDown()
            executor.shutdownNow()
            executor.awaitTermination(5, TimeUnit.SECONDS)
            KeytaoAndroidPaths.resetUserRootCacheForTests()
            temporary.deleteRecursively()
        }
    }

    @Test
    fun `only an unused internal root without an install or learned words is stale`() {
        val temporary = Files.createTempDirectory("keytao-android-stale-root").toFile()
        try {
            val root = temporary.resolve("internal").apply { mkdirs() }
            val external = temporary.resolve("external")
            root.resolve("rime-data").mkdirs()
            root.resolve("rime-data/default.yaml").writeText("schema_list: []\n")
            root.resolve("keyboard.yaml").writeText("keyboard: {}\n")
            assertTrue(KeytaoAndroidPaths.isStaleInternalRoot(root, external))

            for (name in listOf("default.custom.yaml", "default-custom.yaml")) {
                val marker = root.resolve(name).apply { writeText("patch: {}\n") }
                assertFalse(KeytaoAndroidPaths.isStaleInternalRoot(root, external))
                marker.delete()
            }
            val dictionary = root.resolve("rime-data/../keydo.userdb").apply { mkdirs() }
            assertFalse(KeytaoAndroidPaths.isStaleInternalRoot(root, external))
            dictionary.deleteRecursively()
            val snapshot = root.resolve("sync/x/keydo.userdb.txt")
            requireNotNull(snapshot.parentFile).mkdirs()
            snapshot.writeText("learned words\n")
            assertFalse(KeytaoAndroidPaths.isStaleInternalRoot(root, external))
            snapshot.delete()
            assertTrue(KeytaoAndroidPaths.isStaleInternalRoot(root, external))
            assertFalse(KeytaoAndroidPaths.isStaleInternalRoot(root, root))
            assertFalse(KeytaoAndroidPaths.isStaleInternalRoot(root, root.resolve("../internal")))
            assertFalse(KeytaoAndroidPaths.isStaleInternalRoot(temporary.resolve("missing"), external))

            val canonicalFailure = object : File(root.path) {
                override fun getCanonicalPath(): String = throw IOException("Canonical path unavailable")
            }
            assertFalse(KeytaoAndroidPaths.isStaleInternalRoot(canonicalFailure, external))
            val unreadable = object : File(root.path) {
                override fun listFiles(): Array<File>? = null
            }
            assertFalse(KeytaoAndroidPaths.isStaleInternalRoot(unreadable, external))

            val linkedTarget = Files.createTempDirectory(temporary.toPath(), "linked-target")
            Files.createSymbolicLink(root.resolve("linked").toPath(), linkedTarget)
            assertFalse(KeytaoAndroidPaths.isStaleInternalRoot(root, external))
        } finally {
            temporary.deleteRecursively()
        }
    }

    @Test
    fun `scheme requires source and a fresh manual deployment`() {
        val root = Files.createTempDirectory("keytao-android-schema-state").toFile()
        try {
            val build = root.resolve("build").apply { mkdirs() }
            build.resolve("keydo.schema.yaml").writeText("schema: {}\n")

            assertFalse(KeytaoAndroidPaths.hasInstalledSchema(root))
            assertFalse(KeytaoAndroidPaths.hasDeployedSchema(root))

            root.resolve("default.custom.yaml").writeText(
                "patch:\n  schema_list:\n    - schema: keydo\n",
            )
            root.resolve("keydo.schema.yaml").writeText("schema: {}\n")

            assertTrue(KeytaoAndroidPaths.hasInstalledSchema(root))
            assertTrue(KeytaoAndroidPaths.hasDeployedSchema(root))
            assertTrue(KeytaoAndroidPaths.invalidateDeployment(root))
            assertTrue(KeytaoAndroidPaths.hasInstalledSchema(root))
            assertFalse(KeytaoAndroidPaths.hasDeployedSchema(root))
        } finally {
            root.deleteRecursively()
        }
    }
}
