package com.aukilabs.annotator

import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test

class AnnotationControllerTest {
    @Test fun placeAckDeleteUndoAndSceneSwitchUseStableRequests() {
        val sent = mutableListOf<JSONObject>()
        var action: (() -> Unit)? = null
        var rendered = JSONArray()
        val controller = AnnotationController({ sent.add(JSONObject(it.toString(Charsets.UTF_8))); true }) { _,_,callback -> action = callback }
        controller.renderNotes = { rendered = JSONArray(it) }
        controller.scene("home",JSONArray())
        controller.pick(AnnotationPick("other",true,origin=floatArrayOf(0f,1f,0f),position=floatArrayOf(0f,0f,-1f)))
        assertTrue(sent.isEmpty())
        controller.pick(AnnotationPick("home",true,origin=floatArrayOf(0f,1f,0f),position=floatArrayOf(0f,0f,-1f)))
        val place = sent.last()
        assertEquals("place",place.getJSONObject("action").getString("kind"))
        assertTrue(rendered.getJSONObject(0).getBoolean("pending"))
        val note = JSONObject(rendered.getJSONObject(0).toString()).apply { remove("pending") }
        controller.result(JSONObject().put("id",place.getString("request_id")).put("saved",true).put("note",note))
        assertFalse(rendered.getJSONObject(0).optBoolean("pending"))
        controller.scene("home",JSONArray()) // Old scene snapshot cannot erase acknowledged note.
        assertEquals(1,rendered.length())
        controller.scene("home",JSONArray().put(note))
        controller.pick(AnnotationPick("home",true,note=note))
        val delete = sent.last()
        assertEquals("delete",delete.getJSONObject("action").getString("kind"))
        controller.result(JSONObject().put("id",delete.getString("request_id")).put("saved",true).put("deleted",true).put("note",note))
        assertEquals(0,rendered.length())
        action!!.invoke()
        assertEquals("restore",sent.last().getJSONObject("action").getString("kind"))
        controller.scene("other",JSONArray())
        controller.result(JSONObject().put("id",sent.last().getString("request_id")).put("saved",true).put("note",note))
        assertEquals(0,rendered.length())
    }
    @Test fun failedChangeRollsBackAndRetryReusesItsInvocationId() {
        val sent = mutableListOf<JSONObject>(); var retry: (() -> Unit)? = null
        var rendered = JSONArray()
        val controller = AnnotationController({sent.add(JSONObject(it.toString(Charsets.UTF_8))); true}) { _,_,action -> retry=action }
        controller.renderNotes = { rendered=JSONArray(it) }
        controller.scene("home",JSONArray())
        controller.pick(AnnotationPick("home",true,origin=floatArrayOf(0f,1f,0f),position=floatArrayOf(0f,0f,-1f)))
        val id = sent.last().getString("request_id")
        controller.result(JSONObject().put("id",id).put("saved",false).put("detail","Unavailable"))
        assertEquals(0,rendered.length())
        retry!!.invoke()
        assertEquals(id,sent.last().getString("request_id"))
    }
    @Test fun singleTapEditsAndDevicePosesNeverBecomeNoteTargets() {
        val sent = mutableListOf<JSONObject>(); var rendered = JSONArray()
        val controller = AnnotationController({sent.add(JSONObject(it.toString(Charsets.UTF_8))); true}) { _,_,_ -> }
        controller.renderNotes = { rendered=JSONArray(it) }
        val note = JSONObject().put("id","note").put("text","Before").put("position",JSONArray("[0,0,0]"))
        controller.scene("home",JSONArray().put(note).put(JSONObject().put("id","phone").put("phone_pose",JSONObject())))
        assertEquals(1,rendered.length())
        controller.pick(AnnotationPick("home",false,note=note))
        assertTrue(sent.isEmpty()); assertNotNull(controller.editor)
        controller.text="After"; controller.saveEditor()
        assertEquals("edit",sent.last().getJSONObject("action").getString("kind"))
        assertEquals("After",sent.last().getJSONObject("action").getString("text"))
    }
}
