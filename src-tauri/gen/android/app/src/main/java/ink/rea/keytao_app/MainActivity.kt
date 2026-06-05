package ink.rea.keytao_app

import android.os.Bundle
import androidx.activity.enableEdgeToEdge
import app.tauri.plugin.PluginManager

class MainActivity : TauriActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
    PluginManager.onActivityCreate(this)
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
  }
}
