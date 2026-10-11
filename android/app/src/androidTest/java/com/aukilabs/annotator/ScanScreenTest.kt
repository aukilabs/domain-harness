package com.aukilabs.annotator

import android.os.SystemClock
import android.view.MotionEvent
import android.view.View
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.mutableStateOf
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.layout.boundsInWindow
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.platform.LocalView
import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicInteger
import java.util.concurrent.atomic.AtomicReference
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class ScanScreenTest {
    @Test fun screenStaysAwakeOnlyWhileScanningAndRestoresPreviousSetting() {
        val scanning = mutableStateOf(false)
        val view = AtomicReference<View?>(null)
        ActivityScenario.launch(MainActivity::class.java).use { scenario ->
            scenario.onActivity { activity ->
                activity.setContent {
                    view.set(LocalView.current)
                    MaterialTheme {
                        if (scanning.value) ScanScreen(session = 1,
                            camera = { Box(Modifier.fillMaxSize()) }, feedback = { Text("Feedback") })
                        else Text("Not scanning")
                    }
                }
            }
            fun awaitAwake(expected: Boolean) {
                val deadline = SystemClock.uptimeMillis()+3000
                var matched = false
                while (!matched && SystemClock.uptimeMillis()<deadline) {
                    scenario.onActivity { matched = view.get()?.keepScreenOn == expected }
                    if (!matched) Thread.sleep(20)
                }
                assertTrue("Expected keepScreenOn=$expected",matched)
            }
            awaitAwake(false)
            scenario.onActivity { scanning.value = true }
            awaitAwake(true) // Feedback is still hidden by default.
            scenario.onActivity { scanning.value = false }
            awaitAwake(false)
            // Do not clear a pre-existing request owned by another UI feature.
            scenario.onActivity { view.get()!!.keepScreenOn = true; scanning.value = true }
            InstrumentationRegistry.getInstrumentation().waitForIdleSync()
            scenario.onActivity { scanning.value = false }
            InstrumentationRegistry.getInstrumentation().waitForIdleSync()
            awaitAwake(true)
            scenario.onActivity { view.get()!!.keepScreenOn = false }
        }
    }
    @Test fun cornerDoubleTapTogglesFeedbackWithoutRemountingCamera() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val visible = AtomicBoolean(false)
        val mounts = AtomicInteger(0)
        val noteTaps = AtomicInteger(0)
        val cameraBounds = AtomicReference<Rect?>(null)
        val session = mutableStateOf(1)
        ActivityScenario.launch(MainActivity::class.java).use { scenario ->
            scenario.onActivity { activity ->
                // UI-only camera stand-in: no AR session, native peer, or fake observations.
                activity.setContent {
                    MaterialTheme {
                        ScanScreen(session = session.value,
                            annotations = { Box(Modifier.fillMaxSize().pointerInput(Unit) {
                                detectTapGestures(onDoubleTap = { noteTaps.incrementAndGet() })
                            }) },
                            camera = {
                                DisposableEffect(Unit) { mounts.incrementAndGet(); onDispose {} }
                                Box(Modifier.fillMaxSize().onGloballyPositioned { cameraBounds.set(it.boundsInWindow()) })
                            },
                            feedback = {
                                DisposableEffect(Unit) { visible.set(true); onDispose { visible.set(false) } }
                                Text("Scan feedback")
                            })
                    }
                }
            }
            fun await(condition: () -> Boolean) {
                val deadline = SystemClock.uptimeMillis()+3000
                while (!condition() && SystemClock.uptimeMillis()<deadline) Thread.sleep(20)
                assertTrue("UI did not reach expected state",condition())
            }
            await { cameraBounds.get() != null && mounts.get() == 1 }
            val initialBounds = cameraBounds.get()!!
            val density = instrumentation.targetContext.resources.displayMetrics.density
            fun tap(x: Float, y: Float) {
                val down = SystemClock.uptimeMillis()
                val press = MotionEvent.obtain(down,down,MotionEvent.ACTION_DOWN,x,y,0)
                val release = MotionEvent.obtain(down,down+35,MotionEvent.ACTION_UP,x,y,0)
                try {
                    scenario.onActivity { it.window.decorView.dispatchTouchEvent(press) }
                    Thread.sleep(35)
                    scenario.onActivity { it.window.decorView.dispatchTouchEvent(release) }
                } finally { press.recycle(); release.recycle() }
            }
            val x = initialBounds.right-40*density
            val y = initialBounds.top+40*density
            fun doubleTap(tx: Float = x, ty: Float = y) { tap(tx,ty); Thread.sleep(80); tap(tx,ty) }
            assertFalse(visible.get())
            tap(x,y); Thread.sleep(400)
            assertFalse("Single tap must not reveal feedback",visible.get())
            doubleTap(initialBounds.center.x,initialBounds.center.y); Thread.sleep(400)
            assertFalse("Other parts of the camera must not toggle feedback",visible.get())
            assertEquals(1,noteTaps.get())
            doubleTap(); await { visible.get() }
            assertEquals("Diagnostics must consume the gesture",1,noteTaps.get())
            assertEquals(initialBounds,cameraBounds.get())
            assertEquals(1,mounts.get())
            Thread.sleep(400)
            doubleTap(); await { !visible.get() }
            assertEquals(initialBounds,cameraBounds.get())
            assertEquals(1,mounts.get())
            Thread.sleep(400)
            doubleTap(); await { visible.get() }
            scenario.onActivity { session.value = 2 }
            await { !visible.get() }
        }
    }
}
