package com.aukilabs.annotator

import android.app.Activity
import android.opengl.GLES11Ext
import android.opengl.GLES20.*
import android.opengl.GLSurfaceView
import android.opengl.Matrix
import com.google.ar.core.*
import com.google.ar.core.exceptions.NotYetAvailableException
import android.os.Process
import android.util.Log
import org.json.JSONObject
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.nio.FloatBuffer
import java.util.UUID
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicReference
import javax.microedition.khronos.egl.EGLConfig
import javax.microedition.khronos.opengles.GL10

/** Camera and metric point overlay. Only depth hits, never inferred free space. */
class ArScanner(private val activity: Activity, private val session: Session, private val status: (String) -> Unit) :
    GLSurfaceView(activity), GLSurfaceView.Renderer {
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
    private val points = floats(FloatArray(10000 * 3))
    private var pointCount = 0
    private var displayedCenters: FloatArray? = null
    private val view = FloatArray(16)
    private val projection = FloatArray(16)
    private val vp = FloatArray(16)
    private val quad = floats(floatArrayOf(-1f,-1f, 1f,-1f, -1f,1f, 1f,1f))
    private val uv = floats(FloatArray(8))
    private var lastStatus = ""
    private var depthStatus = "Waiting for the first depth image"
    private var localStatus = "Local snapshot pending"
    private var lastReport = 0L
    private val metrics = ScanMetrics()
    private var performance = "Measuring AR frame rate…"
    private val mapper = ScanMapper()
    private val cameraPublisher = CameraPublisher()
    internal val annotations = AnnotationPicker(resources.displayMetrics.density)
    internal var annotationPick: (AnnotationPick) -> Unit = {}
    internal var annotationProjection: (List<ScreenNote>) -> Unit = {}
    private val mapping = Executors.newSingleThreadExecutor { task -> Thread(task, "annotator-mapper") }
    private val work = ScanWorkGate()
    private val poseWork = ScanWorkGate()
    private val poseWorker = Executors.newSingleThreadExecutor { task -> Thread(task, "annotator-pose") }
    private val poseCadence = PoseCadence()
    private val mapped = AtomicReference<Pair<ScanOutput, Boolean>?>(null)
    private var depthBytes = ByteArray(0)
    private var confidenceBytes = ByteArray(0)
    private var lumaBytes = ByteArray(0)
    @Volatile var homeReference: String? = null
    @Volatile private var closed = false
    private val calibration = CalibrationState()
    private val detector = Executors.newSingleThreadExecutor { task -> Thread(task, "annotator-portals") }
    private var detecting = false
    private var lastDetection = 0L
    private var latestFrameTime = 0L
    private var alignment: JSONObject? = null
    private var calibrationStatus = "Connect to Home to load Portal references"
    init { setEGLContextClientVersion(2); preserveEGLContextOnPause = true; setRenderer(this); renderMode = RENDERMODE_CONTINUOUSLY }
    private fun report(s: String) { if (s != lastStatus) { lastStatus = s; activity.runOnUiThread { status(s) } } }
    fun shutdown() {
        closed = true
        cameraPublisher.close()
        poseWork.close { NativeBridge.pose(null) }
        work.close { mapped.set(null); NativeBridge.localMap(null); NativeBridge.depth(null); NativeBridge.pose(null) }
        // No worker retains an ARCore Image/Frame/Session. In-flight work may finish
        // but cannot publish after close, and no rendering waits for it to finish.
        detector.shutdown(); mapping.shutdown(); poseWorker.shutdown(); onPause(); session.pause(); session.close()
    }
    private fun clearAlignment() {
        alignment = null; calibration.reset()
        poseWork.invalidate { NativeBridge.pose(null) }; poseCadence.reset()
        work.invalidate { mapped.set(null); NativeBridge.depth(null); NativeBridge.pose(null) }
        calibrationStatus = if (calibration.reference == null) "Connect to Home to load Portal references" else "Show a Portal from the selected Home scene"
    }
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
        val started = System.nanoTime()
        var updateNs = 0L
        var cameraTimestamp = 0L
        try {
            if (closed) return
            if (calibration.updateReference(homeReference)) clearAlignment()
            @Suppress("DEPRECATION") val rotation = activity.windowManager.defaultDisplay.rotation
            session.setDisplayGeometry(rotation, widthPx, heightPx)
            val updateStart = System.nanoTime()
            val frame = session.update()
            updateNs = System.nanoTime() - updateStart
            cameraTimestamp = frame.timestamp
            glClear(GL_COLOR_BUFFER_BIT or GL_DEPTH_BUFFER_BIT)
            if (frame.timestamp == 0L) return
            latestFrameTime = frame.timestamp
            frame.transformCoordinates2d(Coordinates2d.OPENGL_NORMALIZED_DEVICE_COORDINATES, quad, Coordinates2d.TEXTURE_NORMALIZED, uv)
            glDisable(GL_DEPTH_TEST); glUseProgram(background)
            attribute(background, "p", quad, 2); attribute(background, "uv", uv, 2)
            glActiveTexture(GL_TEXTURE0); glBindTexture(GLES11Ext.GL_TEXTURE_EXTERNAL_OES, texture)
            glUniform1i(glGetUniformLocation(background,"camera"),0); glDrawArrays(GL_TRIANGLE_STRIP,0,4)
            val camera = frame.camera
            // Camera observation does not require tracking or Portal calibration.
            cameraPublisher.capture(frame)
            if (camera.trackingState != TrackingState.TRACKING) {
                annotations.paused({ p -> activity.runOnUiThread { annotationPick(p) } }, { activity.runOnUiThread { annotationProjection(emptyList()) } })
                if (tracking) { clearAlignment(); depthFreshness.reset(); pointCount = 0; displayedCenters = null; mapId = UUID.randomUUID().toString(); NativeBridge.localMap(null) }
                tracking = false; report("Tracking paused · ${camera.trackingFailureReason}. Move slowly toward textured surfaces."); return
            }
            tracking = true
            detectPortal(frame)
            schedulePose(frame)
            if (frame.timestamp - lastCapture >= 500_000_000L) scheduleMapping(frame)
            mapped.getAndSet(null)?.let { (result, accepted) ->
                if (displayedCenters !== result.centers) {
                    points.position(0); points.put(result.centers); points.position(0)
                    displayedCenters = result.centers
                }
                pointCount = result.count; depthStatus = result.depthStatus
                if (result.local != null) localStatus = if (accepted) "Local snapshot queued" else "Local only; connect to Home to share"
            }
            if (frame.timestamp - lastReport >= 1_000_000_000L) {
                lastReport = frame.timestamp
                report("$calibrationStatus\n$pointCount local voxels · 10 cm\n$depthStatus\n" +
                    (if (pointCount >= 10000) "Local map full" else localStatus) + "\n$performance")
            }
            camera.getViewMatrix(view,0); camera.getProjectionMatrix(projection,0,.1f,100f); Matrix.multiplyMM(vp,0,projection,0,view,0)
            annotations.frame(frame,alignment,vp,widthPx,heightPx,
                { p -> activity.runOnUiThread { if (!closed) annotationPick(p) } },
                { markers -> activity.runOnUiThread { if (!closed) annotationProjection(markers) } })
            glUseProgram(pointsProgram); attribute(pointsProgram,"p",points,3)
            glUniformMatrix4fv(glGetUniformLocation(pointsProgram,"vp"),1,false,vp,0)
            glDrawArrays(GL_POINTS,0,pointCount)
        } catch (e: Exception) { clearAlignment(); report("AR paused: ${e.javaClass.simpleName} · ${e.message ?: ""}"); NativeBridge.localMap(null) }
        finally {
            if (!closed) metrics.record(started,System.nanoTime(),updateNs,cameraTimestamp)?.let {
                performance = it; Log.i("AnnotatorPerf",it)
            }
        }
    }
    private fun detectPortal(frame: Frame) {
        if (detecting || !calibration.canDetect(frame.timestamp) || frame.timestamp-lastDetection < 500_000_000L) return
        lastDetection = frame.timestamp
        val reference = calibration.reference ?: return
        try {
            frame.acquireCameraImage().use { image ->
                // acquireCameraImage guarantees this Frame's image. Android Image
                // and ARCore timestamps need not share a defined time base.
                if (image.width > 1920 || image.height > 1920) {
                    calibrationStatus = "Camera image exceeds Portal scanner bounds"; return
                }
                val plane = image.planes[0]
                if (lumaBytes.size != image.width*image.height) lumaBytes = ByteArray(image.width*image.height)
                val pixels = lumaBytes
                copyLuma(plane.buffer,image.width,image.height,plane.rowStride,plane.pixelStride,pixels)
                // CPU image coordinates, not display/texture coordinates. Copy all
                // capture-time values before closing Image and leaving this frame.
                val k = frame.camera.imageIntrinsics
                val focal = k.focalLength; val principal = k.principalPoint
                val dimensions = k.imageDimensions
                if (dimensions[0] != image.width || dimensions[1] != image.height) {
                    calibrationStatus = "Camera intrinsics do not match image dimensions"; return
                }
                val epoch = calibration.generation; val timestamp = frame.timestamp
                val sessionId = mapId; val pose = frame.camera.pose
                val imageWidth = image.width; val imageHeight = image.height
                detecting = true
                detector.execute {
                    Process.setThreadPriority(Process.THREAD_PRIORITY_BACKGROUND)
                    val reply = runCatching {
                        val capture = JSONObject().put("session",sessionId).put("timestamp_ns",timestamp)
                            .put("width",imageWidth).put("height",imageHeight)
                            .put("fx",focal[0].toDouble()).put("fy",focal[1].toDouble())
                            .put("cx",principal[0].toDouble()).put("cy",principal[1].toDouble())
                            .put("camera_to_session",poseJson(pose,"phone-camera:$sessionId","phone-ar:$sessionId"))
                            .put("scenegraph",JSONObject(reference)).toString().toByteArray()
                        NativeBridge.calibrate(pixels,capture)?.let { JSONObject(it.toString(Charsets.UTF_8)) }
                    }.getOrNull()
                    if (!closed) queueEvent {
                        detecting = false
                        if (!closed && tracking && epoch == calibration.generation && homeReference == reference) {
                            val found = reply?.optJSONObject("alignment")
                            if (found != null && calibration.accept(epoch,timestamp,latestFrameTime)) {
                                alignment = found
                                calibrationStatus = reply.optString("detail") + " · sharing depth to Home"
                            } else if (alignment == null) calibrationStatus = reply?.optString("detail") ?: "Portal scan failed; try again"
                        }
                    }
                }
            }
        } catch (_: NotYetAvailableException) { if (alignment == null) calibrationStatus = "Waiting for a camera image to scan Portals" }
    }
    private fun scheduleMapping(frame: Frame) {
        val token = work.acquire() ?: return
        val capturedAt = System.nanoTime()
        try {
            lastCapture = frame.timestamp
            val depth = copyDepth(frame)
            val publish = frame.timestamp - lastPublish >= 1_000_000_000L
            if (publish) lastPublish = frame.timestamp
            val input = ScanInput(mapId,frame.timestamp,frame.camera.pose,alignment,depth,depthStatus,publish)
            mapping.execute {
                try {
                    Process.setThreadPriority(Process.THREAD_PRIORITY_BACKGROUND)
                    val result = mapper.process(input)
                    // A slow frame is never published as a fresh observation. Epoch
                    // check and sends are atomic with tracking reset/shutdown.
                    work.commit(token) {
                        if (System.nanoTime()-capturedAt < 2_000_000_000L) {
                            val accepted = result.local?.let { NativeBridge.localMap(it) } ?: false
                            result.depth?.let { NativeBridge.depth(it) }
                            mapped.set(result to accepted)
                        }
                    }
                } catch (e: Exception) {
                    // Never log observation payloads or connection credentials.
                    Log.w("AnnotatorPerf","Mapping failed: ${e.javaClass.simpleName}")
                } finally { work.finish() }
            }
        } catch (e: Exception) { work.finish(); throw e }
    }
    private fun schedulePose(frame: Frame) {
        val aligned = alignment ?: return
        if (!poseCadence.due(frame.timestamp)) return
        val token = poseWork.acquire() ?: return
        val timestamp = frame.timestamp; val pose = frame.camera.pose; val sessionId = mapId
        val captured = System.nanoTime()
        poseCadence.sent(timestamp)
        try {
            poseWorker.execute {
                try {
                    val encoded = encodePhonePose(sessionId,timestamp,pose,aligned)
                    poseWork.commit(token) {
                        if (System.nanoTime()-captured < 500_000_000L) NativeBridge.pose(encoded)
                    }
                } finally { poseWork.finish() }
            }
        } catch (e: Exception) { poseWork.finish(); throw e }
    }
    private fun copyDepth(frame: Frame): DepthCopy? {
        try {
            frame.acquireRawDepthImage16Bits().use { depth ->
                frame.acquireRawDepthConfidenceImage().use { confidence ->
                    // Images close before dispatch; pose/intrinsics are from this
                    // same Frame, and repeated raw measurements are not integrated.
                    if (!depthFreshness.accept(depth.timestamp)) {
                        depthStatus = "Waiting for the next depth measurement · previous voxels retained"; return null
                    }
                    check(depth.width == confidence.width && depth.height == confidence.height)
                    val dp = depth.planes[0]; val cp = confidence.planes[0]
                    val db = dp.buffer.duplicate(); val cb = cp.buffer.duplicate()
                    check(db.remaining() <= 2*1024*1024 && cb.remaining() <= 1024*1024)
                    if (depthBytes.size != db.remaining()) depthBytes = ByteArray(db.remaining())
                    if (confidenceBytes.size != cb.remaining()) confidenceBytes = ByteArray(cb.remaining())
                    db.get(depthBytes); cb.get(confidenceBytes)
                    val k = frame.camera.textureIntrinsics
                    val dims = k.imageDimensions; val focal = k.focalLength; val principal = k.principalPoint
                    val sx = depth.width.toFloat()/dims[0]; val sy = depth.height.toFloat()/dims[1]
                    return DepthCopy(depth.width,depth.height,depthBytes,dp.rowStride,dp.pixelStride,
                        confidenceBytes,cp.rowStride,cp.pixelStride,focal[0]*sx,focal[1]*sy,principal[0]*sx,principal[1]*sy)
                }
            }
        } catch (_: NotYetAvailableException) { depthStatus = "ARCore has no depth image yet. Move sideways toward textured surfaces in good light." }
        return null
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
