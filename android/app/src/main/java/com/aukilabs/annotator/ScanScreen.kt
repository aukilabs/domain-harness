package com.aukilabs.annotator

import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.semantics.CustomAccessibilityAction
import androidx.compose.ui.semantics.customActions
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp

/** Camera never leaves composition or changes size when diagnostics are toggled. */
@Composable
internal fun ScanScreen(
    session: Any,
    modifier: Modifier = Modifier,
    camera: @Composable () -> Unit,
    feedback: @Composable () -> Unit,
    annotations: @Composable () -> Unit = {},
    messages: @Composable () -> Unit = {},
) {
    val view = LocalView.current
    DisposableEffect(view) {
        val previous = view.keepScreenOn
        view.keepScreenOn = true
        onDispose { view.keepScreenOn = previous }
    }
    var feedbackVisible by remember(session) { mutableStateOf(false) }
    Box(modifier.fillMaxSize().safeDrawingPadding()) {
        camera()
        annotations()
        if (feedbackVisible) {
            Surface(
                modifier = Modifier.align(Alignment.TopStart).fillMaxWidth()
                    .padding(end = 96.dp).heightIn(max = 320.dp),
                color = MaterialTheme.colorScheme.surface.copy(alpha = .88f),
            ) {
                Column(Modifier.verticalScroll(rememberScrollState()).padding(12.dp),
                    verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    feedback()
                }
            }
        }
        // Invisible, fixed target outside the text panel; single taps do nothing.
        // Keep this last so the camera/native view cannot intercept the gesture.
        Box(Modifier.align(Alignment.TopEnd).size(96.dp)
            .semantics {
                contentDescription = "Scan diagnostics"
                customActions = listOf(CustomAccessibilityAction(
                    if (feedbackVisible) "Hide feedback" else "Show feedback",
                ) { feedbackVisible = !feedbackVisible; true })
            }
            .pointerInput(session) {
                detectTapGestures(onDoubleTap = { feedbackVisible = !feedbackVisible })
            })
        Box(Modifier.align(Alignment.BottomCenter)) { messages() }
    }
}
