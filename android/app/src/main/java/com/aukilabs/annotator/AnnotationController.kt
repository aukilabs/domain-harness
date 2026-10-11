package com.aukilabs.annotator

import androidx.compose.runtime.*
import org.json.JSONArray
import org.json.JSONObject
import java.util.UUID

internal data class ScreenNote(val note: JSONObject, val x: Float, val y: Float, val pending: Boolean)
internal data class AnnotationPick(val frame: String, val double: Boolean, val note: JSONObject? = null,
    val origin: FloatArray? = null, val position: FloatArray? = null, val error: String? = null)

/** Main-thread UI state. Home acknowledgements, never speculative taps, confirm notes. */
internal class AnnotationController(
    private val send: (ByteArray) -> Boolean = NativeBridge::changeNote,
    private val notify: (String, String?, (() -> Unit)?) -> Unit,
) {
    var editor by mutableStateOf<JSONObject?>(null); private set
    var text by mutableStateOf("")
    private var frame: String? = null
    private var scene = emptyList<JSONObject>()
    private var pending: JSONObject? = null
    private var preview: JSONObject? = null
    // Keep acknowledged mutations over an older scene snapshot until reflected back.
    private data class Ack(val note: JSONObject?,val expires: Long)
    private val acknowledged = mutableMapOf<String,Ack>()
    var renderNotes: (String) -> Unit = {}
    var accepted: () -> Unit = {}

    fun scene(frame: String?, notes: JSONArray?) {
        if (this.frame != frame) {
            pending = null; preview = null; acknowledged.clear(); editor = null
        }
        this.frame = frame
        scene = (0 until (notes?.length() ?: 0)).map { notes!!.getJSONObject(it) }
            .filter { !it.has("anchor_robot") && !it.has("phone_pose") && !it.has("robot_pose") }
        acknowledged.entries.removeAll { (id,ack) ->
            val note = ack.note
            val current = scene.find { it.optString("id") == id }
            System.nanoTime() >= ack.expires || (if (note == null) current == null else current != null && current.optString("text") == note.optString("text") &&
                current.optJSONArray("position")?.toString() == note.optJSONArray("position")?.toString())
        }
        render()
    }
    private fun render() {
        val visible = scene.associateBy { it.getString("id") }.toMutableMap()
        acknowledged.forEach { (id,ack) -> if (ack.note == null) visible.remove(id) else visible[id] = ack.note }
        preview?.let { visible[it.getString("id")] = it }
        renderNotes(JSONArray(visible.values.toList()).toString())
    }
    fun refreshRender() = render()
    fun expireAcknowledgements() {
        if (acknowledged.entries.removeAll { System.nanoTime() >= it.value.expires }) render()
    }
    fun pick(hit: AnnotationPick) {
        if (pending != null) { notify("Waiting for Home…",null,null); return }
        if (hit.error != null) { notify(hit.error,null,null); return }
        if (frame == null || hit.frame != frame) { notify("Scan this scene's portal before annotating",null,null); return }
        val note = hit.note
        if (note != null) {
            if (hit.double) submit(JSONObject().put("kind","delete").put("id",note.getString("id")),note)
            else { editor = note; text = note.optString("text","Note") }
        } else if (hit.double && hit.position != null && hit.origin != null) {
            val id = UUID.randomUUID().toString()
            val position = JSONArray(hit.position.map { it.toDouble() })
            submit(JSONObject().put("kind","place").put("id",id).put("text","Note")
                .put("origin",JSONArray(hit.origin.map { it.toDouble() })).put("position",position),
                JSONObject().put("id",id).put("text","Note").put("position",position))
        }
    }
    fun dismissEditor() { editor = null }
    fun saveEditor() {
        val note = editor ?: return
        val value = text.trim()
        if (value.isEmpty() || value.codePointCount(0,value.length) > 120 || value.any { it.isISOControl() }) {
            notify("Use 1–120 printable characters",null,null); return
        }
        editor = null
        submit(JSONObject().put("kind","edit").put("id",note.getString("id")).put("text",value),note)
    }
    private fun submit(action: JSONObject, note: JSONObject?, request: JSONObject? = null) {
        val currentFrame = frame ?: return
        if (pending != null) return
        val command = request ?: JSONObject().put("request_id",UUID.randomUUID().toString()).put("frame",currentFrame).put("action",action)
        if (command.getString("frame") != currentFrame) return
        if (!send(command.toString().toByteArray())) { notify("Connect to Home before annotating",null,null); return }
        pending = command
        accepted()
        preview = note?.let { JSONObject(it.toString()).put("pending",true) }
        render()
    }
    fun result(result: JSONObject?) {
        val command = pending ?: return
        if (result == null || result.optString("id") != command.getString("request_id")) return
        val oldPreview = preview
        pending = null; preview = null
        if (!result.optBoolean("saved")) {
            render()
            val detail = result.optString("detail","Home did not confirm the change")
            // Only locally classified operation failures; no note text, poses or auth material.
            if (detail.startsWith("Home did not confirm the operation ("))
                android.util.Log.w("AnnotatorNote",detail)
            notify(detail,"Retry") {
                submit(command.getJSONObject("action"),oldPreview,command)
            }
            return
        }
        val note = result.optJSONObject("note")
        val deleted = result.optBoolean("deleted")
        val id = note?.optString("id") ?: command.getJSONObject("action").optString("id")
        if (id.isNotBlank()) acknowledged[id] = Ack(if (deleted) null else note,System.nanoTime()+5_000_000_000L)
        render()
        if (deleted && note != null) {
            val deletedFrame = frame
            notify("Note deleted","Undo") {
                if (frame == deletedFrame) submit(JSONObject().put("kind","restore").put("note",note),note)
            }
        }
    }
}
