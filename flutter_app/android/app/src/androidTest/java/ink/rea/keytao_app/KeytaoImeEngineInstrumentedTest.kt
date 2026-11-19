package ink.rea.keytao_app

import android.content.Context
import android.util.Log
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.FixMethodOrder
import org.junit.AfterClass
import org.junit.Test
import org.junit.runner.RunWith
import org.junit.runners.MethodSorters
import java.io.File
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicReference

/**
 * Fixed order because the cases share one user directory: the composition case
 * installs and deploys, the source-only case then drops the build output again.
 * Name ordering runs them in exactly that sequence.
 */
@RunWith(AndroidJUnit4::class)
@FixMethodOrder(MethodSorters.NAME_ASCENDING)
class KeytaoImeEngineInstrumentedTest {
    @Test
    fun loadedSessionKeepsComposingDuringDeployment() {
        val context = ApplicationProvider.getApplicationContext<Context>()
        assertTrue(KeytaoSchemaFixture.install(context, allowSynthetic = true) != KeytaoSchemaFixture.Source.NONE)
        val engine = KeytaoImeEngine(context)
        val fresh = KeytaoImeEngine(context)
        val root = requireNotNull(engine.userDir)
        try {
            assertTrue(engine.deployNow())
            val schemaId = engine.setAsciiMode(false).schemaId
            KeytaoAndroidPaths.beginDeployment(root)

            assertTrue(KeytaoAndroidPaths.isDeploymentInProgress(root))
            assertEquals(Readiness.READY, engine.refreshReadiness())
            assertNull("deployment stays silent until a key arrives", engine.deploymentHint)
            val state = engine.processKey('a'.code, 0)
            assertEquals("正在部署中", engine.deploymentHint)
            assertTrue("the loaded engine must keep handling keys", state.accepted)
            assertEquals(schemaId, state.schemaId)
            engine.resetDeploymentInput()
            engine.refreshDeploymentHint()
            assertNull("a new input session starts silent", engine.deploymentHint)
            engine.processKey('a'.code, 0)
            assertEquals("正在部署中", engine.deploymentHint)
            assertEquals(Readiness.DEPLOYING, fresh.refreshReadiness())
            assertFalse("a new engine must not read a build in progress", fresh.ensureReady())
            assertEquals(0L, KeytaoNativeBridge.createSession())

            assertTrue(KeytaoNativeBridge.completeDeployment(root.absolutePath))
            KeytaoAndroidPaths.reloadStampFile(context).writeText("deployment/${System.nanoTime()}")
            assertTrue(engine.reload())
            assertEquals("部署成功", engine.deploymentHint)
            assertEquals(schemaId, engine.processKey('a'.code, 0).schemaId)
        } finally {
            KeytaoNativeBridge.completeDeployment(root.absolutePath)
            fresh.close()
            engine.close()
        }
    }

    @Test
    fun sourceOnlyInstallDoesNotDeployOnEnsureReady() {
        val context = ApplicationProvider.getApplicationContext<Context>()
        // A fresh install has an empty user directory — installing a scheme is a
        // user action in the app, never something the IME does. Reproduce the
        // post-install state here; gating is all this case needs, so a synthetic
        // package is enough when no real one is around.
        val source = KeytaoSchemaFixture.install(context, allowSynthetic = true)
        assertTrue(
            "test fixture should install a schema source (${KeytaoSchemaFixture.missingPackageHint()})",
            source != KeytaoSchemaFixture.Source.NONE,
        )
        val engine = KeytaoImeEngine(context)
        try {
            // Source-only means no build output, whatever an earlier case left behind.
            assertTrue("build output should be removable", KeytaoAndroidPaths.invalidateDeployment(requireNotNull(engine.userDir)))
            assertTrue("schema source should be installed in ${engine.userDir}", engine.hasInstalledSchema())
            assertFalse("schema should not be deployed before this check", engine.hasDeployedSchema())
            assertFalse("ensureReady must not run full deploy on IME hot path", engine.ensureReady())
            assertFalse("ensureReady must not create build artifacts", engine.hasDeployedSchema())
        } finally {
            engine.close()
        }
    }

    @Test
    fun removingLastSchemaDiscardsLoadedSession() {
        val context = ApplicationProvider.getApplicationContext<Context>()
        assertTrue(KeytaoSchemaFixture.install(context, allowSynthetic = true) != KeytaoSchemaFixture.Source.NONE)
        val engine = KeytaoImeEngine(context)
        try {
            assertTrue("fixture should deploy and load", engine.deployNow())
            val root = requireNotNull(engine.userDir)
            File(root, "default.custom.yaml").writeText("patch:\n  schema_list: []\n")
            File(root, "build/.keytao-deployed").delete()
            KeytaoAndroidPaths.reloadStampFile(context).writeText("removed/${System.nanoTime()}")

            assertEquals(Readiness.NOT_INSTALLED, engine.refreshReadiness())
            assertFalse(engine.nativeReady)
            assertEquals("", engine.state().schemaId)
            assertFalse(engine.ensureReady())
            assertEquals("", engine.processKey('a'.code, 0).schemaId)
            assertEquals("removed schemas must not create another native session", 0L, KeytaoNativeBridge.createSession())
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
        // Real candidates need a real package, so no synthetic fallback here.
        val source = KeytaoSchemaFixture.install(context, allowSynthetic = false)
        assumeTrue(KeytaoSchemaFixture.missingPackageHint(), source != KeytaoSchemaFixture.Source.NONE)

        val engine = KeytaoImeEngine(context)
        try {
            assertTrue("native bridge should load", KeytaoNativeBridge.loaded)
            assertTrue("schema should be installed in ${engine.userDir} (from $source)", engine.hasInstalledSchema())
            if (deployBeforeTest) {
                assertTrue("engine should deploy schema data", engine.deployNow())
            } else {
                assertTrue("engine should become ready", engine.ensureReady())
            }
            for (schema in expectedDeployedSchemas) {
                assertTrue(
                    "compiled schema should exist: $schema",
                    requireNotNull(engine.userDir).resolve("build/$schema.schema.yaml").isFile,
                )
            }

            engine.reset()
            var state = engine.setAsciiMode(false)
            for (char in input) {
                val key = AndroidKeyMapper.fromText(char.toString())
                assertTrue("unsupported key: $char", key != null)
                state = engine.processKey(key!!.keyCode, key.modifiers)
            }

            val candidates = engine.allCandidates(96).ifEmpty { state.candidates }
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
        val userRoot = KeytaoSchemaFixture.useIsolatedRoot(ApplicationProvider.getApplicationContext<Context>())
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
        val candidates = engine.allCandidates(96).ifEmpty { state.candidates }.map { it.text }
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

        @JvmStatic @AfterClass
        fun clearRootOverride() {
            KeytaoAndroidPaths.setUserRootForTests(ApplicationProvider.getApplicationContext<Context>(), null)
        }
    }
}
