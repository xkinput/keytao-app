package ink.rea.keytao_app

import android.content.Context
import android.util.Log
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class KeytaoImeEngineInstrumentedTest {
    @Test
    fun sourceOnlyInstallDoesNotDeployOnEnsureReady() {
        val context = ApplicationProvider.getApplicationContext<Context>()
        val engine = KeytaoImeEngine(context)
        try {
            assertTrue("schema source should be installed in ${engine.userDir}", engine.hasInstalledSchema())
            assertFalse("schema should not be deployed before this check", engine.hasDeployedSchema())
            assertFalse("ensureReady must not run full deploy on IME hot path", engine.ensureReady())
            assertFalse("ensureReady must not create build artifacts", engine.hasDeployedSchema())
        } finally {
            engine.close()
        }
    }

    @Test
    fun selectedSchemeComposesCandidates() {
        val args = InstrumentationRegistry.getArguments()
        val expectedSchemaName = args.getString("expectedSchemaName").orEmpty()
        val input = args.getString("input").orEmpty()
        val expectedCandidate = args.getString("expectedCandidate").orEmpty()
        val deployBeforeTest = args.getString("deployBeforeTest") == "true"
        assertFalse("expectedSchemaName argument is required", expectedSchemaName.isBlank())
        assertFalse("input argument is required", input.isBlank())

        val context = ApplicationProvider.getApplicationContext<Context>()
        val engine = KeytaoImeEngine(context)
        try {
            assertTrue("native bridge should load", KeytaoNativeBridge.loaded)
            assertTrue("schema should be installed in ${engine.userDir}", engine.hasInstalledSchema())
            if (deployBeforeTest) {
                assertTrue("engine should deploy schema data", engine.deployNow())
            } else {
                assertTrue("engine should become ready", engine.ensureReady())
            }

            engine.reset()
            var state = engine.setAsciiMode(false)
            for (char in input) {
                val key = AndroidKeyMapper.fromText(char.toString())
                assertTrue("unsupported key: $char", key != null)
                state = engine.processKey(key!!.keyCode, key.modifiers)
            }

            val candidates = state.allCandidates.ifEmpty { state.candidates }
            val candidateTexts = candidates.map { it.text }
            Log.i(
                tag,
                "schema=${state.schemaName}, ascii=${state.asciiMode}, preedit=${state.preedit}, candidates=$candidateTexts"
            )

            assertEquals(expectedSchemaName, state.schemaName)
            assertTrue("candidate list should not be empty for '$input'", candidateTexts.isNotEmpty())
            if (expectedCandidate.isNotBlank()) {
                assertTrue(
                    "expected candidate '$expectedCandidate' in $candidateTexts",
                    candidateTexts.contains(expectedCandidate)
                )
            }
        } finally {
            engine.close()
        }
    }

    companion object {
        private const val tag = "KeytaoImeSmoke"
    }
}
