package com.aukilabs.annotator

import android.opengl.EGL14.*
import android.opengl.GLES20.*
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import java.nio.ByteBuffer

class AnnotationRendererTest {
    private fun program(vertex: String, fragment: String): Int {
        fun shader(type: Int, source: String): Int {
            val shader = glCreateShader(type); glShaderSource(shader,source); glCompileShader(shader)
            val ok = IntArray(1); glGetShaderiv(shader,GL_COMPILE_STATUS,ok,0)
            check(ok[0] != 0) { glGetShaderInfoLog(shader) }; return shader
        }
        val v = shader(GL_VERTEX_SHADER,vertex); val f = shader(GL_FRAGMENT_SHADER,fragment)
        val result = glCreateProgram(); glAttachShader(result,v); glAttachShader(result,f); glLinkProgram(result)
        val ok = IntArray(1); glGetProgramiv(result,GL_LINK_STATUS,ok,0)
        check(ok[0] != 0) { glGetProgramInfoLog(result) }
        glDeleteShader(v); glDeleteShader(f); return result
    }

    @Test fun actualGlLabelsFollowEveryDrawWithoutReuploadingAndKeepPendingColors() {
        val display = eglGetDisplay(EGL_DEFAULT_DISPLAY)
        assertTrue(eglInitialize(display,IntArray(2),0,IntArray(2),0))
        val configs = arrayOfNulls<android.opengl.EGLConfig>(1)
        val count = IntArray(1)
        assertTrue(eglChooseConfig(display,intArrayOf(EGL_RENDERABLE_TYPE,EGL_OPENGL_ES2_BIT,
            EGL_SURFACE_TYPE,EGL_PBUFFER_BIT,EGL_RED_SIZE,8,EGL_GREEN_SIZE,8,EGL_BLUE_SIZE,8,EGL_ALPHA_SIZE,8,EGL_NONE),
            0,configs,0,1,count,0))
        assertTrue(count[0] > 0)
        val context = eglCreateContext(display,configs[0],EGL_NO_CONTEXT,intArrayOf(EGL_CONTEXT_CLIENT_VERSION,2,EGL_NONE),0)
        val surface = eglCreatePbufferSurface(display,configs[0],intArrayOf(EGL_WIDTH,256,EGL_HEIGHT,256,EGL_NONE),0)
        assertTrue(eglMakeCurrent(display,surface,surface,context))
        val renderer = AnnotationRenderer(1f)
        try {
            glViewport(0,0,256,256); glClearColor(0f,0f,0f,1f)
            renderer.surfaceCreated(::program)
            val note = JSONObject().put("id","test").put("text","Test")
            fun draw(x: Float, pending: Boolean = false) {
                glClear(GL_COLOR_BUFFER_BIT)
                renderer.draw(listOf(ScreenNote(note,x,80f,pending)),256,256)
                assertEquals(GL_NO_ERROR,glGetError())
            }
            fun pixel(x: Int,y: Int): IntArray {
                val buffer = ByteBuffer.allocateDirect(4)
                glReadPixels(x,255-y,1,1,GL_RGBA,GL_UNSIGNED_BYTE,buffer)
                return IntArray(3) { buffer.get(it).toInt() and 255 }
            }
            draw(64f)
            assertArrayEquals(intArrayOf(88,221,255),pixel(72,80))
            val pixels = ByteBuffer.allocateDirect(256*256*4)
            glReadPixels(0,0,256,256,GL_RGBA,GL_UNSIGNED_BYTE,pixels)
            assertTrue("Label text must be rendered, not just the ring",(0 until 256*256).any {
                (pixels.get(it*4).toInt() and 255)>230 && (pixels.get(it*4+1).toInt() and 255)>230 && (pixels.get(it*4+2).toInt() and 255)>230
            })
            draw(120f)
            assertArrayEquals(intArrayOf(0,0,0),pixel(72,80))
            assertArrayEquals(intArrayOf(88,221,255),pixel(128,80))
            assertEquals(1,renderer.textureUploads)
            draw(120f,true)
            assertArrayEquals(intArrayOf(255,209,102),pixel(128,80))
            assertEquals(2,renderer.textureUploads)
            note.put("text","Edited")
            draw(120f,true); assertEquals(3,renderer.textureUploads)
            repeat(70) { note.put("text","Label $it"); draw(120f) }
            assertEquals(64,renderer.cachedLabels)
            glClear(GL_COLOR_BUFFER_BIT); renderer.draw(emptyList(),256,256)
            assertArrayEquals(intArrayOf(0,0,0),pixel(128,80))
            renderer.release(); assertEquals(0,renderer.cachedLabels)
            renderer.surfaceCreated(::program); draw(120f)
            assertEquals(1,renderer.textureUploads)
            assertArrayEquals(intArrayOf(88,221,255),pixel(128,80))
        } finally {
            renderer.release()
            eglMakeCurrent(display,EGL_NO_SURFACE,EGL_NO_SURFACE,EGL_NO_CONTEXT)
            eglDestroySurface(display,surface); eglDestroyContext(display,context); eglTerminate(display)
        }
    }
}
