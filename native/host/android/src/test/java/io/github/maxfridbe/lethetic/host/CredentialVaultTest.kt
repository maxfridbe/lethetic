package io.github.maxfridbe.lethetic.host

import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import java.nio.file.Files
import javax.crypto.KeyGenerator

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [35])
class CredentialVaultTest {
    @Test fun survivesReopenEncryptedWithFreshIvAndSupportsDeletion() {
        val dir = Files.createTempDirectory("vault").toFile()
        try {
            val file = dir.resolve("credentials"); val key = KeyGenerator.getInstance("AES").apply { init(256) }.generateKey()
            val name = "lethetic.connection-keys.v1"; val secret = "private-provider-credential"
            val vault = CredentialVault(file) { key }; vault.write(name, secret)
            val first = file.readBytes(); assertFalse(String(first).contains(secret))
            assertEquals(secret, CredentialVault(file) { key }.read().getString(name))
            vault.write(name, secret); assertFalse(first.contentEquals(file.readBytes()))
            vault.write(name, null); assertFalse(CredentialVault(file) { key }.read().has(name))
            try { vault.write("arbitrary", "value"); fail("Unknown key accepted") } catch (_: IllegalArgumentException) { }
        } finally { dir.deleteRecursively() }
    }
    @Test fun corruptedVaultIsNotSilentlyOverwritten() {
        val dir = Files.createTempDirectory("vault").toFile()
        try {
            val file = dir.resolve("credentials"); val key = KeyGenerator.getInstance("AES").generateKey()
            val vault = CredentialVault(file) { key }; vault.write("lethetic.speech-keys.v1", "secret")
            val corrupt = file.readBytes().also { it[it.lastIndex] = (it.last().toInt() xor 1).toByte() }; file.writeBytes(corrupt)
            try { vault.write("lethetic.speech-keys.v1", "replacement"); fail("Tampered vault accepted") } catch (_: javax.crypto.AEADBadTagException) { }
            assertArrayEquals(corrupt, file.readBytes())
        } finally { dir.deleteRecursively() }
    }
}
