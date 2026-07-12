package ink.rea.keytao_app

import android.content.Context
import android.util.Log
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicReference

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
        val expectedDeployedSchemas = args.getString("expectedDeployedSchemas")
            .orEmpty()
            .split(',')
            .map(String::trim)
            .filter(String::isNotEmpty)
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
            for (schema in expectedDeployedSchemas) {
                assertTrue(
                    "compiled schema should exist: $schema",
                    engine.userDir.resolve("build/$schema.schema.yaml").isFile,
                )
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

    @Test
    fun switchingInstalledSchemesReloadsInOneProcess() {
        val fixtureRootPath = InstrumentationRegistry.getArguments().getString("fixtureRoot").orEmpty()
        assumeTrue("fixtureRoot argument is required", fixtureRootPath.isNotBlank())

        val fixtureRoot = File(fixtureRootPath)
        val userRoot = KeytaoAndroidPaths.userRoot()
        assertTrue("fixture root should exist: $fixtureRoot", fixtureRoot.isDirectory)
        userRoot.listFiles().orEmpty().forEach(File::deleteRecursively)

        val cases = listOf(
            SchemeCase("keytao", "键道6", "ba", "不能", listOf("keytao", "keytao-dz", "keytao-bj", "keytao-cx")),
            SchemeCase("xmjd", "🌟🐈", "ba", "不能", listOf("xmjd6", "xmjd6.cx", "pinyin_simp", "liangfen", "xmjd6.gbk")),
            SchemeCase("txjx", "天行键", "aa", "那又", listOf("txjx", "txjx.cx", "txjx.danzi", "txjx.gbk", "liangfen")),
            SchemeCase("keydo", "键道·我流", "bbb", "并不比", listOf("keydo")),
        )

        val context = ApplicationProvider.getApplicationContext<Context>()
        val engine = KeytaoImeEngine(context)
        try {
            for ((index, case) in cases.withIndex()) {
                overlayDirectory(File(fixtureRoot, case.fixture), userRoot)

                val result = deployInRemoteProcess(context)
                assertTrue(
                    "${case.fixture} remote deployment failed: ${result.error}",
                    result.success,
                )
                assertTrue("${case.fixture} should report deployed artifacts", result.deployed)
                for (schema in case.deployedSchemas) {
                    assertTrue(
                        "${case.fixture} compiled schema should exist: $schema",
                        File(userRoot, "build/$schema.schema.yaml").isFile,
                    )
                }
                val ready = if (index == 0) engine.ensureReady() else engine.reload()
                assertTrue("${case.fixture} runtime should load deployed artifacts", ready)
                assertComposition(engine, case)
            }
        } finally {
            engine.close()
        }
    }

    private fun deployInRemoteProcess(context: Context): KeytaoRimeDeployClient.Result {
        val latch = CountDownLatch(1)
        val result = AtomicReference<KeytaoRimeDeployClient.Result>()
        KeytaoRimeDeployClient.deploy(context, timeoutMs = 120_000L) {
            result.set(it)
            latch.countDown()
        }
        assertTrue("remote deployment did not complete", latch.await(150, TimeUnit.SECONDS))
        return requireNotNull(result.get())
    }

    private fun assertComposition(engine: KeytaoImeEngine, case: SchemeCase) {
        engine.reset()
        var state = engine.setAsciiMode(false)
        for (char in case.input) {
            val key = AndroidKeyMapper.fromText(char.toString())
            assertTrue("unsupported key: $char", key != null)
            state = engine.processKey(key!!.keyCode, key.modifiers)
        }
        val candidates = state.allCandidates.ifEmpty { state.candidates }.map { it.text }
        Log.i(tag, "fixture=${case.fixture}, schema=${state.schemaName}, candidates=$candidates")
        assertEquals(case.schemaName, state.schemaName)
        assertTrue("expected ${case.candidate} in $candidates", candidates.contains(case.candidate))
    }

    private fun overlayDirectory(source: File, target: File) {
        assertTrue("fixture should exist: $source", source.isDirectory)
        source.listFiles().orEmpty().forEach { child ->
            assertTrue(
                "fixture copy should succeed: $child",
                child.copyRecursively(File(target, child.name), overwrite = true),
            )
        }
    }

    private data class SchemeCase(
        val fixture: String,
        val schemaName: String,
        val input: String,
        val candidate: String,
        val deployedSchemas: List<String>,
    )

    companion object {
        private const val tag = "KeytaoImeSmoke"
    }
}
