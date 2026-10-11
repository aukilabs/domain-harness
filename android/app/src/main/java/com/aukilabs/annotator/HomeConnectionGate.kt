package com.aukilabs.annotator

/** A running native peer is not enough: Home must have delivered its scene. */
internal fun homeReady(active: Boolean, state: String, hasReference: Boolean): Boolean =
    active && state == "live" && hasReference

internal fun awaitingHome(ready: Boolean, active: Boolean, busy: Boolean,
                         autoConnect: Boolean, hasProfile: Boolean, hasSavedSignIn: Boolean,
                         importing: Boolean): Boolean = !ready &&
    (active || busy || (autoConnect && hasProfile && hasSavedSignIn && !importing))
