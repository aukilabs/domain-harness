package com.aukilabs.annotator

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import java.util.UUID

@RunWith(AndroidJUnit4::class)
class SavedSignInTest {
    private fun fixture(test: (SavedSignInStore,File,String) -> Unit) {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val id = UUID.randomUUID().toString()
        val alias = "annotator.test.$id"
        val filename = "test-sign-in-$id.enc"
        val store = SavedSignInStore(context,alias,filename)
        try { test(store,File(context.noBackupFilesDir,filename),alias) }
        finally { store.clear() }
    }

    @Test fun encryptedSignInSurvivesStoreRecreationAndUsesRandomNonces() = fixture { store,file,alias ->
        val login = SavedSignIn("fixture@example.invalid","fixture-secret-測試".toByteArray())
        try {
            assertNull(store.load())
            store.save(login)
            val first = file.readBytes()
            assertFalse(first.toString(Charsets.UTF_8).contains(login.email))
            assertFalse(first.toString(Charsets.UTF_8).contains("fixture-secret"))
            val context = InstrumentationRegistry.getInstrumentation().targetContext
            val reopened = SavedSignInStore(context,alias,file.name)
            val restored = reopened.load()!!
            try {
                assertEquals(login.email,restored.email)
                assertArrayEquals(login.password,restored.password)
                assertFalse(restored.toString().contains("fixture-secret"))
            } finally { restored.clear() }
            store.save(login)
            assertFalse(first.contentEquals(file.readBytes()))
        } finally { login.clear() }
        assertTrue(login.password.all { it == 0.toByte() })
    }

    @Test fun alteredCiphertextFailsClosedWithoutDiscardingTheFile() = fixture { store,file,_ ->
        val login = SavedSignIn("fixture@example.invalid","fixture-only".toByteArray())
        try { store.save(login) } finally { login.clear() }
        val bytes = file.readBytes(); bytes[bytes.lastIndex] = (bytes.last().toInt() xor 1).toByte()
        file.writeBytes(bytes)
        assertTrue(runCatching { store.load() }.isFailure)
        assertTrue(store.exists())
    }

    @Test fun forgettingOnlyRemovesThisSavedSignIn() = fixture { store,file,_ ->
        // Isolated test files, never the user's actual profile or phone identity.
        val unrelated = File(file.parentFile,"${file.name}.unrelated")
        unrelated.writeText("profile-and-identity-sentinel")
        try {
            val login = SavedSignIn("fixture@example.invalid","fixture-only".toByteArray())
            try { store.save(login) } finally { login.clear() }
            store.clear()
            assertFalse(store.exists()); assertNull(store.load())
            assertEquals("profile-and-identity-sentinel",unrelated.readText())
        } finally { unrelated.delete() }
    }

    @Test fun missingKeyDoesNotSilentlyReplaceCredentials() = fixture { store,file,alias ->
        val login = SavedSignIn("fixture@example.invalid","fixture-only".toByteArray())
        try { store.save(login) } finally { login.clear() }
        val encrypted = file.readBytes()
        java.security.KeyStore.getInstance("AndroidKeyStore").apply { load(null); deleteEntry(alias) }
        assertTrue(runCatching { store.load() }.isFailure)
        assertArrayEquals(encrypted,file.readBytes())
    }
}
