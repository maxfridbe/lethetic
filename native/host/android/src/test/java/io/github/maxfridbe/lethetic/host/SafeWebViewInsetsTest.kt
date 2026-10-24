package io.github.maxfridbe.lethetic.host

import android.app.Activity
import android.view.View
import android.view.ViewGroup
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
    private lateinit var viewport: View

    @Before fun setup() {
        activity = Robolectric.buildActivity(Activity::class.java).setup().get()
        // Robolectric's WebView provider stubs setFrame(), so its bounds stay zero.
        // Use a match-parent View to exercise the native viewport's real layout.
        viewport = View(activity)
        activity.setContentView(viewport)
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
        assertEquals(24, viewport.top)
        assertEquals(752, viewport.bottom)
        assertEquals(Insets.NONE, remaining.getInsets(WindowInsetsCompat.Type.systemBars()))
        dispatch(Insets.of(0, 24, 0, 48))
        assertEquals(24, viewport.top)
        assertEquals(752, viewport.bottom)
    }

    @Test fun keyboardReplacesNavigationInsetAndRestoresOnDismissal() {
        val remaining = dispatch(Insets.of(0, 24, 0, 24), keyboard = 300)
        assertEquals(500, viewport.bottom)
        assertEquals(Insets.NONE, remaining.getInsets(WindowInsetsCompat.Type.ime()))
        dispatch(Insets.of(0, 24, 0, 24))
        assertEquals(776, viewport.bottom)
    }

    @Test fun landscapeCutoutAndChangedNavigationEdgesRemainOutsideWebView() {
        dispatch(Insets.of(0, 0, 24, 0), cutout = Insets.of(60, 0, 0, 0))
        assertEquals(60, viewport.left)
        assertEquals(376, viewport.right)
        dispatch(Insets.of(0, 24, 0, 48))
        assertEquals(0, viewport.left)
        assertEquals(400, viewport.right)
    }
}
