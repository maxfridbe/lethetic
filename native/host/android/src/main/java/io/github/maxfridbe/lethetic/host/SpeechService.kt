package io.github.maxfridbe.lethetic.host

import android.app.*
import android.content.*
import android.media.*
import android.media.session.MediaSession
import android.media.session.PlaybackState
import android.os.*
import org.json.JSONObject
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicReference

/** Owns both HTTP and playback. WebView suspension does not suspend this service. */
class SpeechService : Service() {
    companion object {
        private const val CHANNEL = "lethetic-response-audio"
        private const val NOTIFICATION = 712
        private val pending = AtomicReference<SpeechPlan?>()
        @Volatile private var snapshot = JSONObject().put("active", false)
        internal fun start(context: Context, plan: SpeechPlan) {
            pending.set(plan)
            try { context.startForegroundService(Intent(context, SpeechService::class.java).setAction("start")) }
            catch (error: Exception) { pending.compareAndSet(plan, null); throw error }
        }
        internal fun status(): JSONObject = JSONObject(snapshot.toString())
        internal fun stop(context: Context) { pending.set(null); context.stopService(Intent(context, SpeechService::class.java)) }
    }
    private class Playback(val plan: SpeechPlan) {
        @Volatile var cancelled = false
        @Volatile var paused = false
        @Volatile var connection: java.io.InputStream? = null
        @Volatile var track: AudioTrack? = null
        var frames = 0L
        fun check() { if (cancelled) throw InterruptedException() }
        fun waitPlaying() { check(); while (paused) { Thread.sleep(100); check() } }
        fun cancel() { cancelled = true; runCatching { connection?.close() }; track?.let { runCatching { it.pause(); it.flush() } } }
    }
    internal var http: (SpeechCall, Boolean) -> java.io.InputStream = { call, wav -> NativeSpeechInputStream(call, wav) }
    private val handler = Handler(Looper.getMainLooper())
    private val worker = Executors.newSingleThreadExecutor()
    private var current: Playback? = null
    private lateinit var session: MediaSession
    private lateinit var manager: AudioManager
    private lateinit var focus: AudioFocusRequest
    private lateinit var wake: PowerManager.WakeLock
    private var resumeOnFocus = false
    private val noisy = object : BroadcastReceiver() {
        override fun onReceive(context: Context?, intent: Intent?) { if (intent?.action == AudioManager.ACTION_AUDIO_BECOMING_NOISY) pause() }
    }
    override fun onBind(intent: Intent?): IBinder? = null
    override fun onCreate() {
        super.onCreate()
        manager = getSystemService(AudioManager::class.java)
        wake = getSystemService(PowerManager::class.java).newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "Lethetic:response-audio").apply { setReferenceCounted(false) }
        session = MediaSession(this, "Lethetic response").apply {
            setMetadata(MediaMetadata.Builder().putString(MediaMetadata.METADATA_KEY_TITLE, "Lethetic response").putString(MediaMetadata.METADATA_KEY_ARTIST, "Lethetic").build())
            setCallback(object : MediaSession.Callback() {
                override fun onPlay() { resume() }
                override fun onPause() { pause() }
                override fun onStop() { stopSelf() }
            }); isActive = true
        }
        focus = AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN)
            .setAudioAttributes(attributes()).setWillPauseWhenDucked(true)
            .setOnAudioFocusChangeListener({ change ->
                when (change) {
                    AudioManager.AUDIOFOCUS_GAIN -> if (resumeOnFocus) { resumeOnFocus = false; resume() }
                    AudioManager.AUDIOFOCUS_LOSS -> { pause(); resumeOnFocus = false }
                    AudioManager.AUDIOFOCUS_LOSS_TRANSIENT, AudioManager.AUDIOFOCUS_LOSS_TRANSIENT_CAN_DUCK -> {
                        val wasPlaying = current?.paused == false; pause(); resumeOnFocus = wasPlaying
                    }
                }
            }, handler).build()
        getSystemService(NotificationManager::class.java).createNotificationChannel(NotificationChannel(CHANNEL, "Response audio", NotificationManager.IMPORTANCE_LOW))
        if (Build.VERSION.SDK_INT >= 33) registerReceiver(noisy, IntentFilter(AudioManager.ACTION_AUDIO_BECOMING_NOISY), RECEIVER_NOT_EXPORTED)
        else registerReceiver(noisy, IntentFilter(AudioManager.ACTION_AUDIO_BECOMING_NOISY))
    }
    private fun attributes() = AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_MEDIA).setContentType(AudioAttributes.CONTENT_TYPE_SPEECH).build()
    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            "stop" -> stopSelf()
            "pause" -> pause()
            "play" -> resume()
            "start" -> {
                // Must be foreground before requesting focus on Android 15+.
                startForeground(NOTIFICATION, notification(false))
                val plan = pending.getAndSet(null) ?: run { stopSelf(startId); return START_NOT_STICKY }
                current?.cancel()
                val playback = Playback(plan); current = playback
                publish(playback, true)
                if (manager.requestAudioFocus(focus) != AudioManager.AUDIOFOCUS_REQUEST_GRANTED) {
                    finish(playback, "Audio focus unavailable. Stop other audio and retry.")
                } else {
                    wake.acquire(3 * 60 * 60 * 1000L)
                    updateMedia(false)
                    worker.execute { play(playback) }
                }
            }
        }
        return START_NOT_STICKY
    }
    private fun publish(playback: Playback, active: Boolean, error: String = "") {
        snapshot = JSONObject().put("id", playback.plan.id).put("key", playback.plan.key)
            .put("active", active).put("paused", playback.paused).put("error", error)
            .put("ready", SpeechCache(java.io.File(cacheDir, "audio")).file(playback.plan.key).isFile)
    }
    private fun play(playback: Playback) {
        var failure = ""
        var stage = "cache access"
        try {
            val cache = SpeechCache(java.io.File(cacheDir, "audio"))
            cache.stream(playback.plan, { call ->
                playback.waitPlaying()
                stage = "endpoint connection"
                val input = http(call, playback.plan.wav)
                playback.connection = input
                playback.check()
                stage = "stream decoding"
                input
            }, { playback.waitPlaying() }) { format, bytes, length ->
                playback.waitPlaying()
                stage = "audio output"
                val track = playback.track ?: AudioTrack.Builder().setAudioAttributes(attributes())
                    .setAudioFormat(android.media.AudioFormat.Builder().setEncoding(android.media.AudioFormat.ENCODING_PCM_16BIT)
                        .setSampleRate(format.rate).setChannelMask(if (format.channels == 1) android.media.AudioFormat.CHANNEL_OUT_MONO else android.media.AudioFormat.CHANNEL_OUT_STEREO).build())
                    .setBufferSizeInBytes(maxOf(16384, AudioTrack.getMinBufferSize(format.rate, if (format.channels == 1) android.media.AudioFormat.CHANNEL_OUT_MONO else android.media.AudioFormat.CHANNEL_OUT_STEREO, android.media.AudioFormat.ENCODING_PCM_16BIT)))
                    .setTransferMode(AudioTrack.MODE_STREAM).build().also { playback.track = it; playback.check(); it.play() }
                var offset = 0
                while (offset < length) {
                    playback.waitPlaying(); val n = track.write(bytes, offset, length - offset, AudioTrack.WRITE_BLOCKING)
                    require(n > 0) { "Android audio output failed" }; offset += n
                }
                playback.frames += length / (format.channels * 2)
                stage = "stream decoding"
            }
            handler.post { if (current === playback) publish(playback, true) }
            while ((playback.track?.playbackHeadPosition?.toLong()?.and(0xffffffffL) ?: 0) < playback.frames) { playback.waitPlaying(); Thread.sleep(30) }
        } catch (error: Exception) {
            // Do not expose provider bodies, URLs or header values in diagnostics.
            failure = when (error) {
                is IllegalArgumentException -> error.message ?: "Unsupported speech audio"
                else -> SpeechFailure.describe(error, stage)
            }
        } finally {
            runCatching { playback.connection?.close() }; playback.track?.let { runCatching { it.stop(); it.release() } }; playback.track = null
            handler.post { if (current === playback) finish(playback, if (playback.cancelled) "" else failure) }
        }
    }
    private fun finish(playback: Playback, error: String) {
        publish(playback, false, error); current = null; stopSelf()
    }
    private fun pause() {
        resumeOnFocus = false
        current?.let { it.paused = true; runCatching { it.track?.pause() }; publish(it, true) }
        if (wake.isHeld) wake.release(); updateMedia(true)
    }
    private fun resume() {
        val playback = current ?: return
        if (manager.requestAudioFocus(focus) != AudioManager.AUDIOFOCUS_REQUEST_GRANTED) return
        wake.acquire(3 * 60 * 60 * 1000L); runCatching { playback.track?.play() }; playback.paused = false
        publish(playback, true); updateMedia(false)
    }
    private fun updateMedia(paused: Boolean) {
        session.setPlaybackState(PlaybackState.Builder().setActions(PlaybackState.ACTION_PLAY or PlaybackState.ACTION_PAUSE or PlaybackState.ACTION_STOP)
            .setState(if (paused) PlaybackState.STATE_PAUSED else PlaybackState.STATE_PLAYING, PlaybackState.PLAYBACK_POSITION_UNKNOWN, if (paused) 0f else 1f).build())
        getSystemService(NotificationManager::class.java).notify(NOTIFICATION, notification(paused))
    }
    private fun notification(paused: Boolean): Notification {
        fun action(name: String) = PendingIntent.getService(this, name.hashCode(), Intent(this, SpeechService::class.java).setAction(name), PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
        val launch = packageManager.getLaunchIntentForPackage(packageName)?.let { PendingIntent.getActivity(this, 713, it, PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE) }
        return Notification.Builder(this, CHANNEL).setContentTitle("Lethetic response").setContentText(if (paused) "Audio paused" else "Playing response audio")
            .setSmallIcon(android.R.drawable.ic_media_play).setContentIntent(launch).setVisibility(Notification.VISIBILITY_PRIVATE).setOnlyAlertOnce(true).setOngoing(!paused)
            .addAction(Notification.Action.Builder(android.graphics.drawable.Icon.createWithResource(this, if (paused) android.R.drawable.ic_media_play else android.R.drawable.ic_media_pause), if (paused) "Play" else "Pause", action(if (paused) "play" else "pause")).build())
            .addAction(Notification.Action.Builder(android.graphics.drawable.Icon.createWithResource(this, android.R.drawable.ic_menu_close_clear_cancel), "Stop", action("stop")).build())
            .setStyle(Notification.MediaStyle().setMediaSession(session.sessionToken).setShowActionsInCompactView(0, 1)).build()
    }
    override fun onDestroy() {
        current?.let { it.cancel(); publish(it, false) }; current = null
        worker.shutdown(); manager.abandonAudioFocusRequest(focus)
        if (wake.isHeld) wake.release()
        unregisterReceiver(noisy); session.release(); stopForeground(STOP_FOREGROUND_REMOVE)
        super.onDestroy()
    }
}
