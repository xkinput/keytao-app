package ink.rea.keytao_app

import android.Manifest
import android.app.Activity
import android.content.Context
import android.content.pm.PackageManager
import android.os.Bundle

class KeytaoVoicePermissionActivity : Activity() {
    private var rationaleBeforeRequest: Boolean? = null

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        if (savedInstanceState?.containsKey(rationaleBeforeRequestKey) == true) {
            rationaleBeforeRequest = savedInstanceState.getBoolean(rationaleBeforeRequestKey)
        }
        if (hasPermission(this) || isPermanentlyDenied(this)) {
            finish()
        } else if (savedInstanceState == null) {
            rationaleBeforeRequest = shouldShowRequestPermissionRationale(Manifest.permission.RECORD_AUDIO)
            requestPermissions(arrayOf(Manifest.permission.RECORD_AUDIO), requestCode)
        }
    }

    override fun onSaveInstanceState(outState: Bundle) {
        super.onSaveInstanceState(outState)
        rationaleBeforeRequest?.let { outState.putBoolean(rationaleBeforeRequestKey, it) }
    }

    override fun onRequestPermissionsResult(
        requestCode: Int,
        permissions: Array<out String>,
        grantResults: IntArray,
    ) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults)
        if (requestCode != Companion.requestCode) return
        if (KeytaoVoiceInput.shouldMarkPermissionPermanentlyDenied(
                denied = !hasPermission(this) && grantResults.firstOrNull() == PackageManager.PERMISSION_DENIED,
                rationaleBeforeRequest = rationaleBeforeRequest,
                rationaleAfterRequest = shouldShowRequestPermissionRationale(Manifest.permission.RECORD_AUDIO),
            )
        ) {
            getSharedPreferences(preferencesName, Context.MODE_PRIVATE).edit()
                .putBoolean(permanentlyDeniedKey, true)
                .apply()
        }
        finish()
    }

    companion object {
        private const val requestCode = 1
        private const val preferencesName = "voice_input"
        private const val permanentlyDeniedKey = "permanently_denied"
        private const val rationaleBeforeRequestKey = "rationale_before_request"

        fun hasPermission(context: Context): Boolean {
            val granted = context.checkSelfPermission(Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED
            if (granted) {
                context.getSharedPreferences(preferencesName, Context.MODE_PRIVATE).edit()
                    .remove(permanentlyDeniedKey).apply()
            }
            return granted
        }

        fun isPermanentlyDenied(context: Context): Boolean {
            return !hasPermission(context) && context.getSharedPreferences(preferencesName, Context.MODE_PRIVATE)
                .getBoolean(permanentlyDeniedKey, false)
        }
    }
}
