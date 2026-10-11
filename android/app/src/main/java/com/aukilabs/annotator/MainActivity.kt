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
import androidx.compose.ui.Alignment
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.lifecycleScope
import androidx.lifecycle.repeatOnLifecycle
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.cancelAndJoin
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
    private val annotationMessages = SnackbarHostState()
    private var messageJob: kotlinx.coroutines.Job? = null
    private val annotations = AnnotationController { message,label,action ->
        messageJob?.cancel()
        messageJob = lifecycleScope.launch {
            if (annotationMessages.showSnackbar(message,label,duration=SnackbarDuration.Long) == SnackbarResult.ActionPerformed) action?.invoke()
        }
    }
    private var profile by mutableStateOf("")
    private var status by mutableStateOf("Import Home's connection profile to begin.")
    private var active by mutableStateOf(false)
    private val signInStore by lazy { SavedSignInStore(this) }
    private var hasSavedSignIn by mutableStateOf(false)
    private var signInNotice by mutableStateOf("")
    private var signInBusy by mutableStateOf(false)
    private var autoConnectRequested by mutableStateOf(false)
    private var homeConnected by mutableStateOf(false)
    private var connectionEpoch = 0L
    private var importingProfile by mutableStateOf(false)
    private var pendingSignIn: SavedSignIn? = null
    private var saveSignInJob: kotlinx.coroutines.Job? = null

    private fun connect(signIn: SavedSignIn, remember: Boolean) {
        homeConnected = false
        pendingSignIn?.clear(); pendingSignIn = null
        val started = try {
            NativeBridge.start(profile.toByteArray(),signIn.email.toByteArray(),signIn.password,
                File(noBackupFilesDir,"phone.identity").absolutePath.toByteArray())
        } catch (_: Exception) { false }
        if (started && remember) pendingSignIn = signIn else signIn.clear()
        if (started) {
            connectionEpoch++
            lastUpdateVersion = -1L; active = true; status = "Connecting…"
            sceneSummary = ""; noteTitles = emptyList()
        } else status = "Previous connection is still shutting down. Try again shortly."
    }

    private suspend fun reconnectSavedSignIn() {
        if (profile.isEmpty() || signInBusy) return
        signInBusy = true
        var saved: SavedSignIn? = null
        try {
            withContext(Dispatchers.IO) { saved = signInStore.load() }
            hasSavedSignIn = saved != null
            saved?.let { connect(it,remember=false) }
        } catch (e: kotlinx.coroutines.CancellationException) { throw e }
        catch (_: Exception) {
            hasSavedSignIn = false
            signInNotice = "Saved sign-in could not be unlocked. Please sign in again."
        } finally { saved?.clear(); signInBusy = false }
    }

    private fun rememberSuccessfulSignIn() {
        val saved = pendingSignIn ?: return
        pendingSignIn = null
        saveSignInJob = lifecycleScope.launch {
            signInBusy = true
            try {
                withContext(Dispatchers.IO) { signInStore.save(saved) }
                hasSavedSignIn = true; signInNotice = "Sign-in saved securely on this phone."
            } catch (e: kotlinx.coroutines.CancellationException) { throw e }
            catch (_: Exception) { signInNotice = "Connected, but sign-in could not be saved securely." }
            finally { saved.clear(); signInBusy = false }
        }
    }

    private fun forgetSignIn() {
        homeConnected = false
        autoConnectRequested = false
        pendingSignIn?.clear(); pendingSignIn = null
        NativeBridge.stop()
        lifecycleScope.launch {
            saveSignInJob?.cancelAndJoin()
            signInBusy = true
            try {
                withContext(Dispatchers.IO) { signInStore.clear() }
                hasSavedSignIn = false; signInNotice = "Saved sign-in forgotten. Home profile and phone data are unchanged."
            } catch (e: kotlinx.coroutines.CancellationException) { throw e }
            catch (_: Exception) { signInNotice = "Could not forget saved sign-in. Please try again." }
            finally { signInBusy = false }
        }
    }
    private var sceneSummary by mutableStateOf("")
    private var noteTitles by mutableStateOf(emptyList<String>())
    private var scanSession by mutableStateOf<Session?>(null)
    private var scanner: ArScanner? = null
    private var scanStatus by mutableStateOf("Move slowly to capture depth")
    private var sharingStatus by mutableStateOf("")
    private var homeReference: String? = null
    private var homeRender: JSONObject? = null
    private var homeVoxelsVisible by mutableStateOf(false)
    private var homeVoxelJob: kotlinx.coroutines.Job? = null
    private fun refreshHomeVoxels() {
        homeVoxelJob?.cancel()
        val target = scanner ?: return
        val render = homeRender
        if (!homeVoxelsVisible || homeReference == null || render == null) { target.homeVoxels.set(null); return }
        // Do not leave geometry from a different scene visible during preparation.
        if (target.homeVoxels.get()?.frame != render.optString("frame")) target.homeVoxels.set(null)
        homeVoxelJob = lifecycleScope.launch {
            val mesh = withContext(Dispatchers.Default) {
                try { homeVoxelMesh(render) { coroutineContext.ensureActive() } }
                catch (e: kotlinx.coroutines.CancellationException) { throw e }
                catch (_: Exception) { null }
            }
            if (scanner === target && homeVoxelsVisible && homeRender === render && homeReference != null) target.homeVoxels.set(mesh)
        }
    }
    private val cameraPermission = registerForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        if (granted) startScanning() else { status = "Camera permission is required for local mapping." }
    }
    private fun startScanning() {
        // Also guards delayed camera-permission callbacks after connection loss.
        if (!homeConnected) return
        if (scanSession != null) return
        if (checkSelfPermission(Manifest.permission.CAMERA) != PackageManager.PERMISSION_GRANTED) {
            cameraPermission.launch(Manifest.permission.CAMERA); return
        }
        var candidate: Session? = null
        try {
            if (ArCoreApk.getInstance().requestInstall(this, true) == ArCoreApk.InstallStatus.INSTALL_REQUESTED) {
                status = "Finish installing Google Play Services for AR, reconnect to Home, then tap Start scan again."; return
            }
            val s = Session(this); candidate = s
            check(s.isDepthModeSupported(Config.DepthMode.AUTOMATIC)) { "This device does not support ARCore Depth." }
            s.configure(s.config.apply { depthMode = Config.DepthMode.AUTOMATIC; focusMode = Config.FocusMode.AUTO })
            s.resume(); scanSession = s
        } catch (e: Exception) { candidate?.close(); status = "Cannot start AR: ${e.message ?: e.javaClass.simpleName}" }
    }
    private fun stopScanning() {
        homeVoxelJob?.cancel(); homeVoxelsVisible = false
        val s = scanner
        if (s != null) s.shutdown() else {scanSession?.pause(); scanSession?.close()}
        scanner = null; scanSession = null; NativeBridge.localMap(null)
    }
    private var lastUpdateVersion = -1L
    private val profileFile get() = File(filesDir, "home-connection.json")

    private val importProfile = registerForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        if (uri == null || active) importingProfile = false
        if (uri != null && !active) lifecycleScope.launch {
            try {
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
            }.onSuccess { profile = it; autoConnectRequested = hasSavedSignIn; status = "Profile imported. Connect with your Auki User account." }
                .onFailure { status = it.message ?: "Profile import failed" }
            } finally { importingProfile = false }
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        profile = runCatching { profileFile.takeIf { it.exists() }?.readText() ?: "" }.getOrDefault("")
        hasSavedSignIn = signInStore.exists()
        setContent {
            MaterialTheme(colorScheme = darkColorScheme()) {
                Surface(Modifier.fillMaxSize()) {
                    val ar = scanSession
                    if (ar != null) {
                        BackHandler { stopScanning() }
                        ScanScreen(session = ar,
                            camera = {
                                AndroidView(factory = { ArScanner(this@MainActivity, ar) {scanStatus = it}.also {
                                    scanner = it; it.homeReference = homeReference
                                    it.annotationPick = annotations::pick
                                    annotations.renderNotes = { notes -> it.annotations.notes = notes }
                                    annotations.accepted = { it.performHapticFeedback(android.view.HapticFeedbackConstants.VIRTUAL_KEY) }
                                    annotations.refreshRender()
                                } }, modifier = Modifier.fillMaxSize())
                            },
                            homeVoxelsVisible = homeVoxelsVisible,
                            onHomeVoxelsToggle = {
                                homeVoxelsVisible = !homeVoxelsVisible; refreshHomeVoxels()
                                if (homeVoxelsVisible) {
                                    val hint = when {
                                        homeReference == null -> "Connect to Home to see its voxels"
                                        scanner?.homeAligned != true -> "Scan the portal to align Home's voxels"
                                        (homeRender?.optJSONArray("voxels")?.length() ?: 0) == 0 -> "Home has no voxels yet"
                                        else -> null
                                    }
                                    if (hint != null) lifecycleScope.launch { annotationMessages.showSnackbar(hint) }
                                }
                            },
                            annotations = { AnnotationOverlay(annotations) { x,y,double -> scanner?.annotations?.tap(x,y,double) } },
                            messages = { SnackbarHost(annotationMessages) },
                            feedback = {
                                Button(onClick = {stopScanning()}) { Text("Stop scan") }
                                Text(scanStatus)
                                Text(sharingStatus, style = MaterialTheme.typography.bodySmall)
                            })
                    } else if (awaitingHome(homeConnected,active,signInBusy,autoConnectRequested,
                            profile.isNotEmpty(),hasSavedSignIn,importingProfile)) {
                        val stage = when {
                            active -> status.ifBlank { "Connecting to Home…" }
                            signInBusy -> "Opening saved sign-in…"
                            else -> "Preparing Home connection…"
                        }
                        var stageSeconds by remember(stage) { mutableStateOf(0L) }
                        LaunchedEffect(stage) {
                            val started = android.os.SystemClock.elapsedRealtime()
                            while (true) {
                                delay(1000)
                                stageSeconds = (android.os.SystemClock.elapsedRealtime()-started)/1000
                            }
                        }
                        Box(Modifier.fillMaxSize().safeDrawingPadding().padding(32.dp),contentAlignment=Alignment.Center) {
                            Column(Modifier.fillMaxWidth(),verticalArrangement=Arrangement.spacedBy(12.dp)) {
                                Text(stage,style=MaterialTheme.typography.bodyLarge)
                                // Connection phases have no honest percentage; use an indeterminate bar.
                                LinearProgressIndicator(Modifier.fillMaxWidth())
                                Text("${stageSeconds}s in this step",style=MaterialTheme.typography.bodySmall)
                            }
                        }
                    } else {
                    var email by remember { mutableStateOf("") }
                    var password by remember { mutableStateOf("") }
                    Column(Modifier.safeDrawingPadding().padding(24.dp).verticalScroll(rememberScrollState()),
                        verticalArrangement = Arrangement.spacedBy(16.dp)) {
                        Text("Spatial Annotator", style = MaterialTheme.typography.headlineMedium)
                        Text("Android · Portal-aligned AR mapping", style = MaterialTheme.typography.titleMedium)
                        Text("Scan a Portal from the selected Home scene to align and contribute depth. Double-tap a surface to add a note, double-tap a note to delete it, or tap a note to edit. The camera is shared only while a viewer subscribes.")
                        if (homeConnected) Button(onClick = {startScanning()}) { Text("Start scan") }
                        Text("AR uses Google Play Services for AR. https://policies.google.com/privacy", style = MaterialTheme.typography.bodySmall)
                        OutlinedButton(onClick = {
                            importingProfile = true
                            importProfile.launch(arrayOf("application/json", "text/plain", "application/octet-stream"))
                        }, enabled = !active && !signInBusy && !importingProfile) {
                            Text(if (profile.isEmpty()) "Import Home connection" else "Replace Home connection")
                        }
                        if (profile.isNotEmpty()) {
                            val peer = runCatching { JSONObject(profile).optString("peer_id") }.getOrDefault("")
                            Text("Home peer\n$peer", style = MaterialTheme.typography.bodySmall)
                            Text(runCatching { JSONObject(profile).optString("frame") }.getOrDefault(""), style = MaterialTheme.typography.bodySmall)
                        }
                        if (!hasSavedSignIn) {
                        OutlinedTextField(email, { email = it }, label = { Text("Auki User email") }, singleLine = true, enabled = !active && !signInBusy)
                        OutlinedTextField(password, { password = it }, label = { Text("Password") },
                            visualTransformation = PasswordVisualTransformation(), singleLine = true, enabled = !active && !signInBusy)
                        Text("Sign in once. Your credentials are encrypted on this phone for automatic reconnection.",style=MaterialTheme.typography.bodySmall)
                        Button(onClick = {
                            val secret = password.toByteArray(Charsets.UTF_8)
                            password = ""
                            autoConnectRequested = false; signInNotice = ""
                            connect(SavedSignIn(email.trim(),secret),remember=true)
                        }, enabled = !active && !signInBusy && !importingProfile && profile.isNotEmpty() && email.isNotBlank() && password.isNotEmpty()) { Text("Sign in and remember") }
                        } else {
                            Text("Saved sign-in · reconnects when Annotator opens",style=MaterialTheme.typography.bodySmall)
                            if (!active) Button(onClick = { autoConnectRequested = true },enabled = !signInBusy && !importingProfile && profile.isNotEmpty()) { Text("Reconnect to Home") }
                            OutlinedButton(onClick = { forgetSignIn() },enabled = !signInBusy) { Text("Forget saved sign-in") }
                        }
                        if (active) OutlinedButton(onClick = {
                            homeConnected = false
                            autoConnectRequested = false; pendingSignIn?.clear(); pendingSignIn = null
                            NativeBridge.stop(); status = "Disconnecting…"
                        }) { Text("Disconnect") }
                        if (signInNotice.isNotEmpty()) Text(signInNotice,style=MaterialTheme.typography.bodySmall)
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
                    annotations.expireAcknowledgements()
                    val polledEpoch = connectionEpoch
                    val snapshot = withContext(Dispatchers.IO) {
                        NativeBridge.poll()?.let { JSONObject(it.toString(Charsets.UTF_8)) }
                    }
                    // A UI sign-in can start while the old connection is being polled.
                    if (polledEpoch != connectionEpoch) { delay(500); continue }
                    if (snapshot != null) {
                        active = !snapshot.optBoolean("finished", true)
                        if (!active) { pendingSignIn?.clear(); pendingSignIn = null }
                        val update = snapshot.getJSONObject("update")
                        homeConnected = homeReady(active,update.optString("state"),
                            update.optJSONObject("portals")?.optJSONObject("scenegraph") != null)
                        if (active && update.optString("state") == "live") rememberSuccessfulSignIn()
                        val version = update.getLong("version")
                        if (version == lastUpdateVersion) {
                            if (autoConnectRequested && !active && !signInBusy && !importingProfile) {
                                autoConnectRequested = false; reconnectSavedSignIn()
                            }
                            delay(500); continue
                        }
                        lastUpdateVersion = version
                        status = update.optString("detail")
                        sharingStatus = listOf(update.optString("local_detail"),update.optString("depth_detail")).filter { it.isNotBlank() }.joinToString("\n")
                        homeReference = if (active && update.optString("state") == "live") update.optJSONObject("portals")?.optJSONObject("scenegraph")?.toString() else null
                        scanner?.homeReference = homeReference
                        val render = update.optJSONObject("portals")?.optJSONObject("render")
                        homeRender = if (homeReference != null) render else null
                        refreshHomeVoxels()
                        annotations.scene(if (homeReference != null) render?.optString("frame") else null,render?.optJSONArray("notes"))
                        annotations.result(update.optJSONObject("annotation"))
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
                    if (autoConnectRequested && !active && !signInBusy && !importingProfile) {
                        autoConnectRequested = false; reconnectSavedSignIn()
                    }
                    delay(500)
                }
            }
        }
    }

    override fun onStart() {
        super.onStart()
        autoConnectRequested = true
    }

    override fun onStop() {
        homeConnected = false
        autoConnectRequested = false
        pendingSignIn?.clear(); pendingSignIn = null
        stopScanning()
        NativeBridge.stop()
        super.onStop()
    }

    override fun onPause() {
        stopScanning()
        super.onPause()
    }
}
