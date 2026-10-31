package io.github.maxfridbe.lethetic.host

import org.json.JSONObject
import java.io.File
import java.io.InputStream
import java.io.RandomAccessFile
import java.net.URI
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.security.MessageDigest

internal data class AudioFormat(val rate: Int = 24000, val channels: Int = 1) {
    init { require(rate in 8000..192000 && channels in 1..2) { "Unsupported speech sample rate or channel count" } }
}
internal data class SpeechCall(val url: String, val headers: Map<String, String>, val body: String)
internal data class SpeechPlan(val id: String, val wav: Boolean, val calls: List<SpeechCall>) {
    val key: String get() {
        val digest = MessageDigest.getInstance("SHA-256")
        fun add(s: String) { val bytes = s.toByteArray(Charsets.UTF_8); digest.update(ByteBuffer.allocate(4).putInt(bytes.size).array()); digest.update(bytes) }
        add(if (wav) "wav-v1" else "pcm-v1")
        calls.forEach { call -> add(call.url); add(call.body); call.headers.toSortedMap().forEach { (k,v) -> add(k); add(v) } }
        return digest.digest().joinToString("") { "%02x".format(it) }
    }
    companion object {
        fun parse(json: JSONObject): SpeechPlan {
            val id = json.getString("id"); require(id.length in 1..512)
            val format = json.getString("format"); require(format in listOf("pcm", "wav"))
            val rows = json.getJSONArray("requests"); require(rows.length() in 1..64) { "Speech response is too long" }
            var size = 0
            val calls = (0 until rows.length()).map { i ->
                val row = rows.getJSONObject(i); val url = row.getString("url"); val uri = URI(url)
                require(url.length <= 4096 && uri.scheme in listOf("http", "https") && uri.host != null && uri.rawUserInfo == null && uri.fragment == null) { "Invalid speech endpoint" }
                val body = row.getString("body"); size += body.toByteArray(Charsets.UTF_8).size
                require(size <= 512 * 1024) { "Speech response is too long" }
                val headers = row.getJSONObject("headers"); require(headers.length() <= 4)
                val parsed = headers.keys().asSequence().associate { name ->
                    val value = headers.getString(name)
                    require(name.lowercase() in listOf("content-type", "accept", "authorization", "xi-api-key") && value.length <= 8192 && value.none { it == '\r' || it == '\n' }) { "Invalid speech header" }
                    name to value
                }
                SpeechCall(url, parsed, body)
            }
            return SpeechPlan(id, format == "wav", calls)
        }
    }
}
/** The cache contains audio only: never requests, endpoint URLs or credentials. */
internal class SpeechCache(private val directory: File, private val limit: Long = 128L * 1024 * 1024) {
    init {
        directory.mkdirs()
        synchronized(activeParts) {
            directory.listFiles()?.filter { it.extension == "part" && it.absolutePath !in activeParts }?.forEach { it.delete() }
        }
    }
    fun file(key: String): File {
        require(key.matches(Regex("[a-f0-9]{64}"))) { "Invalid audio cache key" }
        return File(directory, "$key.wav")
    }
    fun ready(key: String): File = file(key).also { require(it.isFile) { "Play the complete response first to download its audio" }; it.setLastModified(System.currentTimeMillis()) }
    fun prune(keep: File) {
        val files = directory.listFiles()?.filter { it.extension == "wav" }?.sortedByDescending { it.lastModified() } ?: return
        var total = keep.length()
        for (file in files) if (file != keep) { total += file.length(); if (total > 512L * 1024 * 1024) file.delete() }
    }
    /** Writes each received block before sending it to Android's bounded audio buffer. */
    fun stream(plan: SpeechPlan, open: (SpeechCall) -> InputStream, check: () -> Unit, sink: (AudioFormat, ByteArray, Int) -> Unit): File {
        val target = file(plan.key)
        if (target.isFile) { target.inputStream().buffered().use { readAudio(it, true, check, sink) }; target.setLastModified(System.currentTimeMillis()); return target }
        val part = synchronized(activeParts) { File.createTempFile("${plan.key}-", ".part", directory).also { activeParts.add(it.absolutePath) } }
        var count = 0L; var format: AudioFormat? = null
        try {
            RandomAccessFile(part, "rw").use { output ->
                output.setLength(0); output.write(ByteArray(44))
                for (call in plan.calls) {
                    check()
                    open(call).buffered().use { input -> readAudio(input, plan.wav, check) { actual, bytes, length ->
                        if (format == null) format = actual
                        require(format == actual) { "Speech endpoint changed audio format between requests" }
                        count += length; require(count <= limit) { "Speech audio exceeds 128 MiB; use a shorter response" }
                        output.write(bytes, 0, length); sink(actual, bytes, length)
                    } }
                }
                check(); require(count > 0) { "Speech endpoint returned no audio" }
                output.seek(0); output.write(wavHeader(format!!, count.toInt())); output.fd.sync()
            }
            check(); require(part.renameTo(target)) { "Could not save audio cache" }; prune(target); return target
        } finally { synchronized(activeParts) { part.delete(); activeParts.remove(part.absolutePath) } }
    }
    companion object {
        private val activeParts = mutableSetOf<String>()
        fun wavHeader(format: AudioFormat, size: Int): ByteArray = ByteBuffer.allocate(44).order(ByteOrder.LITTLE_ENDIAN).apply {
            put("RIFF".toByteArray()); putInt(size + 36); put("WAVEfmt ".toByteArray()); putInt(16); putShort(1); putShort(format.channels.toShort())
            putInt(format.rate); putInt(format.rate * format.channels * 2); putShort((format.channels * 2).toShort()); putShort(16); put("data".toByteArray()); putInt(size)
        }.array()
        private fun exact(input: InputStream, count: Int): ByteArray {
            val bytes = ByteArray(count); var offset = 0
            while (offset < count) { val n = input.read(bytes, offset, count - offset); require(n > 0) { "Speech audio ended unexpectedly" }; offset += n }
            return bytes
        }
        fun readAudio(input: InputStream, wav: Boolean, check: () -> Unit, sink: (AudioFormat, ByteArray, Int) -> Unit) {
            var format = AudioFormat(); var bits = 16; var codec = 1; var remaining = Long.MAX_VALUE
            if (wav) {
                val header = exact(input, 12)
                require(String(header, 0, 4, Charsets.US_ASCII) == "RIFF" && String(header, 8, 4, Charsets.US_ASCII) == "WAVE") { "Endpoint must return a WAV file" }
                var found = false; var skipped = 0L
                while (true) {
                    check(); val chunk = exact(input, 8); val kind = String(chunk, 0, 4, Charsets.US_ASCII)
                    val size = ByteBuffer.wrap(chunk, 4, 4).order(ByteOrder.LITTLE_ENDIAN).int.toLong() and 0xffffffffL
                    if (kind == "data") { require(found) { "WAV format header is missing" }; remaining = size; break }
                    skipped += size + 8; require(skipped <= 1024 * 1024) { "WAV metadata is too large" }
                    if (kind == "fmt ") {
                        require(size in 16..4096); val fmt = ByteBuffer.wrap(exact(input, size.toInt())).order(ByteOrder.LITTLE_ENDIAN)
                        codec = fmt.short.toInt(); val channels = fmt.short.toInt(); val rate = fmt.int; fmt.int
                        val align = fmt.short.toInt(); bits = fmt.short.toInt()
                        require((codec == 1 && bits in listOf(8,16,24,32)) || (codec == 3 && bits == 32)) { "WAV must use PCM or 32-bit float samples" }
                        require(align == channels * bits / 8) { "Invalid WAV block alignment" }
                        format = AudioFormat(rate, channels); found = true
                    } else { var left = size; while (left > 0) { check(); val n = minOf(left, 8192).toInt(); exact(input, n); left -= n } }
                    if (size % 2L != 0L) exact(input, 1)
                }
            }
            val sampleBytes = bits / 8; val frame = sampleBytes * format.channels
            val bytes = ByteArray(8192 + frame); var carry = 0
            while (remaining > 0) {
                check(); val n = input.read(bytes, carry, minOf(8192L, remaining).toInt())
                if (n < 0) { require(!wav) { "Truncated WAV audio" }; break }
                if (n == 0) continue
                remaining -= n; val length = n + carry; val usable = length - length % frame
                val pcm = ByteArray(usable / sampleBytes * 2); val source = ByteBuffer.wrap(bytes).order(ByteOrder.LITTLE_ENDIAN); val out = ByteBuffer.wrap(pcm).order(ByteOrder.LITTLE_ENDIAN)
                repeat(usable / sampleBytes) {
                    val sample = when {
                        codec == 3 -> (source.float.let { if (it.isFinite()) it else 0f }.coerceIn(-1f,1f) * 32767).toInt()
                        bits == 8 -> ((source.get().toInt() and 255) - 128) shl 8
                        bits == 16 -> source.short.toInt()
                        bits == 24 -> { source.get(); (source.get().toInt() and 255) or (source.get().toInt() shl 8) }
                        else -> source.int shr 16
                    }; out.putShort(sample.toShort())
                }
                if (pcm.isNotEmpty()) sink(format, pcm, pcm.size)
                carry = length - usable; bytes.copyInto(bytes, 0, usable, length)
            }
            require(carry == 0) { "Speech stream ended with an incomplete audio frame" }
        }
    }
}
