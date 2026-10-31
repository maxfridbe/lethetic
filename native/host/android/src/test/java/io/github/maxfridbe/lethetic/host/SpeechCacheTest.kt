package io.github.maxfridbe.lethetic.host

import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import java.io.ByteArrayInputStream
import java.nio.file.Files

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [35])
class SpeechCacheTest {
    private fun plan(wav: Boolean = false) = SpeechPlan("response", wav, listOf(SpeechCall("http://localhost/voice", mapOf("Authorization" to "Bearer secret"), "hello")))
    @Test fun pcmIsCachedBeforeEofAndReplayDoesNotContactEndpoint() {
        val directory = Files.createTempDirectory("speech").toFile()
        try {
            directory.resolve("crashed.part").writeBytes(ByteArray(10))
            val cache = SpeechCache(directory); assertFalse(directory.resolve("crashed.part").exists()); val plan = plan(); val bytes = ByteArray(24000) { it.toByte() }
            var opens = 0; var played = 0
            cache.stream(plan, { opens++; ByteArrayInputStream(bytes) }, {}) { format, _, count ->
                assertEquals(AudioFormat(), format); played += count
                assertTrue(directory.listFiles()!!.single { it.extension == "part" }.length() >= played + 44)
                assertFalse(cache.file(plan.key).exists())
                SpeechCache(directory) // Status lookups must not discard an active partial file.
                assertTrue(directory.listFiles()!!.any { it.extension == "part" })
            }
            assertEquals(bytes.size, played); assertEquals(bytes.size + 44L, cache.ready(plan.key).length())
            assertArrayEquals(bytes, cache.ready(plan.key).readBytes().drop(44).toByteArray())
            played = 0
            cache.stream(plan, { error("Replay must use cache") }, {}) { _, _, count -> played += count }
            assertEquals(bytes.size, played); assertEquals(1, opens)
            assertFalse(directory.walk().filter { it.isFile }.any { it.readText().contains("Bearer secret") })
        } finally { directory.deleteRecursively() }
    }
    @Test fun truncationCancellationAndQuotaFailureNeverPublishPartialAudio() {
        val directory = Files.createTempDirectory("speech").toFile()
        try {
            val plan = plan(); val cache = SpeechCache(directory, 20)
            for (mode in 0..2) {
                try {
                    cache.stream(plan, { ByteArrayInputStream(ByteArray(if (mode == 0) 3 else 22)) }, { if (mode == 2) throw InterruptedException() }) { _, _, _ -> }
                    fail("Expected incomplete, quota or cancellation failure")
                } catch (_: Exception) { }
                assertFalse(cache.file(plan.key).exists()); assertTrue(directory.listFiles()!!.isEmpty())
            }
        } finally { directory.deleteRecursively() }
    }
    @Test fun wavAndOneByteReadsPreserveSamplesAndRejectTruncation() {
        val pcm = byteArrayOf(0, -128, -1, 127, 0, 0, 1, 0)
        val wav = SpeechCache.wavHeader(AudioFormat(44100, 2), pcm.size) + pcm
        val input = object : ByteArrayInputStream(wav) { override fun read(b: ByteArray, off: Int, len: Int) = super.read(b, off, minOf(len, 1)) }
        val result = java.io.ByteArrayOutputStream()
        SpeechCache.readAudio(input, true, {}) { format, bytes, n -> assertEquals(AudioFormat(44100, 2), format); result.write(bytes, 0, n) }
        assertArrayEquals(pcm, result.toByteArray())
        try { SpeechCache.readAudio(ByteArrayInputStream(wav.dropLast(1).toByteArray()), true, {}) { _,_,_ -> }; fail("Truncated WAV accepted") } catch (_: IllegalArgumentException) { }
    }
    @Test fun requestValidationRejectsUnsafeUrlsHeadersAndUnboundedWork() {
        fun request(url: String, headers: JSONObject = JSONObject()) = JSONObject().put("id", "turn").put("format", "pcm")
            .put("requests", org.json.JSONArray().put(JSONObject().put("url", url).put("body", "{}").put("headers", headers)))
        assertEquals("http://brainiac-nvidia:8000/voice", SpeechPlan.parse(request("http://brainiac-nvidia:8000/voice")).calls.single().url)
        for (url in listOf("file:///secret", "https://user:secret@example.com/speech", "https://example.com/#secret")) {
            try { SpeechPlan.parse(request(url)); fail("Unsafe URL accepted") } catch (_: IllegalArgumentException) { }
        }
        for (headers in listOf(JSONObject().put("Cookie", "secret"), JSONObject().put("Authorization", "Bearer secret\r\nInjected: value"))) {
            try { SpeechPlan.parse(request("http://localhost", headers)); fail("Unsafe headers accepted") } catch (_: IllegalArgumentException) { }
        }
        assertNotEquals(plan().key, plan().copy(calls = listOf(plan().calls.single().copy(body = "different"))).key)
    }
}
