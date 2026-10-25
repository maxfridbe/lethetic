package io.github.maxfridbe.lethetic.host

import android.app.Activity
import android.content.Intent
import android.net.Uri
import android.webkit.MimeTypeMap
import java.io.File
import java.io.OutputStream

/** ACTION_CREATE_DOCUMENT grants write access to just the user's selected output. */
internal object SaveDocument {
    fun intent(name: String, mime: String): Intent = Intent(Intent.ACTION_CREATE_DOCUMENT)
        .addCategory(Intent.CATEGORY_OPENABLE)
        .setType(if (mime == "application/octet-stream") MimeTypeMap.getSingleton().getMimeTypeFromExtension(name.substringAfterLast('.', "").lowercase()) ?: mime else mime)
        .putExtra(Intent.EXTRA_TITLE, name)
        .addFlags(Intent.FLAG_GRANT_WRITE_URI_PERMISSION)

    fun source(cache: File, path: String): File {
        val file = File(path).canonicalFile
        require(file.path.startsWith(File(cache, "downloads").canonicalPath + File.separator) && file.isFile) { "Download source unavailable" }
        return file
    }

    fun complete(resultCode: Int, uri: Uri?, source: File, open: (Uri) -> OutputStream?): Boolean {
        if (resultCode != Activity.RESULT_OK || uri == null) return false
        require(uri.scheme == "content") { "Unsupported save location" }
        val output = open(uri) ?: error("Unable to open selected save location")
        output.use { destination -> source.inputStream().use { it.copyTo(destination) }; destination.flush() }
        return true
    }
}
