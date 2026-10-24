package io.github.maxfridbe.lethetic.host

import android.content.ContentResolver
import android.util.Base64
import androidx.documentfile.provider.DocumentFile
import org.json.JSONArray
import org.json.JSONObject

/** SAF tree grant, never raw paths or MANAGE_EXTERNAL_STORAGE. No disk deletions. */
internal class TreeWorkspace(private val resolver: ContentResolver, private val root: DocumentFile) {
    private val baseline = mutableMapOf<String, ByteArray>()
    private val fileLimit = 16 * 1024 * 1024
    private val totalLimit = 64 * 1024 * 1024
    private fun parts(path: String): List<String> {
        require(path.isNotEmpty() && path.length <= 4096 && !path.contains('\\') && !path.contains(':') && !path.contains('\u0000')) { "Invalid file path" }
        return path.split('/').also { require(it.all { part -> part.isNotEmpty() && !part.startsWith('.') }) { "Hidden or relative paths are excluded" } }
    }
    private fun bytes(file: DocumentFile): ByteArray {
        require(file.isFile && file.length() <= fileLimit) { "Unsupported or oversized file" }
        return (resolver.openInputStream(file.uri) ?: error("Cannot read file")).use { input ->
            val output = java.io.ByteArrayOutputStream(); val buffer = ByteArray(8192)
            while (true) { val count = input.read(buffer); if (count < 0) break; require(output.size() + count <= fileLimit) { "File exceeds 16 MiB" }; output.write(buffer, 0, count) }
            output.toByteArray()
        }
    }
    fun read(): JSONObject {
        val folders = JSONArray(); val files = JSONArray(); var total = 0
        fun visit(directory: DocumentFile, prefix: String) {
            for (entry in directory.listFiles()) {
                val name = entry.name ?: continue
                if (name.startsWith('.') || name.contains('/') || name.contains('\\') || name.contains(':')) continue
                val path = prefix + name; parts(path)
                if (entry.isDirectory) {
                    require(folders.length() < 256) { "Directory exceeds 256 folders" }
                    folders.put(path); visit(entry, "$path/")
                } else if (entry.isFile) {
                    require(files.length() < 100) { "Directory exceeds 100 files" }
                    val data = bytes(entry); total += data.size; require(total <= totalLimit) { "Directory exceeds 64 MiB" }
                    baseline[path] = data
                    files.put(JSONObject().put("path", path).put("data", Base64.encodeToString(data, Base64.NO_WRAP)))
                }
            }
        }
        visit(root, "")
        return JSONObject().put("version", 1).put("directories", folders).put("files", files)
    }
    private fun find(path: String, createParents: Boolean): Pair<DocumentFile?, String> {
        val segments = parts(path); var parent = root
        for (segment in segments.dropLast(1)) {
            val next = parent.findFile(segment) ?: if (createParents) parent.createDirectory(segment) else null
            if (next == null) return Pair(null, segments.last())
            require(next.isDirectory) { "Parent is not a directory" }; parent = next
        }
        return Pair(parent, segments.last())
    }
    fun sync(workspace: JSONObject) {
        require(workspace.getInt("version") == 1)
        val files = workspace.getJSONArray("files"); val directories = workspace.getJSONArray("directories")
        require(files.length() <= 100 && directories.length() <= 256)
        val incoming = linkedMapOf<String, ByteArray>(); var total = 0
        for (index in 0 until files.length()) {
            val file = files.getJSONObject(index); val path = file.getString("path"); parts(path)
            val encoded = file.getString("data"); require(encoded.length <= ((fileLimit + 2) / 3) * 4)
            val data = Base64.decode(encoded, Base64.NO_WRAP); total += data.size
            require(data.size <= fileLimit && total <= totalLimit && !incoming.containsKey(path))
            incoming[path] = data
        }
        val folderPaths = (0 until directories.length()).map { directories.getString(it).also { path -> parts(path) } }
        require(incoming.keys.none { it in folderPaths || incoming.keys.any { other -> other.startsWith("$it/") } }) { "File and directory conflict" }
        val allFolders = folderPaths.toMutableSet()
        for (path in incoming.keys + folderPaths) {
            val segments = parts(path)
            for (count in 1 until segments.size) allFolders.add(segments.take(count).joinToString("/"))
        }
        require(allFolders.size <= 256) { "Workspace exceeds 256 folders including parents" }
        val changed = incoming.filter { (path, data) -> baseline[path]?.contentEquals(data) != true }
        for ((path, _) in changed) {
            val (parent, name) = find(path, false); val existing = parent?.findFile(name)
            val current = existing?.let { bytes(it) }; val previous = baseline[path]
            require(if (previous == null) current == null else current?.contentEquals(previous) == true) { "External edit conflict: $path. Reopen the folder." }
        }
        for (path in folderPaths) { val (parent, name) = find(path, true); if (parent?.findFile(name) == null) require(parent?.createDirectory(name) != null) }
        for ((path, data) in changed) {
            val (parent, name) = find(path, true)
            val file = parent?.findFile(name) ?: parent?.createFile("application/octet-stream", name) ?: error("Cannot create $path")
            // SAF providers do not promise atomic replacement. Keep the virtual copy
            // and surface failures so users can retry/export instead of losing it.
            (resolver.openOutputStream(file.uri, "wt") ?: error("Cannot write $path")).use { it.write(data) }
            baseline[path] = data
        }
    }
}
