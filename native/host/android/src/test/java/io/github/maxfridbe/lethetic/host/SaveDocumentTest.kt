package io.github.maxfridbe.lethetic.host

import android.app.Activity
import android.content.Intent
import android.net.Uri
import java.io.ByteArrayOutputStream
import java.io.File
import java.io.IOException
import java.io.OutputStream
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.Robolectric
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [35])
class SaveDocumentTest {
    @Test fun pickerUsesCreateDocumentWithNameAndMimeWithoutFolderAccess() {
        val intent = SaveDocument.intent("session.tar.gz", "application/gzip")
        assertEquals(Intent.ACTION_CREATE_DOCUMENT, intent.action)
        assertTrue(intent.hasCategory(Intent.CATEGORY_OPENABLE))
        assertEquals("session.tar.gz", intent.getStringExtra(Intent.EXTRA_TITLE))
        assertEquals("application/gzip", intent.type)
        assertEquals(0, intent.flags and Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION)
    }
    @Test fun genericBinaryDownloadsKeepRecognizableImageMimeTypes() {
        assertEquals("image/png", SaveDocument.intent("plot.PNG", "application/octet-stream").type)
        assertEquals("application/octet-stream", SaveDocument.intent("data.unknown-extension", "application/octet-stream").type)
    }
    @Test fun selectedDocumentReceivesBinaryBytesAndClosesOutput() {
        val activity = Robolectric.buildActivity(Activity::class.java).setup().get()
        val directory = File(activity.cacheDir, "downloads").apply { mkdirs() }
        val source = File.createTempFile("lethetic-download-", ".tmp", directory)
        val bytes = ByteArray(700_000) { (it % 256).toByte() }; source.writeBytes(bytes)
        var closed = false
        val output = object : ByteArrayOutputStream() { override fun close() { closed = true; super.close() } }
        val uri = Uri.parse("content://test.documents/output")
        assertEquals(source.canonicalFile, SaveDocument.source(activity.cacheDir, source.path))
        assertTrue(SaveDocument.complete(Activity.RESULT_OK, uri, source) { selected -> assertEquals(uri, selected); output })
        assertArrayEquals(bytes, output.toByteArray()); assertTrue(closed)
        source.delete()
    }
    @Test fun cancellationNeverOpensDestinationAndFailuresAreNotReportedAsSaved() {
        val source = File.createTempFile("download", ".tmp"); source.writeBytes(byteArrayOf(0, -1, 1))
        val uri = Uri.parse("content://test.documents/output")
        assertFalse(SaveDocument.complete(Activity.RESULT_CANCELED, uri, source) { error("Must not open on cancel") })
        assertFalse(SaveDocument.complete(Activity.RESULT_OK, null, source) { error("Must not open without URI") })
        assertThrows(IllegalStateException::class.java) { SaveDocument.complete(Activity.RESULT_OK, uri, source) { null } }
        var closed = false
        assertThrows(IOException::class.java) {
            SaveDocument.complete(Activity.RESULT_OK, uri, source) { object : OutputStream() {
                override fun write(value: Int) { throw IOException("Disk full") }
                override fun close() { closed = true }
            } }
        }
        assertTrue(closed); source.delete()
    }
    @Test fun sourcesOutsideAppDownloadStagingAreRejected() {
        val activity = Robolectric.buildActivity(Activity::class.java).setup().get()
        val outside = File.createTempFile("private", ".tmp", activity.cacheDir)
        assertThrows(IllegalArgumentException::class.java) { SaveDocument.source(activity.cacheDir, outside.path) }
        outside.delete()
    }
}
