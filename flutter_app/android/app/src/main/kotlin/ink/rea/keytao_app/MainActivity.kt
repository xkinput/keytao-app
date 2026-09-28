package ink.rea.keytao_app

import android.content.Intent
import android.os.Bundle
import io.flutter.embedding.android.FlutterActivity
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.plugin.common.MethodChannel

class MainActivity : FlutterActivity() {
    private var androidChannel: KeytaoAndroidChannel? = null

    override fun configureFlutterEngine(flutterEngine: FlutterEngine) {
        super.configureFlutterEngine(flutterEngine)
        val methodChannel = MethodChannel(flutterEngine.dartExecutor.binaryMessenger, "keytao/android")
        val channel = KeytaoAndroidChannel(this, methodChannel)
        androidChannel = channel
        methodChannel.setMethodCallHandler(channel)
    }

    override fun onRequestPermissionsResult(
        requestCode: Int,
        permissions: Array<out String>,
        grantResults: IntArray,
    ) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults)
        androidChannel?.onRequestPermissionsResult(requestCode)
    }

    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        super.onActivityResult(requestCode, resultCode, data)
        androidChannel?.onActivityResult(requestCode, resultCode, data)
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        KeytaoAndroidPaths.retryResolution()
        super.onCreate(savedInstanceState)
        KeytaoRuntimeLog.adoptAppLoggerIfEnabled()
    }

    override fun onResume() {
        super.onResume()
        KeytaoAndroidPaths.retryResolution()
        KeytaoRuntimeLog.adoptAppLoggerIfEnabled()
    }
}
