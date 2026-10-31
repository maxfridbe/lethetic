package io.github.maxfridbe.lethetic.host

import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.RuntimeEnvironment
import org.robolectric.annotation.Config
import org.xmlpull.v1.XmlPullParser

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [35])
class SpeechFailureTest {
    @Test fun webViewPolicyStillBlocksNonLoopbackCleartext() {
        val context = RuntimeEnvironment.getApplication()
        val parser = context.resources.getXml(R.xml.lethetic_network_security_config)
        var baseAllowsHttp = false
        parser.use {
            while (it.next() != XmlPullParser.END_DOCUMENT) {
                if (it.eventType == XmlPullParser.START_TAG && it.name == "base-config") {
                    baseAllowsHttp = it.getAttributeValue(null, "cleartextTrafficPermitted") == "true"
                }
            }
        }
        assertFalse("Native HTTP must not weaken the WebView network policy", baseAllowsHttp)
    }
    @Test fun reportsActionableFailureClassesWithoutLeakingExceptionDetails() {
        val secret = "Bearer private-key https://secret.example/private?key=private-key"
        val cases = listOf(
            java.net.UnknownHostException(secret) to "Hostname",
            java.net.ConnectException(secret) to "Connection refused",
            java.net.NoRouteToHostException(secret) to "No route",
            javax.net.ssl.SSLHandshakeException(secret) to "TLS",
            java.net.SocketTimeoutException(secret) to "60 seconds",
            java.io.IOException("Cleartext HTTP traffic not permitted: $secret") to "Android blocked",
            IllegalStateException(secret) to "audio output"
        )
        for ((error, expected) in cases) {
            val message = SpeechFailure.describe(error, "endpoint connection")
            assertTrue(message, message.contains(expected)); assertFalse(message.contains("private-key")); assertFalse(message.contains("secret.example"))
        }
        assertTrue(SpeechFailure.describe(java.io.IOException(secret, java.net.UnknownHostException(secret)), "stream decoding").contains("Hostname"))
    }
}
