package com.aukilabs.annotator

import org.junit.Assert.*
import org.junit.Test

class HomeConnectionGateTest {
    @Test fun scanningRequiresLiveHomeAndReceivedScene() {
        assertFalse(homeReady(true,"connecting",false))
        assertFalse(homeReady(true,"connecting",true))
        assertFalse(homeReady(true,"live",false))
        assertTrue(homeReady(true,"live",true))
        assertFalse(homeReady(true,"reconnecting",true))
        assertFalse(homeReady(false,"live",true))
        assertFalse(homeReady(false,"error",true))
    }

    @Test fun savedStartupAndConnectionShowOnlyProgress() {
        assertTrue(awaitingHome(false,false,false,true,true,true,false))
        assertTrue(awaitingHome(false,false,true,false,true,true,false))
        assertTrue(awaitingHome(false,true,false,false,true,true,false))
        assertFalse(awaitingHome(true,true,true,false,true,true,false))
    }

    @Test fun setupAndFailuresRemainRecoverableWithoutScanAccess() {
        assertFalse(awaitingHome(false,false,false,true,false,true,false))
        assertFalse(awaitingHome(false,false,false,true,true,false,false))
        assertFalse(awaitingHome(false,false,false,true,true,true,true))
        assertFalse(awaitingHome(false,false,false,false,true,true,false))
        assertFalse(homeReady(false,"error",false))
    }
}
