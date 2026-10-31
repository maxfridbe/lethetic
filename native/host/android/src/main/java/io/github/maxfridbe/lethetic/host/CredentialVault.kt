package io.github.maxfridbe.lethetic.host

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.AtomicFile
import org.json.JSONObject
import java.io.File
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/** Device-bound AES-GCM in noBackupFilesDir. Failure never overwrites an unreadable vault. */
internal class CredentialVault(private val file: File, private val key: () -> SecretKey) {
    companion object {
        private const val ALIAS = "lethetic.endpoint-credentials.v1"
        private val allowed = setOf("lethetic.connection-keys.v1", "lethetic.relay-keys.v1", "lethetic.speech-keys.v1")
        fun open(context: Context) = CredentialVault(File(context.noBackupFilesDir, "credentials.v1")) {
            val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
            (store.getKey(ALIAS, null) as? SecretKey) ?: KeyGenerator.getInstance("AES", "AndroidKeyStore").apply {
                init(KeyGenParameterSpec.Builder(ALIAS, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                    .setBlockModes(KeyProperties.BLOCK_MODE_GCM).setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                    .setKeySize(256).build())
            }.generateKey()
        }
    }
    @Synchronized fun read(): JSONObject {
        val atomic = AtomicFile(file)
        val bytes = try { atomic.readFully() } catch (error: java.io.FileNotFoundException) { return JSONObject() }
        require(bytes.size in 29..800000 && bytes[0] == 1.toByte()) { "Credential vault is invalid; existing data was kept" }
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(128, bytes.copyOfRange(1, 13)))
        cipher.updateAAD(ALIAS.toByteArray(Charsets.UTF_8))
        return JSONObject(String(cipher.doFinal(bytes.copyOfRange(13, bytes.size)), Charsets.UTF_8))
    }
    @Synchronized fun write(name: String, value: String?) {
        require(name in allowed && (value?.toByteArray(Charsets.UTF_8)?.size ?: 0) <= 262144) { "Invalid credential entry" }
        val values = read()
        if (value == null) values.remove(name) else values.put(name, value)
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.ENCRYPT_MODE, key()); cipher.updateAAD(ALIAS.toByteArray(Charsets.UTF_8))
        val plaintext = values.toString().toByteArray(Charsets.UTF_8)
        require(plaintext.size <= 790000) { "Credential vault is full" }
        val bytes = byteArrayOf(1) + cipher.iv + cipher.doFinal(plaintext)
        file.parentFile?.mkdirs()
        val atomic = AtomicFile(file); val output = atomic.startWrite()
        try { output.write(bytes); atomic.finishWrite(output) }
        catch (error: Exception) { atomic.failWrite(output); throw error }
    }
}
