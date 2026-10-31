package io.github.maxfridbe.lethetic.host

import android.util.Base64
import org.json.JSONObject
import java.io.InputStream
import java.io.IOException
import java.util.UUID

/** Only native transport's sanitized messages are eligible to be displayed verbatim. */
internal class SpeechHttpException(message: String) : IOException(message)
internal interface SpeechHttpBridge {
    fun begin(id: String, request: String)
    fun next(id: String): String
    fun cancel(id: String)
}
/** Bounded JNI stream; the Android service owns it, independent of the Activity/WebView. */
internal class NativeSpeechInputStream(call: SpeechCall, private val wav: Boolean, bridge: SpeechHttpBridge? = null) : InputStream() {
    private external fun beginNative(id: String, request: String): String
    private external fun nextNative(id: String): String
    private external fun cancelNative(id: String): String
    private val transport = bridge ?: object : SpeechHttpBridge {
        override fun begin(id: String, request: String) { beginNative(id, request) }
        override fun next(id: String) = nextNative(id)
        override fun cancel(id: String) { cancelNative(id) }
    }
    private val id = UUID.randomUUID().toString()
    private val request = JSONObject().put("url", call.url).put("headers", JSONObject(call.headers)).put("body", call.body).toString()
    @Volatile private var closed = false
    private var started = false
    private var head = false
    private var done = false
    private var bytes = ByteArray(0)
    private var offset = 0
    private inline fun <T> native(action: () -> T): T = try { action() } catch (error: IOException) {
        // JNI messages originate in our validated Rust transport, never raw provider bodies.
        throw SpeechHttpException(error.message ?: "Native speech transport failed")
    } catch (_: LinkageError) {
        throw SpeechHttpException("Native speech engine unavailable. Install the latest complete Android release.")
    }
    private fun start() = synchronized(this) {
        check(!closed) { "Speech stream stopped" }
        if (!started) { native { transport.begin(id, request) }; started = true }
    }
    override fun read(): Int {
        val one = ByteArray(1); return if (read(one, 0, 1) < 0) -1 else one[0].toInt() and 255
    }
    override fun read(target: ByteArray, off: Int, len: Int): Int {
        require(off >= 0 && len >= 0 && off <= target.size - len)
        if (len == 0) return 0
        start()
        while (offset == bytes.size && !done) {
            if (closed) throw IOException("Speech stream stopped")
            val event = JSONObject(native { transport.next(id) })
            when (event.getString("kind")) {
                "head" -> {
                    require(!head) { "Duplicate speech response headers" }; head = true
                    val status = event.getInt("status")
                    require(status in 200..299) { "Speech endpoint returned HTTP $status. Check voice settings and credentials." }
                    val mime = event.getJSONObject("headers").optString("content-type").substringBefore(';').trim().lowercase()
                    val accepted = if (wav) setOf("audio/wav", "audio/wave", "audio/x-wav", "audio/vnd.wave") else setOf("audio/pcm", "audio/x-pcm", "audio/raw")
                    require(mime.isEmpty() || mime == "application/octet-stream" || mime in accepted) { "Speech endpoint returned the wrong audio format. Check PCM / WAV voice settings." }
                }
                "chunk" -> {
                    require(head) { "Speech response headers missing" }
                    val data = event.getString("data"); require(data.length <= 90000) { "Speech transport chunk too large" }
                    bytes = Base64.decode(data, Base64.NO_WRAP); offset = 0
                    require(bytes.size <= 65536) { "Speech transport chunk too large" }
                }
                "done" -> { require(head) { "Speech response headers missing" }; done = true }
                "error" -> throw SpeechHttpException(event.getString("message"))
                else -> throw IOException("Invalid native speech event")
            }
        }
        if (done) return -1
        val n = minOf(len, bytes.size - offset); bytes.copyInto(target, off, offset, offset + n); offset += n; return n
    }
    override fun close() = synchronized(this) {
        if (!closed) { closed = true; if (started) native { transport.cancel(id) } }
    }
}
