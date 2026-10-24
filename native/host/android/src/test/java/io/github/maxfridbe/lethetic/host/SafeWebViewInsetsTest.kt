package io.github.maxfridbe.lethetic.host

import android.app.Activity
import android.view.View
import android.view.ViewGroup
import android.webkit.WebView
import androidx.core.graphics.Insets
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import org.junit.Assert.assertEquals
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.Robolectric
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [35])
class SafeWebViewInsetsTest {
    private lateinit var activity: Activity
    private lateinit var content: ViewGroup
    private lateinit var web: WebView

    @Before fun setup() {
        activity = Robolectric.buildActivity(Activity::class.java).setup().get()
        web = WebView(activity)
        activity.setContentView(web)
        content = activity.findViewById(android.R.id.content)
        SafeWebViewInsets.install(activity)
    }

    private fun dispatch(bars: Insets, cutout: Insets = Insets.NONE, keyboard: Int = 0): WindowInsetsCompat {
        val insets = WindowInsetsCompat.Builder()
            .setInsets(WindowInsetsCompat.Type.systemBars(), bars)
            .setInsets(WindowInsetsCompat.Type.displayCutout(), cutout)
            .setInsets(WindowInsetsCompat.Type.ime(), Insets.of(0, 0, 0, keyboard))
            .build()
        val remaining = ViewCompat.dispatchApplyWindowInsets(content, insets)
        content.measure(View.MeasureSpec.makeMeasureSpec(400, View.MeasureSpec.EXACTLY), View.MeasureSpec.makeMeasureSpec(800, View.MeasureSpec.EXACTLY))
        content.layout(0, 0, 400, 800)
        return remaining
    }

    @Test fun systemBarsShrinkTheWebViewAndAreNotAppliedTwice() {
        val remaining = dispatch(Insets.of(0, 24, 0, 48))
        assertEquals(24, web.top)
        assertEquals(752, web.bottom)
        assertEquals(Insets.NONE, remaining.getInsets(WindowInsetsCompat.Type.systemBars()))
        dispatch(Insets.of(0, 24, 0, 48))
        assertEquals(24, web.top)
        assertEquals(752, web.bottom)
    }

    @Test fun keyboardReplacesNavigationInsetAndRestoresOnDismissal() {
        val remaining = dispatch(Insets.of(0, 24, 0, 24), keyboard = 300)
        assertEquals(500, web.bottom)
        assertEquals(Insets.NONE, remaining.getInsets(WindowInsetsCompat.Type.ime()))
        dispatch(Insets.of(0, 24, 0, 24))
        assertEquals(776, web.bottom)
    }

    @Test fun landscapeCutoutAndChangedNavigationEdgesRemainOutsideWebView() {
        dispatch(Insets.of(0, 0, 24, 0), cutout = Insets.of(60, 0, 0, 0))
        assertEquals(60, web.left)
        assertEquals(376, web.right)
        dispatch(Insets.of(0, 24, 0, 48))
        assertEquals(0, web.left)
        assertEquals(400, web.right)
    }
}
