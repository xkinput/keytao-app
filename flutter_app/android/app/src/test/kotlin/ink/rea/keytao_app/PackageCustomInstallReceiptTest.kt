package ink.rea.keytao_app

import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import java.security.MessageDigest

class PackageCustomInstallReceiptTest {
    private val manifest = ".keytao-package-custom.json"
    private val custom = "xmjd6.custom.yaml"
    private fun hash(content: ByteArray): String =
        MessageDigest.getInstance("SHA-256").digest(content).joinToString("") { "%02x".format(it) }

    private fun receipt(files: MutableMap<String, ByteArray>) = PackageCustomInstallReceipt(
        read = { files[it] },
        write = { name, content -> files[name] = content },
        remove = { files.remove(it); Unit },
    )

    @Test
    fun `successful install records shipped content and survives reopening`() {
        val user = "user changes".toByteArray()
        val incoming = "package v2".toByteArray()
        val files = mutableMapOf(custom to user)
        receipt(files).use {
            assertNull(it.previousSha256(custom))
            it.replace("$custom.new", incoming)
            it.record(custom, incoming)
            it.finish()
        }
        assertArrayEquals(user, files[custom])
        assertArrayEquals(incoming, files["$custom.new"])
        receipt(files).use { assertEquals(hash(incoming), it.previousSha256(custom)) }
    }

    @Test
    fun `later extraction failure restores custom and an existing sidecar`() {
        val original = "package v1".toByteArray()
        val sidecar = "older comparison".toByteArray()
        val record = JSONObject().put(custom, hash(original)).toString().toByteArray()
        val files = mutableMapOf(custom to original, "$custom.new" to sidecar, manifest to record)
        runCatching {
            receipt(files).use {
                assertEquals(hash(original), it.previousSha256(custom))
                it.replace(custom, "package v2".toByteArray())
                it.replace("$custom.new", "comparison v2".toByteArray())
                it.record(custom, "package v2".toByteArray())
                error("Later extraction failed")
            }
        }
        assertEquals(3, files.size)
        assertArrayEquals(original, files[custom])
        assertArrayEquals(sidecar, files["$custom.new"])
        assertArrayEquals(record, files[manifest])
    }

    @Test
    fun `failed record write restores its previous bytes and newly created files`() {
        val oldContent = "package v1".toByteArray()
        val record = JSONObject().put(custom, hash(oldContent)).toString().toByteArray()
        val files = mutableMapOf(custom to oldContent, manifest to record)
        var failRecordWrite = true
        val receipt = PackageCustomInstallReceipt(
            read = { files[it] },
            write = { name, content ->
                files[name] = content
                if (name == manifest && failRecordWrite) {
                    failRecordWrite = false
                    error("Record write failed")
                }
            },
            remove = { files.remove(it); Unit },
        )
        val failure = runCatching {
            receipt.use {
                it.replace(custom, "package v2".toByteArray())
                it.replace("$custom.new", "comparison v2".toByteArray())
                it.record(custom, "package v2".toByteArray())
                it.finish()
            }
        }.exceptionOrNull()
        assertEquals("Record write failed", failure?.message)
        assertEquals(2, files.size)
        assertArrayEquals(oldContent, files[custom])
        assertArrayEquals(record, files[manifest])
    }

    @Test
    fun `default and user customizations stay outside package custom tracking`() {
        assertTrue(isPackageCustomFile(custom))
        assertFalse(isPackageCustomFile("default.custom.yaml"))
        assertFalse(isPackageCustomFile("default-custom.yaml"))
        assertFalse(isPackageCustomFile("xmjd6.user.yaml"))
        assertFalse(isPackageCustomFile("sync/xmjd6.custom.yaml"))
    }

    @Test
    fun `receipt remains rollback capable until Rust package recording succeeds`() {
        val original = "package v1".toByteArray()
        val files = mutableMapOf(custom to original)
        val receipt = receipt(files)
        receipt.use {
            it.replace(custom, "package v2".toByteArray())
            it.record(custom, "package v2".toByteArray())
            it.defer()
        }
        assertFalse(files.containsKey(manifest))
        assertArrayEquals("package v2".toByteArray(), files[custom])
        receipt.rollback()
        assertEquals(1, files.size)
        assertArrayEquals(original, files[custom])
    }

    @Test
    fun `identical content records its hash after deferred commit without rewriting custom`() {
        val original = "package v1".toByteArray()
        val files = mutableMapOf(custom to original)
        val receipt = receipt(files)
        receipt.use {
            it.record(custom, original)
            it.defer()
        }
        receipt.finish()
        assertSame(original, files[custom])
        assertEquals(hash(original), JSONObject(files.getValue(manifest).toString(Charsets.UTF_8)).getString(custom))
    }

    @Test
    fun `package cannot overwrite the comparison file generated for its custom`() {
        assertTrue(runCatching { validatePackageCustomSidecars(setOf(custom, "$custom.new")) }.isFailure)
        validatePackageCustomSidecars(setOf(custom, "other.custom.yaml.new"))
        validatePackageCustomSidecars(setOf("default.custom.yaml", "default.custom.yaml.new"))
    }
}
