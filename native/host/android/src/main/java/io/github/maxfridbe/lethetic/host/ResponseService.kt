package io.github.maxfridbe.lethetic.host

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Intent
import android.os.Build
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.os.PowerManager

/** A visible, bounded lease; never an idle or boot-started service. */
class ResponseService : Service() {
    private var wake: PowerManager.WakeLock? = null
    private val handler = Handler(Looper.getMainLooper())
    private val expire = Runnable { stopSelf() }
    override fun onBind(intent: Intent?): IBinder? = null
    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val working = intent?.getBooleanExtra("working", false) == true
        val manager = getSystemService(NotificationManager::class.java)
        if (Build.VERSION.SDK_INT >= 26) manager.createNotificationChannel(NotificationChannel("response", "Model response", NotificationManager.IMPORTANCE_LOW))
        val builder = if (Build.VERSION.SDK_INT >= 26) Notification.Builder(this, "response") else Notification.Builder(this)
        val launch = packageManager.getLaunchIntentForPackage(packageName)
        if (launch != null) builder.setContentIntent(PendingIntent.getActivity(this, 0, launch, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT))
        startForeground(71, builder.setSmallIcon(android.R.drawable.stat_notify_sync).setContentTitle("Lethetic")
            .setContentText(if (working) "Waiting for a model response" else "Session work in progress").setOngoing(true).build())
        if (!working) { wake?.let { if (it.isHeld) it.release() }; wake = null }
        if (working && wake?.isHeld != true) {
            wake = getSystemService(PowerManager::class.java).newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "lethetic:response").also { it.acquire(30 * 60 * 1000L) }
        }
        handler.removeCallbacks(expire); handler.postDelayed(expire, 30 * 60 * 1000L)
        return START_NOT_STICKY
    }
    override fun onDestroy() {
        handler.removeCallbacks(expire)
        wake?.let { if (it.isHeld) it.release() }; wake = null
        super.onDestroy()
    }
}
