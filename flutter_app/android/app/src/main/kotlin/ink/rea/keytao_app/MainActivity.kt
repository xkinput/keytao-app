package ink.rea.keytao_app

import android.os.Bundle
import io.flutter.embedding.android.FlutterActivity
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.plugin.common.MethodChannel

class MainActivity : FlutterActivity() {
    private var androidChannel: KeytaoAndroidChannel? = null

    override fun configureFlutterEngine(flutterEngine: FlutterEngine) {
        super.configureFlutterEngine(flutterEngine)
        val channel = KeytaoAndroidChannel(this)
        androidChannel = channel
        MethodChannel(flutterEngine.dartExecutor.binaryMessenger, "keytao/android")
            .setMethodCallHandler(channel)
    }

    override fun onRequestPermissionsResult(
        requestCode: Int,
        permissions: Array<out String>,
        grantResults: IntArray,
    ) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults)
        androidChannel?.onRequestPermissionsResult(requestCode)
    }

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
