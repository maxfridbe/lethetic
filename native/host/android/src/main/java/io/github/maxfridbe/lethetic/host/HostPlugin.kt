package io.github.maxfridbe.lethetic.host

import android.app.Activity
import android.content.Intent
import android.os.Build
import android.webkit.WebView
import androidx.activity.result.ActivityResult
import androidx.appcompat.app.AppCompatActivity
import androidx.documentfile.provider.DocumentFile
import app.tauri.annotation.ActivityCallback
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import java.util.concurrent.Executors

@InvokeArg class WorkArgs { var working: Boolean = false; var active: Boolean = false }
@TauriPlugin
class HostPlugin(private val activity: Activity) : Plugin(activity) {
    private val io = Executors.newSingleThreadExecutor()
    private var directory: TreeWorkspace? = null
    private var webView: WebView? = null
    private var waiting = false
    private var askedNotifications = false
    private var serviceRunning = false
    override fun load(webView: WebView) {
        this.webView = webView
        activity.runOnUiThread { SafeWebViewInsets.install(activity) }
    }
    private fun task(invoke: Invoke, work: () -> JSObject) {
        io.execute { try { invoke.resolve(work()) } catch (error: Exception) { invoke.reject(error.message ?: "Workspace operation failed") } }
    }
    @Command fun chooseDirectory(invoke: Invoke) {
        val intent = Intent(Intent.ACTION_OPEN_DOCUMENT_TREE).addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION)
        startActivityForResult(invoke, intent, "directoryResult")
    }
    @ActivityCallback fun directoryResult(invoke: Invoke, result: ActivityResult) {
        val uri = result.data?.data
        if (result.resultCode != Activity.RESULT_OK || uri == null) { invoke.reject("Directory selection cancelled"); return }
        task(invoke) {
            val tree = DocumentFile.fromTreeUri(activity, uri) ?: error("Directory unavailable")
            val granted = TreeWorkspace(activity.contentResolver, tree)
            val snapshot = granted.read()
            directory = granted
            JSObject().put("name", tree.name ?: "Selected folder").put("workspace", snapshot) as JSObject
        }
    }
    @Command fun syncDirectory(invoke: Invoke) {
        task(invoke) { (directory ?: error("No directory granted")).sync(invoke.getArgs().getJSONObject("workspace")); JSObject() }
    }
    @Command fun revokeDirectory(invoke: Invoke) { task(invoke) { directory = null; JSObject() } }
    @Command fun setWorking(invoke: Invoke) {
        try {
            val args = invoke.parseArgs(WorkArgs::class.java)
            waiting = args.active
            val intent = Intent(activity, ResponseService::class.java).putExtra("working", args.working)
            if (waiting) {
                if (Build.VERSION.SDK_INT >= 33 && !askedNotifications && activity.checkSelfPermission(android.Manifest.permission.POST_NOTIFICATIONS) != android.content.pm.PackageManager.PERMISSION_GRANTED) {
                    askedNotifications = true
                    activity.runOnUiThread { activity.requestPermissions(arrayOf(android.Manifest.permission.POST_NOTIFICATIONS), 710) }
                }
                if (Build.VERSION.SDK_INT >= 26 && !serviceRunning) activity.startForegroundService(intent) else activity.startService(intent)
                serviceRunning = true
                webView?.onResume()
            } else { activity.stopService(intent); serviceRunning = false }
            invoke.resolve()
        } catch (error: Exception) { waiting = false; activity.stopService(Intent(activity, ResponseService::class.java)); serviceRunning = false; invoke.reject(error.message ?: "Keep-awake unavailable") }
    }
    // Tauri pauses its WebView with the Activity. Resume this WebView while a response
    // is pending, without changing global WebView timers or granting native code tools.
    override fun onPause(activity: AppCompatActivity) { if (waiting) webView?.post { if (waiting) webView?.onResume() } }
    override fun onDestroy(activity: AppCompatActivity) {
        waiting = false; activity.stopService(Intent(activity, ResponseService::class.java))
        webView = null; io.shutdown()
    }
}
