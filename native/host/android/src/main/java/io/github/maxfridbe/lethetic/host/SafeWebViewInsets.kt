package io.github.maxfridbe.lethetic.host

import android.app.Activity
import android.view.View
import android.view.WindowManager
import androidx.core.graphics.Insets
import androidx.core.view.ViewCompat
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat

/** Keep the entire WebView (including fixed dialogs) inside Android's usable area. */
internal object SafeWebViewInsets {
    fun install(activity: Activity) {
        WindowCompat.setDecorFitsSystemWindows(activity.window, false)
        activity.window.setSoftInputMode(WindowManager.LayoutParams.SOFT_INPUT_ADJUST_RESIZE)
        val content = activity.findViewById<View>(android.R.id.content)
        val bars = WindowInsetsCompat.Type.systemBars() or WindowInsetsCompat.Type.displayCutout()
        val handled = bars or WindowInsetsCompat.Type.ime()
        ViewCompat.setOnApplyWindowInsetsListener(content) { view, insets ->
            // getInsets unions the edges: the keyboard replaces, rather than adds to,
            // the navigation inset. Absolute padding also makes repeated events safe.
            val safe = insets.getInsets(handled)
            view.setPadding(safe.left, safe.top, safe.right, safe.bottom)
            // Dispatch zero values, not CONSUMED: WebView must receive subsequent
            // updates, without applying the same bars/keyboard a second time in CSS.
            WindowInsetsCompat.Builder(insets)
                .setInsets(handled, Insets.NONE)
                .setInsetsIgnoringVisibility(bars, Insets.NONE)
                .build()
        }
        ViewCompat.requestApplyInsets(content)
    }
}
