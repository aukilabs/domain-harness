package com.aukilabs.annotator

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.*
import androidx.compose.material3.*
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.graphics.nativeCanvas
import androidx.compose.ui.unit.dp
import android.graphics.Paint

@Composable
internal fun AnnotationOverlay(controller: AnnotationController, tap: (Float,Float,Boolean) -> Unit) {
    Canvas(Modifier.fillMaxSize().pointerInput(controller) {
        detectTapGestures(onTap = { tap(it.x,it.y,false) },onDoubleTap = { tap(it.x,it.y,true) })
    }) {
        val paint = Paint(Paint.ANTI_ALIAS_FLAG).apply { textSize = 14.dp.toPx(); color = android.graphics.Color.WHITE
            setShadowLayer(3.dp.toPx(),0f,0f,android.graphics.Color.BLACK) }
        controller.markers.forEach {
            val color = if (it.pending) Color(0xFFFFD166) else Color(0xFF58DDFF)
            drawCircle(color,9.dp.toPx(),Offset(it.x,it.y),style = Stroke(3.dp.toPx()))
            drawContext.canvas.nativeCanvas.drawText(it.note.optString("text","Note").take(40),it.x+14.dp.toPx(),it.y+5.dp.toPx(),paint)
        }
    }
    if (controller.editor != null) AlertDialog(
        onDismissRequest = controller::dismissEditor,
        title = { Text("Edit note") },
        text = { OutlinedTextField(controller.text,{controller.text = it},label = { Text("Note") },singleLine = true) },
        confirmButton = { TextButton(onClick = controller::saveEditor) { Text("Save") } },
        dismissButton = { TextButton(onClick = controller::dismissEditor) { Text("Cancel") } },
    )
}
