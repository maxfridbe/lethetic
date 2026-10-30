package io.github.maxfridbe.lethetic.host

import android.Manifest
import android.app.Activity
import app.tauri.plugin.Invoke
import com.fasterxml.jackson.databind.ObjectMapper
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.Robolectric
import org.robolectric.RobolectricTestRunner
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.Config

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [35])
class MicrophonePermissionTest {
    @Test fun permissionIsCheckedAgainAfterRevocationWithoutOpeningMicrophone() {
        val activity = Robolectric.buildActivity(Activity::class.java).setup().get()
        val plugin = HostPlugin(activity)
        var result = ""
        val invoke = Invoke(1, "checkMicrophone", 2, 3, { callback, data ->
            assertEquals(2L, callback)
            result = data
        }, "{}", ObjectMapper())
        shadowOf(activity.application).denyPermissions(Manifest.permission.RECORD_AUDIO)
        plugin.checkMicrophone(invoke)
        assertFalse(JSONObject(result).getBoolean("granted"))
        shadowOf(activity.application).grantPermissions(Manifest.permission.RECORD_AUDIO)
        plugin.requestMicrophone(invoke) // Existing grant resolves; no permission dialog/capture.
        assertTrue(JSONObject(result).getBoolean("granted"))
        shadowOf(activity.application).denyPermissions(Manifest.permission.RECORD_AUDIO)
        plugin.microphoneResult(invoke) // Denied permission callback must not report success.
        assertFalse(JSONObject(result).getBoolean("granted"))
    }
}
