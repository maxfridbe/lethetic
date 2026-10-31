package io.github.maxfridbe.lethetic.host

import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [35])
class NativeSpeechInputStreamTest {
    private val call = SpeechCall("http://brainiac-nvidia:8000/v1/audio/speech", mapOf("Authorization" to "Bearer stored-key"), "{\"input\":\"hi\"}")
    private class Bridge(vararg responses: String) : SpeechHttpBridge {
        val events = java.util.ArrayDeque(responses.toList())
        var request: JSONObject? = null
        var cancelled = false
        override fun begin(id: String, request: String) { this.request = JSONObject(request) }
        override fun next(id: String) = events.removeFirst()
        override fun cancel(id: String) { cancelled = true }
    }
    @Test fun nativeHttpStreamsWithoutWebViewAndPreservesAuthentication() {
        val bridge = Bridge("""{"kind":"head","status":200,"headers":{"content-type":"application/octet-stream"}}""", """{"kind":"chunk","data":"AQIDBA=="}""", """{"kind":"done"}""")
        NativeSpeechInputStream(call, false, bridge).use { input ->
            assertEquals(1, input.read()); assertFalse(bridge.events.isEmpty())
            assertEquals(call.url, bridge.request!!.getString("url"))
            assertEquals("Bearer stored-key", bridge.request!!.getJSONObject("headers").getString("Authorization"))
            assertArrayEquals(byteArrayOf(2,3,4), input.readBytes())
        }
        assertTrue(bridge.cancelled)
    }
    @Test fun rejectingProviderStatusOrFormatClosesTransportWithoutReadingBodies() {
        for (head in listOf("""{"kind":"head","status":401,"headers":{}}""", """{"kind":"head","status":302,"headers":{}}""", """{"kind":"head","status":200,"headers":{"content-type":"application/json"}}""")) {
            val bridge = Bridge(head, """{"kind":"chunk","data":"c2VjcmV0"}""")
            try { NativeSpeechInputStream(call, false, bridge).use { it.read() }; fail("Invalid response accepted") } catch (error: IllegalArgumentException) { assertFalse(error.message!!.contains("stored-key")) }
            assertTrue(bridge.cancelled); assertEquals(1, bridge.events.size)
        }
    }
    @Test fun closingBeforeReadNeverStartsARequestAndNativeFailuresAreActionable() {
        val bridge = Bridge()
        val stream = NativeSpeechInputStream(call, false, bridge); stream.close()
        try { stream.read(); fail("Closed stream read") } catch (_: IllegalStateException) { }
        assertNull(bridge.request)
        val failed = Bridge("""{"kind":"error","message":"Native speech connection timed out"}""")
        try { NativeSpeechInputStream(call, false, failed).use { it.read() }; fail("Transport failure ignored") }
        catch (error: SpeechHttpException) { assertTrue(SpeechFailure.describe(error, "stream decoding").contains("timed out")) }
        assertTrue(failed.cancelled)
    }
}
