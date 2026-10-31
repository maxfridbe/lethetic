package io.github.maxfridbe.lethetic.host

import android.app.Activity
import android.content.Intent
import android.os.Looper
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.Robolectric
import org.robolectric.RobolectricTestRunner
import org.robolectric.RuntimeEnvironment
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.Config
import java.net.ServerSocket
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [35])
class SpeechServiceTest {
    @Test fun nativeStreamAndMediaNotificationSurviveActivityDestruction() {
        val server = ServerSocket(0)
        val finishBody = CountDownLatch(1)
        val peer = Thread {
            server.accept().use { socket ->
                socket.soTimeout = 10000
                val input = socket.getInputStream().bufferedReader()
                var length = 0
                while (true) { val line = input.readLine() ?: break; if (line.isEmpty()) break; if (line.startsWith("Content-Length:", true)) length = line.substringAfter(':').trim().toInt() }
                repeat(length) { input.read() }
                val out = socket.getOutputStream()
                out.write("HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: 16384\r\nConnection: close\r\n\r\n".toByteArray())
                out.write(ByteArray(8192)); out.flush()
                finishBody.await(10, TimeUnit.SECONDS)
                out.write(ByteArray(8192)); out.flush()
            }
        }.apply { isDaemon = true; start() }
        val app = RuntimeEnvironment.getApplication()
        val plan = SpeechPlan("background-response", false, listOf(SpeechCall("http://127.0.0.1:${server.localPort}/speech", emptyMap(), "{}")))
        val cache = SpeechCache(java.io.File(app.cacheDir, "audio"))
        cache.file(plan.key).delete()
        val controller = Robolectric.buildService(SpeechService::class.java, Intent(app, SpeechService::class.java).setAction("start")).create()
        fun until(condition: () -> Boolean) {
            repeat(200) { shadowOf(Looper.getMainLooper()).idle(); if (condition()) return; Thread.sleep(10) }
            fail("Native audio condition timed out: ${SpeechService.status()}")
        }
        try {
            SpeechService.start(app, plan); controller.startCommand(0, 1)
            until { java.io.File(app.cacheDir, "audio").listFiles()?.any { it.extension == "part" && it.length() > 44 } == true }
            assertTrue(SpeechService.status().getBoolean("active")); assertFalse(cache.file(plan.key).exists())
            assertNotNull(shadowOf(controller.get()).lastForegroundNotification)
            Robolectric.buildActivity(Activity::class.java).setup().pause().stop().destroy()
            assertTrue(SpeechService.status().getBoolean("active"))
            finishBody.countDown()
            until { cache.file(plan.key).isFile }
            assertEquals(16428L, cache.ready(plan.key).length())
        } finally {
            finishBody.countDown(); controller.destroy(); server.close(); peer.join(2000)
        }
        assertFalse(SpeechService.status().getBoolean("active"))
    }
}
