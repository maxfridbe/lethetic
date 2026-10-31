package io.github.maxfridbe.lethetic.host

/** Classify failures without exposing exception text (URLs, keys or provider bodies). */
internal object SpeechFailure {
    fun describe(error: Exception, stage: String): String {
        if (error is SpeechHttpException) return "Speech $stage failed: ${error.message}"
        val causes = generateSequence<Throwable>(error) { it.cause }.take(8).toList()
        val detail = when {
            causes.any { it is java.net.UnknownHostException } -> "Hostname could not be resolved. Check the endpoint hostname and this device's Wi-Fi or VPN DNS."
            causes.any { it is java.net.ConnectException } -> "Connection refused or unreachable. Check the server address, port and firewall."
            causes.any { it is java.net.NoRouteToHostException } -> "No route to the speech server. Connect this device to the server's network or VPN."
            causes.any { it is javax.net.ssl.SSLException } -> "TLS certificate or handshake failed. Check the server certificate and this device's clock."
            causes.any { it is java.net.SocketTimeoutException } -> "No response for 60 seconds. Check server connectivity and retry."
            causes.any { it.message?.contains("cleartext", ignoreCase = true) == true } -> "Android blocked this HTTP endpoint. Update the app to a build supporting native HTTP speech."
            error is java.io.IOException -> "The connection or audio cache was interrupted. Check connectivity and available device storage."
            error is SecurityException -> "Android denied access. Check app permissions and device restrictions."
            error is IllegalStateException -> "Android could not initialize or continue audio output. Stop other playback and retry."
            else -> "The operation failed (${error.javaClass.simpleName.replace(Regex("[^A-Za-z0-9_]"), "").take(64)}). Retry or report this stage."
        }
        return "Speech $stage failed: $detail"
    }
}
