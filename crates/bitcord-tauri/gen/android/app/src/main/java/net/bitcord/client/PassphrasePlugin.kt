package net.bitcord.client

import android.app.Activity
import androidx.security.crypto.EncryptedSharedPreferences
import androidx.security.crypto.MasterKey
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin

private const val PREFS_FILE = "bitcord_secure"
private const val KEY_PASSPHRASE = "passphrase"

@InvokeArg
class SavePassphraseArgs {
    lateinit var value: String
}

@TauriPlugin
class PassphrasePlugin(private val activity: Activity) : Plugin(activity) {

    private fun encryptedPrefs() = EncryptedSharedPreferences.create(
        activity,
        PREFS_FILE,
        MasterKey.Builder(activity)
            .setKeyScheme(MasterKey.KeyScheme.AES256_GCM)
            .build(),
        EncryptedSharedPreferences.PrefKeyEncryptionScheme.AES256_SIV,
        EncryptedSharedPreferences.PrefValueEncryptionScheme.AES256_GCM
    )

    @Command
    fun getPassphrase(invoke: Invoke) {
        val value = encryptedPrefs().getString(KEY_PASSPHRASE, null)
        val ret = JSObject()
        if (value != null) {
            ret.put("value", value)
        }
        invoke.resolve(ret)
    }

    @Command
    fun savePassphrase(invoke: Invoke) {
        val args = invoke.parseArgs(SavePassphraseArgs::class.java)
        encryptedPrefs().edit().putString(KEY_PASSPHRASE, args.value).apply()
        invoke.resolve()
    }

    @Command
    fun deletePassphrase(invoke: Invoke) {
        encryptedPrefs().edit().remove(KEY_PASSPHRASE).apply()
        invoke.resolve()
    }
}
