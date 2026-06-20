package ink.rea.keytao_app

import android.os.Bundle
import android.view.WindowManager
import androidx.activity.enableEdgeToEdge
import app.tauri.plugin.PluginManager

class MainActivity : TauriActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
    PluginManager.onActivityCreate(this)
    window.setSoftInputMode(WindowManager.LayoutParams.SOFT_INPUT_ADJUST_RESIZE)
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
  }
}
