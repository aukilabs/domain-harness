package com.aukilabs.annotator

import android.app.Activity
import android.opengl.GLES11Ext
import android.opengl.GLES20.*
import android.opengl.GLSurfaceView
import android.opengl.Matrix
import com.google.ar.core.*
import com.google.ar.core.exceptions.NotYetAvailableException
import org.json.JSONArray
import org.json.JSONObject
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.nio.FloatBuffer
import java.util.UUID
import javax.microedition.khronos.egl.EGLConfig
import javax.microedition.khronos.opengles.GL10
import kotlin.math.ceil
import kotlin.math.sqrt

/** Camera and metric point overlay. Only depth hits, never inferred free space. */
class ArScanner(private val activity: Activity, private val session: Session, private val status: (String) -> Unit) :
    GLSurfaceView(activity), GLSurfaceView.Renderer {
    private val voxels = LocalVoxels()
    private val depthFreshness = DepthFreshness()
    private var mapId = UUID.randomUUID().toString()
    private var lastCapture = 0L
    private var lastPublish = 0L
    private var tracking = false
    private var texture = 0
    private var background = 0
    private var pointsProgram = 0
    private var widthPx = 1
    private var heightPx = 1
    private var points = floats(floatArrayOf())
    private val quad = floats(floatArrayOf(-1f,-1f, 1f,-1f, -1f,1f, 1f,1f))
    private val uv = floats(FloatArray(8))
    private var lastStatus = ""
    private var depthStatus = "Waiting for the first depth image"
    init { setEGLContextClientVersion(2); preserveEGLContextOnPause = true; setRenderer(this); renderMode = RENDERMODE_CONTINUOUSLY }
    private fun report(s: String) { if (s != lastStatus) { lastStatus = s; activity.runOnUiThread { status(s) } } }
    fun shutdown() { onPause(); session.pause(); session.close(); NativeBridge.localMap(null) }
    override fun onSurfaceCreated(gl: GL10?, config: EGLConfig?) {
        val ids = IntArray(1); glGenTextures(1, ids, 0); texture = ids[0]
        glBindTexture(GLES11Ext.GL_TEXTURE_EXTERNAL_OES, texture)
        glTexParameteri(GLES11Ext.GL_TEXTURE_EXTERNAL_OES, GL_TEXTURE_MIN_FILTER, GL_LINEAR)
        glTexParameteri(GLES11Ext.GL_TEXTURE_EXTERNAL_OES, GL_TEXTURE_MAG_FILTER, GL_LINEAR)
        glTexParameteri(GLES11Ext.GL_TEXTURE_EXTERNAL_OES, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE)
        glTexParameteri(GLES11Ext.GL_TEXTURE_EXTERNAL_OES, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE)
        background = program("attribute vec2 p; attribute vec2 uv; varying vec2 t; void main(){gl_Position=vec4(p,0.,1.);t=uv;}",
            "#extension GL_OES_EGL_image_external : require\nprecision mediump float; uniform samplerExternalOES camera; varying vec2 t; void main(){gl_FragColor=texture2D(camera,t);}")
        pointsProgram = program("attribute vec3 p; uniform mat4 vp; void main(){gl_Position=vp*vec4(p,1.);gl_PointSize=6.;}",
            "precision mediump float; void main(){gl_FragColor=vec4(.15,1.,.65,1.);}")
        session.setCameraTextureName(texture)
    }
    override fun onSurfaceChanged(gl: GL10?, w: Int, h: Int) { widthPx = w; heightPx = h; glViewport(0,0,w,h) }
    override fun onDrawFrame(gl: GL10?) {
        try {
            @Suppress("DEPRECATION") val rotation = activity.windowManager.defaultDisplay.rotation
            session.setDisplayGeometry(rotation, widthPx, heightPx)
            val frame = session.update()
            glClear(GL_COLOR_BUFFER_BIT or GL_DEPTH_BUFFER_BIT)
            if (frame.timestamp == 0L) return
            frame.transformCoordinates2d(Coordinates2d.OPENGL_NORMALIZED_DEVICE_COORDINATES, quad, Coordinates2d.TEXTURE_NORMALIZED, uv)
            glDisable(GL_DEPTH_TEST); glUseProgram(background)
            attribute(background, "p", quad, 2); attribute(background, "uv", uv, 2)
            glActiveTexture(GL_TEXTURE0); glBindTexture(GLES11Ext.GL_TEXTURE_EXTERNAL_OES, texture)
            glUniform1i(glGetUniformLocation(background,"camera"),0); glDrawArrays(GL_TRIANGLE_STRIP,0,4)
            val camera = frame.camera
            if (camera.trackingState != TrackingState.TRACKING) {
                if (tracking) { voxels.clear(); depthFreshness.reset(); points = floats(floatArrayOf()); mapId = UUID.randomUUID().toString(); NativeBridge.localMap(null) }
                tracking = false; report("Tracking paused · ${camera.trackingFailureReason}. Move slowly toward textured surfaces."); return
            }
            tracking = true
            if (frame.timestamp - lastCapture >= 500_000_000L) capture(frame)
            if (frame.timestamp - lastPublish >= 1_000_000_000L) {
                lastPublish = frame.timestamp
                val cells = JSONArray(); for (c in voxels.cells) cells.put(JSONArray(listOf(c.x,c.y,c.z)))
                val accepted = NativeBridge.localMap(JSONObject().put("session",mapId).put("timestamp_ns",frame.timestamp).put("cells",cells).toString().toByteArray())
                report("${voxels.cells.size} voxels · 10 cm · unaligned\n$depthStatus\n" + if (voxels.cells.size >= 10000) "Map full" else if (!accepted) "Local only; connect to Home to share" else "Sharing queued; see Home acknowledgement above")
            }
            val view = FloatArray(16); val projection = FloatArray(16); val vp = FloatArray(16)
            camera.getViewMatrix(view,0); camera.getProjectionMatrix(projection,0,.1f,100f); Matrix.multiplyMM(vp,0,projection,0,view,0)
            glUseProgram(pointsProgram); attribute(pointsProgram,"p",points,3)
            glUniformMatrix4fv(glGetUniformLocation(pointsProgram,"vp"),1,false,vp,0)
            glDrawArrays(GL_POINTS,0,voxels.cells.size)
        } catch (e: Exception) { report("AR paused: ${e.javaClass.simpleName} · ${e.message ?: ""}"); NativeBridge.localMap(null) }
    }
    private fun capture(frame: Frame) {
        try {
            frame.acquireRawDepthImage16Bits().use { depth ->
                frame.acquireRawDepthConfidenceImage().use { confidence ->
                    // ARCore reprojects depth into this frame's camera view. Its
                    // timestamp identifies the underlying measurement and need
                    // not equal frame.timestamp. Integrate each measurement once.
                    if (!depthFreshness.accept(depth.timestamp)) { depthStatus = "Waiting for the next depth measurement · previous voxels retained"; return }
                    val dp = depth.planes[0]; val cp = confidence.planes[0]
                    val db = dp.buffer.order(ByteOrder.LITTLE_ENDIAN); val cb = cp.buffer
                    val intrinsics = frame.camera.textureIntrinsics
                    val dims = intrinsics.imageDimensions; val focal = intrinsics.focalLength; val principal = intrinsics.principalPoint
                    val sx = depth.width.toFloat()/dims[0]; val sy = depth.height.toFloat()/dims[1]
                    val fx = focal[0]*sx; val fy = focal[1]*sy; val cx = principal[0]*sx; val cy = principal[1]*sy
                    val step = ceil(sqrt(depth.width*depth.height/2048.0)).toInt().coerceAtLeast(1)
                    var confident = 0
                    var hits = 0
                    var maxConfidence = 0
                    for (y in 0 until depth.height step step) for (x in 0 until depth.width step step) {
                        val certainty = cb.get(y*cp.rowStride+x*cp.pixelStride).toInt() and 255
                        maxConfidence = maxOf(maxConfidence, certainty)
                        if (certainty < 128) continue
                        confident++
                        val z = (db.getShort(y*dp.rowStride+x*dp.pixelStride).toInt() and 65535)*.001f
                        if (z < .2f || z > 5f) continue
                        val point = floatArrayOf((x-cx)*z/fx, -(y-cy)*z/fy, -z)
                        voxels.add(frame.camera.pose.transformPoint(point))
                        hits++
                    }
                    depthStatus = "Depth ${depth.width}×${depth.height}: $hits hits · $confident confident · max confidence $maxConfidence/255"
                    lastCapture = frame.timestamp; points = floats(voxels.centers())
                }
            }
        } catch (_: NotYetAvailableException) { depthStatus = "ARCore has no depth image yet. Move sideways toward textured surfaces in good light." }
    }
    private fun floats(a: FloatArray): FloatBuffer = ByteBuffer.allocateDirect(a.size*4).order(ByteOrder.nativeOrder()).asFloatBuffer().apply { put(a); position(0) }
    private fun attribute(p: Int, name: String, data: FloatBuffer, size: Int) {
        val location = glGetAttribLocation(p,name); data.position(0); glEnableVertexAttribArray(location); glVertexAttribPointer(location,size,GL_FLOAT,false,0,data)
    }
    private fun program(vertex: String, fragment: String): Int {
        fun shader(type: Int, source: String): Int { val s = glCreateShader(type); glShaderSource(s,source); glCompileShader(s); val ok = IntArray(1); glGetShaderiv(s,GL_COMPILE_STATUS,ok,0); check(ok[0] != 0) {glGetShaderInfoLog(s)}; return s }
        val v = shader(GL_VERTEX_SHADER,vertex); val f = shader(GL_FRAGMENT_SHADER,fragment); val p = glCreateProgram(); glAttachShader(p,v); glAttachShader(p,f); glLinkProgram(p)
        val ok = IntArray(1); glGetProgramiv(p,GL_LINK_STATUS,ok,0); check(ok[0] != 0) {glGetProgramInfoLog(p)}; glDeleteShader(v); glDeleteShader(f); return p
    }
}
