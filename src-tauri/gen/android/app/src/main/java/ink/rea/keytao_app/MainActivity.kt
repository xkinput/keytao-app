package ink.rea.keytao_app

import android.os.Bundle
import android.util.Log
import android.view.WindowManager
import androidx.activity.enableEdgeToEdge
import app.tauri.plugin.PluginManager

class MainActivity : TauriActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
    val started = System.nanoTime()
    PluginManager.onActivityCreate(this)
    window.setSoftInputMode(WindowManager.LayoutParams.SOFT_INPUT_ADJUST_RESIZE)
    enableEdgeToEdge()
    // Upgrades from a build that kept its data in shared storage: pull the old
    // install into app-specific storage once, off the UI thread.
    val context = applicationContext
    Thread({ runCatching { KeytaoAndroidPaths.migrateLegacyRootIfNeeded(context) } }, "KeyTao-Migrate").start()
    super.onCreate(savedInstanceState)
    KeytaoRuntimeLog.event("lifecycle", "app_create", KeytaoRuntimeLog.elapsedMs(started))
    KeytaoRuntimeLog.adoptAppLoggerIfEnabled()
    // Tauri initializes its logger asynchronously; preserve a fixed startup fallback.
    Log.i("KeytaoApp", "{\"cat\":\"lifecycle\",\"ev\":\"app_create\"}")
  }

  override fun onResume() {
    super.onResume()
    KeytaoRuntimeLog.adoptAppLoggerIfEnabled()
  }
}
