package com.aukilabs.annotator

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.AtomicFile
import java.io.File
import java.nio.ByteBuffer
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/** Not a data class: generated toString/copy must not expose or duplicate secrets. */
internal class SavedSignIn(val email: String, val password: ByteArray) {
    fun clear() = password.fill(0)
}

/** Device-local, authenticated encryption. No plaintext preferences or cloud backup.
 * The legacy SDK user/password flow cannot export/import its refresh session yet.
 * This store only supplies that same SDK login; it never bypasses authorization.
 */
internal class SavedSignInStore(
    context: Context,
    private val alias: String = "annotator.auki-dev.sign-in.v1",
    filename: String = "auki-dev-sign-in.enc",
) {
    private val file = AtomicFile(File(context.noBackupFilesDir, filename))
    private val aad = "com.aukilabs.annotator.auki-dev-sign-in.v1".toByteArray()
    private fun keyStore() = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
    fun exists() = file.baseFile.exists() || File(file.baseFile.path+".bak").exists()

    private fun encryptionKey(): SecretKey {
        (keyStore().getKey(alias,null) as? SecretKey)?.let { return it }
        return KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES,"AndroidKeyStore").apply {
            init(KeyGenParameterSpec.Builder(alias,KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                .setKeySize(256).setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setRandomizedEncryptionRequired(true).build())
        }.generateKey()
    }

    @Synchronized fun save(signIn: SavedSignIn) {
        val email = signIn.email.toByteArray(Charsets.UTF_8)
        require(email.size in 1..320 && signIn.password.size in 1..1024)
        val plain = ByteBuffer.allocate(4+email.size+signIn.password.size)
            .putInt(email.size).put(email).put(signIn.password).array()
        try {
            val cipher = Cipher.getInstance("AES/GCM/NoPadding")
            cipher.init(Cipher.ENCRYPT_MODE,encryptionKey()); cipher.updateAAD(aad)
            val encrypted = cipher.doFinal(plain)
            check(cipher.iv.size == 12)
            val stream = file.startWrite()
            try {
                stream.write(1); stream.write(cipher.iv); stream.write(encrypted)
                file.finishWrite(stream)
            } catch (e: Exception) { file.failWrite(stream); throw e }
        } finally { plain.fill(0); email.fill(0) }
    }

    @Synchronized fun load(): SavedSignIn? {
        if (!exists()) return null
        val encoded = file.openRead().use { input ->
            val bytes = input.readBytesBounded(4096)
            require(bytes.size in 30..4096 && bytes[0] == 1.toByte())
            bytes
        }
        val key = keyStore().getKey(alias,null) as? SecretKey ?: error("Saved sign-in key unavailable")
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.DECRYPT_MODE,key,GCMParameterSpec(128,encoded.copyOfRange(1,13)))
        cipher.updateAAD(aad)
        val plain = cipher.doFinal(encoded,13,encoded.size-13)
        try {
            val buffer = ByteBuffer.wrap(plain)
            val emailSize = buffer.int
            require(emailSize in 1..320 && buffer.remaining()-emailSize in 1..1024)
            val email = ByteArray(emailSize); buffer.get(email)
            val password = ByteArray(buffer.remaining()); buffer.get(password)
            return SavedSignIn(email.toString(Charsets.UTF_8),password)
        } finally { plain.fill(0) }
    }

    @Synchronized fun clear() {
        // Only this app's saved login: leave Home profile, peer identity and maps intact.
        file.delete()
        keyStore().deleteEntry(alias)
    }
}

private fun java.io.InputStream.readBytesBounded(limit: Int): ByteArray {
    val bytes = ByteArray(limit+1)
    var size = 0
    while (size < bytes.size) {
        val count = read(bytes,size,bytes.size-size)
        if (count < 0) break
        size += count
    }
    require(size <= limit)
    return bytes.copyOf(size)
}
