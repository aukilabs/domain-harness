package com.aukilabs.annotator

import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.*
import androidx.compose.material3.*
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.input.pointer.pointerInput

@Composable
internal fun AnnotationOverlay(controller: AnnotationController, tap: (Float,Float,Boolean) -> Unit) {
    // Touch/editor layer only. World labels are drawn with the camera on the GL thread.
    Box(Modifier.fillMaxSize().pointerInput(controller) {
        detectTapGestures(onTap = { tap(it.x,it.y,false) },onDoubleTap = { tap(it.x,it.y,true) })
    })
    if (controller.editor != null) AlertDialog(
        onDismissRequest = controller::dismissEditor,
        title = { Text("Edit note") },
        text = { OutlinedTextField(controller.text,{controller.text = it},label = { Text("Note") },singleLine = true) },
        confirmButton = { TextButton(onClick = controller::saveEditor) { Text("Save") } },
        dismissButton = { TextButton(onClick = controller::dismissEditor) { Text("Cancel") } },
    )
}
