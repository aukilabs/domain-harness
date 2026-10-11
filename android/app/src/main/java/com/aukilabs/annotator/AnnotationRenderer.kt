package com.aukilabs.annotator

import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Paint
import android.opengl.GLES20.*
import android.opengl.GLUtils
import java.nio.ByteBuffer
import java.nio.ByteOrder
import kotlin.math.ceil

/** GL-thread-owned screen-facing labels, composited in the same frame as AR's
 * camera and geometry. Only content changes rasterize/upload text, never motion. */
internal class AnnotationRenderer(private val density: Float) {
    private data class Label(val text: String, val pending: Boolean)
    private data class Sprite(val texture: Int, val width: Int, val height: Int)
    private val cache = LinkedHashMap<Label,Sprite>(64,.75f,true)
    private val quad = ByteBuffer.allocateDirect(8*4).order(ByteOrder.nativeOrder()).asFloatBuffer()
        .apply { put(floatArrayOf(0f,0f,1f,0f,0f,1f,1f,1f)); position(0) }
    private var program = 0
    private var position = 0
    private var viewport = 0
    private var origin = 0
    private var size = 0
    private var sampler = 0
    private var maxTextureSize = 2048
    internal var textureUploads = 0; private set
    internal val cachedLabels get() = cache.size

    fun surfaceCreated(makeProgram: (String,String) -> Int) {
        // GL context recreation invalidates old names; do not delete names in the new context.
        cache.clear(); textureUploads = 0
        program = makeProgram(
            "attribute vec2 p; uniform vec2 viewport; uniform vec2 origin; uniform vec2 size; varying vec2 uv; " +
                "void main(){vec2 pixel=origin+p*size; gl_Position=vec4(pixel.x/viewport.x*2.-1.,1.-pixel.y/viewport.y*2.,0.,1.); uv=p;}",
            "precision mediump float; uniform sampler2D label; varying vec2 uv; void main(){gl_FragColor=texture2D(label,uv);}")
        position = glGetAttribLocation(program,"p")
        viewport = glGetUniformLocation(program,"viewport")
        origin = glGetUniformLocation(program,"origin")
        size = glGetUniformLocation(program,"size")
        sampler = glGetUniformLocation(program,"label")
        val bound = IntArray(1); glGetIntegerv(GL_MAX_TEXTURE_SIZE,bound,0); maxTextureSize = bound[0]
    }

    fun draw(markers: List<ScreenNote>, width: Int, height: Int) {
        if (program == 0 || markers.isEmpty() || width <= 0 || height <= 0) return
        glUseProgram(program); glBindBuffer(GL_ARRAY_BUFFER,0)
        glDisable(GL_DEPTH_TEST); glEnable(GL_BLEND)
        // Android bitmap pixels are premultiplied, including anti-aliased edges/shadows.
        glBlendFunc(GL_ONE,GL_ONE_MINUS_SRC_ALPHA)
        glActiveTexture(GL_TEXTURE0); glUniform1i(sampler,0)
        glUniform2f(viewport,width.toFloat(),height.toFloat())
        quad.position(0); glEnableVertexAttribArray(position)
        glVertexAttribPointer(position,2,GL_FLOAT,false,0,quad)
        for (marker in markers) {
            val label = Label(marker.note.optString("text","Note").take(40),marker.pending)
            val sprite = cache[label] ?: upload(label).also { cache[label] = it }
            if (cache.size > 64) {
                val oldest = cache.entries.iterator(); val evicted = oldest.next().value
                glDeleteTextures(1,intArrayOf(evicted.texture),0); oldest.remove()
            }
            glBindTexture(GL_TEXTURE_2D,sprite.texture)
            glUniform2f(origin,marker.x-13*density,marker.y-18*density)
            glUniform2f(size,sprite.width.toFloat(),sprite.height.toFloat())
            glDrawArrays(GL_TRIANGLE_STRIP,0,4)
        }
        glDisableVertexAttribArray(position); glBindTexture(GL_TEXTURE_2D,0); glDisable(GL_BLEND)
    }

    private fun upload(label: Label): Sprite {
        val text = Paint(Paint.ANTI_ALIAS_FLAG).apply {
            textSize = 14*density; color = Color.WHITE
            setShadowLayer(3*density,0f,0f,Color.BLACK)
        }
        val width = ceil(31*density+text.measureText(label.text)).toInt().coerceIn(1,maxTextureSize)
        val height = ceil(36*density).toInt().coerceIn(1,maxTextureSize)
        val bitmap = Bitmap.createBitmap(width,height,Bitmap.Config.ARGB_8888)
        val ids = IntArray(1)
        try {
            val canvas = Canvas(bitmap)
            val ring = Paint(Paint.ANTI_ALIAS_FLAG).apply {
                color = if (label.pending) Color.rgb(255,209,102) else Color.rgb(88,221,255)
                style = Paint.Style.STROKE; strokeWidth = 3*density
            }
            canvas.drawCircle(13*density,18*density,9*density,ring)
            canvas.drawText(label.text,27*density,23*density,text)
            glGenTextures(1,ids,0); glBindTexture(GL_TEXTURE_2D,ids[0])
            glTexParameteri(GL_TEXTURE_2D,GL_TEXTURE_MIN_FILTER,GL_LINEAR)
            glTexParameteri(GL_TEXTURE_2D,GL_TEXTURE_MAG_FILTER,GL_LINEAR)
            glTexParameteri(GL_TEXTURE_2D,GL_TEXTURE_WRAP_S,GL_CLAMP_TO_EDGE)
            glTexParameteri(GL_TEXTURE_2D,GL_TEXTURE_WRAP_T,GL_CLAMP_TO_EDGE)
            GLUtils.texImage2D(GL_TEXTURE_2D,0,bitmap,0)
            textureUploads++
        } finally { bitmap.recycle() }
        return Sprite(ids[0],width,height)
    }

    fun release() {
        cache.values.forEach { glDeleteTextures(1,intArrayOf(it.texture),0) }; cache.clear()
        if (program != 0) glDeleteProgram(program)
        program = 0
    }
}
