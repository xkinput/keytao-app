package ink.rea.keytao_app

import android.database.Cursor
import android.database.MatrixCursor
import android.net.Uri
import android.os.ParcelFileDescriptor
import android.os.SystemClock
import android.provider.OpenableColumns
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.system.ErrnoException
import android.system.Os
import android.system.OsConstants
import android.system.StructPollfd
import androidx.core.content.FileProvider
import java.io.ByteArrayOutputStream
import java.io.FileNotFoundException
import java.io.InputStream
import java.security.KeyStore
import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.RejectedExecutionException
import java.util.concurrent.ThreadPoolExecutor
import java.util.concurrent.TimeUnit
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

internal object ClipboardEncryption {
    const val mediaMaxBytes = 8 * 1024 * 1024
    const val overheadBytes = 12 + 16
    private val key: SecretKey by lazy {
        val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        (store.getKey("keytao_ime_clipboard", null) as? SecretKey) ?: KeyGenerator.getInstance(
            KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore",
        ).apply {
            init(KeyGenParameterSpec.Builder(
                "keytao_ime_clipboard", KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT,
            ).setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE).build())
        }.generateKey()
    }

    fun encrypt(plain: ByteArray): ByteArray {
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.ENCRYPT_MODE, key)
        check(cipher.iv.size == 12)
        return cipher.iv + cipher.doFinal(plain)
    }

    fun decrypt(encrypted: ByteArray): ByteArray {
        require(encrypted.size >= overheadBytes)
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.DECRYPT_MODE, key, GCMParameterSpec(128, encrypted, 0, 12))
        return cipher.doFinal(encrypted, 12, encrypted.size - 12)
    }

    fun readBounded(input: InputStream, maxBytes: Int): ByteArray {
        val output = ByteArrayOutputStream()
        val buffer = ByteArray(16 * 1024)
        while (true) {
            val count = input.read(buffer)
            if (count < 0) break
            require(count <= maxBytes - output.size())
            output.write(buffer, 0, count)
        }
        return output.toByteArray()
    }
}

class KeytaoClipboardFileProvider : FileProvider() {
    private val reader = ThreadPoolExecutor(1, 1, 30, TimeUnit.SECONDS, ArrayBlockingQueue<Runnable>(1)).apply {
        allowCoreThreadTimeOut(true)
    }

    override fun query(
        uri: Uri, projection: Array<out String>?, selection: String?,
        selectionArgs: Array<out String>?, sortOrder: String?,
    ): Cursor {
        val original = super.query(uri, projection, selection, selectionArgs, sortOrder)
            ?: throw FileNotFoundException()
        if (uri.pathSegments.take(2) != listOf("internal", "clipboard-history")) return original
        return original.use { cursor ->
            MatrixCursor(cursor.columnNames).apply {
                if (cursor.moveToFirst()) addRow(cursor.columnNames.mapIndexed { index, name ->
                    if (name == OpenableColumns.SIZE) {
                        (cursor.getLong(index) - ClipboardEncryption.overheadBytes).coerceAtLeast(0L)
                    } else cursor.getString(index)
                })
            }
        }
    }

    override fun openFile(uri: Uri, mode: String): ParcelFileDescriptor {
        if (uri.pathSegments.take(2) != listOf("internal", "clipboard-history")) {
            return super.openFile(uri, mode) ?: throw FileNotFoundException()
        }
        if (mode != "r") throw FileNotFoundException("Clipboard history is read-only")
        val source = super.openFile(uri, "r") ?: throw FileNotFoundException()
        val pipe = try {
            ParcelFileDescriptor.createPipe()
        } catch (error: Exception) {
            source.close()
            throw error
        }
        try {
            reader.execute {
                runCatching {
                    ParcelFileDescriptor.AutoCloseInputStream(source).use { input ->
                        val encrypted = ClipboardEncryption.readBounded(
                            input, ClipboardEncryption.mediaMaxBytes + ClipboardEncryption.overheadBytes,
                        )
                        // Authenticate before exposing any bytes; plaintext never touches disk.
                        val plain = ClipboardEncryption.decrypt(encrypted)
                        val descriptor = pipe[1].fileDescriptor
                        val flags = Os.fcntlInt(descriptor, OsConstants.F_GETFL, 0)
                        Os.fcntlInt(descriptor, OsConstants.F_SETFL, flags or OsConstants.O_NONBLOCK)
                        val poll = StructPollfd().apply { fd = descriptor; events = OsConstants.POLLOUT.toShort() }
                        val deadline = SystemClock.elapsedRealtime() + 30_000
                        var offset = 0
                        while (offset < plain.size) {
                            val remaining = deadline - SystemClock.elapsedRealtime()
                            check(remaining > 0 && Os.poll(arrayOf(poll), remaining.toInt()) > 0)
                            try {
                                offset += Os.write(descriptor, plain, offset, plain.size - offset)
                            } catch (error: ErrnoException) {
                                if (error.errno != OsConstants.EAGAIN && error.errno != OsConstants.EINTR) throw error
                            }
                        }
                    }
                }
                runCatching { source.close() }
                runCatching { pipe[1].close() }
            }
        } catch (_: RejectedExecutionException) {
            runCatching { source.close() }
            pipe.forEach { runCatching { it.close() } }
            throw FileNotFoundException("Clipboard reader is busy")
        }
        return pipe[0]
    }
}
