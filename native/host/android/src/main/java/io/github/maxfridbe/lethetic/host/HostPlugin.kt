package io.github.maxfridbe.lethetic.host

import android.app.Activity
import android.Manifest
import android.content.pm.PackageManager
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
import app.tauri.annotation.Permission
import app.tauri.annotation.PermissionCallback
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleOwner
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import java.util.concurrent.Executors

@InvokeArg class WorkArgs { var working: Boolean = false; var active: Boolean = false }
@InvokeArg class SiteArgs { var url: String = "" }
@InvokeArg class SaveArgs { var path: String = ""; var name: String = ""; var mime: String = "application/octet-stream" }
@TauriPlugin(permissions = [Permission(strings = [Manifest.permission.RECORD_AUDIO], alias = "microphone")])
class HostPlugin(private val activity: Activity) : Plugin(activity) {
    private external fun setDictationForegroundNative(active: Boolean)
    private val io = Executors.newSingleThreadExecutor()
    private var directory: TreeWorkspace? = null
    private var webView: WebView? = null
    private var waiting = false
    private var askedNotifications = false
    private var serviceRunning = false
    override fun load(webView: WebView) {
        this.webView = webView
        setDictationForegroundNative((activity as? LifecycleOwner)?.lifecycle?.currentState?.isAtLeast(Lifecycle.State.RESUMED) == true)
        activity.runOnUiThread { SafeWebViewInsets.install(activity) }
    }
    @Command fun checkMicrophone(invoke: Invoke) {
        invoke.resolve(JSObject().put("granted", activity.checkSelfPermission(Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED) as JSObject)
    }
    @Command fun requestMicrophone(invoke: Invoke) {
        if (activity.checkSelfPermission(Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED) {
            checkMicrophone(invoke)
        } else {
            requestPermissionForAlias("microphone", invoke, "microphoneResult")
        }
    }
    @PermissionCallback fun microphoneResult(invoke: Invoke) { checkMicrophone(invoke) }
    private fun task(invoke: Invoke, work: () -> JSObject) {
        io.execute { try { invoke.resolve(work()) } catch (error: Exception) { invoke.reject(error.message ?: "Workspace operation failed") } }
    }
    @Command fun saveDownload(invoke: Invoke) {
        try {
            val args = invoke.parseArgs(SaveArgs::class.java)
            SaveDocument.source(activity.cacheDir, args.path)
            startActivityForResult(invoke, SaveDocument.intent(args.name, args.mime), "downloadResult")
        } catch (error: Exception) { invoke.reject(error.message ?: "Save dialog unavailable") }
    }
    @ActivityCallback fun downloadResult(invoke: Invoke, result: ActivityResult) {
        task(invoke) {
            val args = invoke.parseArgs(SaveArgs::class.java)
            val source = SaveDocument.source(activity.cacheDir, args.path)
            val saved = SaveDocument.complete(result.resultCode, result.data?.data, source) { uri ->
                activity.contentResolver.openOutputStream(uri, "wt")
            }
            JSObject().apply { put("saved", saved) }
        }
    }
    @Command fun openSiteBrowser(invoke: Invoke) {
        try {
            val uri = android.net.Uri.parse(invoke.parseArgs(SiteArgs::class.java).url)
            require(uri.scheme == "http" && uri.host == "127.0.0.1" && uri.port in 1024..65535) { "Expected a local forwarded site" }
            activity.runOnUiThread {
                try { activity.startActivity(Intent(Intent.ACTION_VIEW, uri)); invoke.resolve() }
                catch (error: Exception) { invoke.reject(error.message ?: "No browser available") }
            }
        } catch (error: Exception) { invoke.reject(error.message ?: "Invalid forwarded site") }
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
    // Tauri pauses its WebView with the Activity. Resume while a turn or explicitly hosted site
    // is active, without changing global timers or granting native code tools.
    override fun onResume(activity: AppCompatActivity) { setDictationForegroundNative(true) }
    override fun onPause(activity: AppCompatActivity) {
        setDictationForegroundNative(false)
        if (waiting) webView?.post { if (waiting) webView?.onResume() }
    }
    override fun onDestroy(activity: AppCompatActivity) {
        setDictationForegroundNative(false)
        waiting = false; activity.stopService(Intent(activity, ResponseService::class.java))
        webView = null; io.shutdown()
    }
}
