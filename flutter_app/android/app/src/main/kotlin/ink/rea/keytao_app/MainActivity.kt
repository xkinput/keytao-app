package ink.rea.keytao_app

import android.os.Bundle
import io.flutter.embedding.android.FlutterActivity

class MainActivity : FlutterActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        KeytaoStorageMigration.start(applicationContext)
        super.onCreate(savedInstanceState)
        KeytaoRuntimeLog.adoptAppLoggerIfEnabled()
    }

    override fun onResume() {
        super.onResume()
        KeytaoAndroidPaths.retryResolution()
        KeytaoStorageMigration.start(applicationContext)
        KeytaoRuntimeLog.adoptAppLoggerIfEnabled()
    }
}
