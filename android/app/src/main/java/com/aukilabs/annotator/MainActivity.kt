package com.aukilabs.annotator

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.lifecycleScope
import androidx.lifecycle.repeatOnLifecycle
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.json.JSONObject
import java.io.File
import android.Manifest
import android.content.pm.PackageManager
import androidx.compose.ui.viewinterop.AndroidView
import androidx.activity.compose.BackHandler
import com.google.ar.core.ArCoreApk
import com.google.ar.core.Session
import com.google.ar.core.Config

class MainActivity : ComponentActivity() {
    private var profile by mutableStateOf("")
    private var status by mutableStateOf("Import Home's connection profile to begin.")
    private var active by mutableStateOf(false)
    private var sceneSummary by mutableStateOf("")
    private var noteTitles by mutableStateOf(emptyList<String>())
    private var scanSession by mutableStateOf<Session?>(null)
    private var scanner: ArScanner? = null
    private var scanStatus by mutableStateOf("Move slowly to capture depth")
    private var sharingStatus by mutableStateOf("")
    private val cameraPermission = registerForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        if (granted) startScanning() else { status = "Camera permission is required for local mapping." }
    }
    private fun startScanning() {
        if (scanSession != null) return
        if (checkSelfPermission(Manifest.permission.CAMERA) != PackageManager.PERMISSION_GRANTED) {
            cameraPermission.launch(Manifest.permission.CAMERA); return
        }
        var candidate: Session? = null
        try {
            if (ArCoreApk.getInstance().requestInstall(this, true) == ArCoreApk.InstallStatus.INSTALL_REQUESTED) {
                status = "Finish installing Google Play Services for AR, then tap Start local scan again."; return
            }
            val s = Session(this); candidate = s
            check(s.isDepthModeSupported(Config.DepthMode.AUTOMATIC)) { "This device does not support ARCore Depth." }
            s.configure(s.config.apply { depthMode = Config.DepthMode.AUTOMATIC; focusMode = Config.FocusMode.AUTO })
            s.resume(); scanSession = s
        } catch (e: Exception) { candidate?.close(); status = "Cannot start AR: ${e.message ?: e.javaClass.simpleName}" }
    }
    private fun stopScanning() {
        val s = scanner
        if (s != null) s.shutdown() else {scanSession?.pause(); scanSession?.close()}
        scanner = null; scanSession = null; NativeBridge.localMap(null)
    }
    private var lastUpdateVersion = -1L
    private val profileFile get() = File(filesDir, "home-connection.json")

    private val importProfile = registerForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        if (uri != null && !active) lifecycleScope.launch {
            runCatching {
                withContext(Dispatchers.IO) {
                    val bytes = contentResolver.openInputStream(uri)?.use { readProfileBytes(it) }
                        ?: error("Could not open profile")
                    require(bytes.size <= 32768) { "Connection profile is too large" }
                    val text = bytes.toString(Charsets.UTF_8)
                    val parsed = JSONObject(text)
                    require(parsed.optString("schema") == "spatial-harness.home-connection/v1") {
                        "Choose a Home connection profile, not a scene export"
                    }
                    require(parsed.optString("peer_id").isNotBlank()) { "Profile has no Home peer ID" }
                    profileFile.writeText(text)
                    text
                }
            }.onSuccess { profile = it; status = "Profile imported. Sign in with your Auki User account." }
                .onFailure { status = it.message ?: "Profile import failed" }
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        profile = runCatching { profileFile.takeIf { it.exists() }?.readText() ?: "" }.getOrDefault("")
        setContent {
            MaterialTheme(colorScheme = darkColorScheme()) {
                Surface(Modifier.fillMaxSize()) {
                    val ar = scanSession
                    if (ar != null) {
                        BackHandler { stopScanning() }
                        Column(Modifier.safeDrawingPadding()) {
                            Row(Modifier.padding(12.dp)) { Button(onClick = {stopScanning()}) { Text("Stop scan") } }
                            Text(scanStatus, Modifier.padding(horizontal = 12.dp))
                            Text(sharingStatus, Modifier.padding(12.dp), style = MaterialTheme.typography.bodySmall)
                            AndroidView(factory = { ArScanner(this@MainActivity, ar) {scanStatus = it}.also {scanner = it} }, modifier = Modifier.fillMaxWidth().weight(1f))
                        }
                    } else {
                    var email by remember { mutableStateOf("") }
                    var password by remember { mutableStateOf("") }
                    Column(Modifier.safeDrawingPadding().padding(24.dp).verticalScroll(rememberScrollState()),
                        verticalArrangement = Arrangement.spacedBy(16.dp)) {
                        Text("Spatial Annotator", style = MaterialTheme.typography.headlineMedium)
                        Text("Android · Local AR mapping", style = MaterialTheme.typography.titleMedium)
                        Text("Scan without a Portal. Your unaligned 10 cm voxel map appears separately in Home. Camera images stay on this device; voxel geometry is shared with Home when connected.")
                        Button(onClick = {startScanning()}) { Text("Start local scan") }
                        Text("AR uses Google Play Services for AR. https://policies.google.com/privacy", style = MaterialTheme.typography.bodySmall)
                        OutlinedButton(onClick = { importProfile.launch(arrayOf("application/json", "text/plain", "application/octet-stream")) }, enabled = !active) {
                            Text(if (profile.isEmpty()) "Import Home connection" else "Replace Home connection")
                        }
                        if (profile.isNotEmpty()) {
                            val peer = runCatching { JSONObject(profile).optString("peer_id") }.getOrDefault("")
                            Text("Home peer\n$peer", style = MaterialTheme.typography.bodySmall)
                        }
                        OutlinedTextField(email, { email = it }, label = { Text("Auki User email") }, singleLine = true, enabled = !active)
                        OutlinedTextField(password, { password = it }, label = { Text("Password") },
                            visualTransformation = PasswordVisualTransformation(), singleLine = true, enabled = !active)
                        Button(onClick = {
                            val secret = password.toByteArray(Charsets.UTF_8)
                            password = ""
                            val started = NativeBridge.start(profile.toByteArray(), email.trim().toByteArray(), secret,
                                File(noBackupFilesDir, "phone.identity").absolutePath.toByteArray())
                            secret.fill(0)
                            if (started) { lastUpdateVersion = -1L; active = true; status = "Connecting…"; sceneSummary = ""; noteTitles = emptyList() }
                            else status = "Previous connection is still shutting down. Try again shortly."
                        }, enabled = !active && profile.isNotEmpty() && email.isNotBlank() && password.isNotEmpty()) { Text("Connect to Home") }
                        if (active) OutlinedButton(onClick = { NativeBridge.stop(); status = "Disconnecting…" }) { Text("Disconnect") }
                        HorizontalDivider()
                        Text(status)
                        if (sceneSummary.isNotEmpty()) Text(sceneSummary)
                        if (noteTitles.isNotEmpty()) {
                            Text("Notes received", style = MaterialTheme.typography.titleMedium)
                            noteTitles.forEach { Text(it) }
                        }
                    }
                    }
                }
            }
        }
        lifecycleScope.launch {
            repeatOnLifecycle(Lifecycle.State.STARTED) {
                while (true) {
                    val snapshot = withContext(Dispatchers.IO) {
                        NativeBridge.poll()?.let { JSONObject(it.toString(Charsets.UTF_8)) }
                    }
                    if (snapshot != null) {
                        active = !snapshot.optBoolean("finished", true)
                        val update = snapshot.getJSONObject("update")
                        val version = update.getLong("version")
                        if (version == lastUpdateVersion) { delay(500); continue }
                        lastUpdateVersion = version
                        status = update.optString("detail")
                        sharingStatus = update.optString("depth_detail")
                        val render = update.optJSONObject("portals")?.optJSONObject("render")
                        if (render != null) {
                            val notes = render.optJSONArray("notes")
                            sceneSummary = "${render.optJSONArray("portals")?.length() ?: 0} portals · ${render.optJSONArray("voxels")?.length() ?: 0} voxels · ${notes?.length() ?: 0} notes"
                            noteTitles = (0 until (notes?.length() ?: 0)).map { index ->
                                val note = notes!!.getJSONObject(index)
                                note.optJSONObject("details")?.optString("title")?.takeIf { it.isNotBlank() }
                                    ?: note.optString("text")
                            }
                        } else { sceneSummary = ""; noteTitles = emptyList() }
                    }
                    delay(500)
                }
            }
        }
    }

    override fun onStop() {
        stopScanning()
        NativeBridge.stop()
        super.onStop()
    }

    override fun onPause() {
        stopScanning()
        super.onPause()
    }
}
