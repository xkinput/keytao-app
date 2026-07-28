package ink.rea.keytao_app

import android.content.Context
import android.util.Log
import androidx.test.platform.app.InstrumentationRegistry
import java.io.File
import java.io.InputStream
import java.util.zip.ZipInputStream

/**
 * Puts a schema source into the IME user directory before a test looks at it.
 *
 * Nothing in production does this. On Android the user installs a scheme from
 * the app — download the release zip, then
 * `ScopedStoragePlugin.smartExtractZipToPrivate()` merges it into
 * `getExternalFilesDir(null)/keytao` — so a fresh install genuinely starts with
 * an empty user directory and `KeytaoAndroidPaths.hasInstalledSchema()` is
 * false. The APK only bundles librime *shared* data
 * (`assets/keytao-rime-data`: default.yaml, essay.txt, opencc, luna_pinyin…),
 * which `KeytaoImeEngine.ensureBundledSharedData()` copies to
 * `<userRoot>/rime-data`; that is the shared data dir, never a schema source.
 * Every instrumented case that needs an installed scheme therefore has to
 * reproduce the post-install state itself.
 *
 * Sources, in order:
 * 1. whatever is already installed — an earlier case in the same run, or a
 *    device that a human installed a scheme on;
 * 2. `-Pandroid.testInstrumentationRunnerArguments.schemaFixture=<device dir>`,
 *    for running against a newer package without rebuilding the test APK. The
 *    directory has to be readable by the app UID, so app-specific storage —
 *    not `/sdcard` or `/data/local/tmp`;
 * 3. `keytao-schema-fixture.zip` from the *test* APK assets: a pinned KeyTao
 *    package, which is what keeps the composition assertions deterministic;
 * 4. a minimal synthetic package, for the cases that only exercise the
 *    install/deploy gating and never reach librime. Same idea as
 *    `crates/keytao-core/tests/support/smoke_fixture.rs`.
 */
object KeytaoSchemaFixture {
    enum class Source { EXISTING, STAGED, BUNDLED, SYNTHETIC, NONE }

    private const val tag = "KeytaoSchemaFixture"
    private const val fixtureArgument = "schemaFixture"
    private const val fixtureAsset = "keytao-schema-fixture.zip"

    /** Must start with `keytao` — [isManagedSchema] is what makes it count as installed. */
    private const val syntheticSchemaId = "keytao_fixture"

    /**
     * Make sure the user directory holds a schema source, and report where it
     * came from. Deployment output is left alone: callers decide whether they
     * want the source-only state or a compiled one.
     */
    fun install(context: Context, allowSynthetic: Boolean = false): Source {
        val root = KeytaoAndroidPaths.userRoot(context)
        if (KeytaoAndroidPaths.hasInstalledSchema(root)) return Source.EXISTING

        stagedFixtureDir()?.let { staged ->
            copyTree(staged, root)
            if (KeytaoAndroidPaths.hasInstalledSchema(root)) {
                Log.i(tag, "installed schema source from staged fixture $staged")
                return Source.STAGED
            }
        }

        if (unpackBundledFixture(root)) {
            Log.i(tag, "installed schema source from test asset $fixtureAsset")
            return Source.BUNDLED
        }

        if (allowSynthetic) {
            writeSyntheticPackage(root)
            if (KeytaoAndroidPaths.hasInstalledSchema(root)) {
                Log.i(tag, "installed synthetic schema source $syntheticSchemaId")
                return Source.SYNTHETIC
            }
        }

        return Source.NONE
    }

    /** What to do when no real package is available; used in assertion messages. */
    fun missingPackageHint(): String =
        "no KeyTao scheme package to install: put one in " +
            "src/androidTest/assets/$fixtureAsset (zip of a release install: default.custom.yaml, " +
            "keytao*.yaml, rime.lua, lua/), or pass " +
            "-Pandroid.testInstrumentationRunnerArguments.$fixtureArgument=<device dir readable by the app>"

    private fun stagedFixtureDir(): File? {
        val path = InstrumentationRegistry.getArguments().getString(fixtureArgument).orEmpty()
        if (path.isBlank()) return null
        val dir = File(path)
        if (!dir.isDirectory || !dir.canRead()) {
            Log.w(tag, "staged fixture is not readable by the app UID: $dir")
            return null
        }
        return dir
    }

    private fun copyTree(source: File, target: File) {
        source.listFiles().orEmpty().forEach { child ->
            child.copyRecursively(File(target, child.name), overwrite = true)
        }
    }

    private fun unpackBundledFixture(root: File): Boolean {
        val assets = InstrumentationRegistry.getInstrumentation().context.assets
        val present = runCatching { assets.list("").orEmpty().contains(fixtureAsset) }
            .getOrDefault(false)
        if (!present) return false
        val unpacked = runCatching { assets.open(fixtureAsset).use { unzip(it, root) } }
        if (unpacked.isFailure) {
            Log.w(tag, "failed to unpack $fixtureAsset", unpacked.exceptionOrNull())
            return false
        }
        return KeytaoAndroidPaths.hasInstalledSchema(root)
    }

    private fun unzip(input: InputStream, root: File) {
        val canonicalRoot = root.canonicalFile
        val prefix = canonicalRoot.path + File.separator
        ZipInputStream(input.buffered()).use { zip ->
            while (true) {
                val entry = zip.nextEntry ?: break
                val target = File(canonicalRoot, entry.name).canonicalFile
                require(target.path.startsWith(prefix)) {
                    "zip entry escapes the user directory: ${entry.name}"
                }
                if (entry.isDirectory) {
                    target.mkdirs()
                } else {
                    target.parentFile?.mkdirs()
                    target.outputStream().use { output -> zip.copyTo(output) }
                }
                zip.closeEntry()
            }
        }
    }

    /**
     * Enough of a package for [KeytaoAndroidPaths.hasInstalledSchema] and for a
     * librime deployment, without shipping dictionary data. Deliberately tiny:
     * cases that need real candidates use the pinned package instead.
     */
    private fun writeSyntheticPackage(root: File) {
        root.mkdirs()
        File(root, "default.custom.yaml").writeText(
            """
            patch:
              schema_list:
                - schema: $syntheticSchemaId

            """.trimIndent()
        )
        File(root, "$syntheticSchemaId.schema.yaml").writeText(
            """
            schema:
              schema_id: $syntheticSchemaId
              name: KeyTao Fixture
              version: "1"
            switches:
              - name: ascii_mode
                reset: 0
            engine:
              processors:
                - ascii_composer
                - speller
                - selector
                - navigator
                - express_editor
              segmentors:
                - ascii_segmentor
                - abc_segmentor
                - fallback_segmentor
              translators:
                - table_translator
            speller:
              alphabet: 'abcdefghijklmnopqrstuvwxyz'
            translator:
              dictionary: $syntheticSchemaId
              enable_completion: false
              enable_encoder: false
              enable_sentence: false
              enable_user_dict: false

            """.trimIndent()
        )
        File(root, "$syntheticSchemaId.dict.yaml").writeText(
            """
            ---
            name: $syntheticSchemaId
            version: "1"
            sort: original
            use_preset_vocabulary: false
            ...
            甲	aa
            乙	aa

            """.trimIndent()
        )
    }
}
