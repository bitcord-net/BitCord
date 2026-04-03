package net.bitcord.client

import android.graphics.Color
import android.os.Bundle
import android.view.ViewGroup
import androidx.core.view.ViewCompat
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat

class MainActivity : TauriActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        WindowCompat.setDecorFitsSystemWindows(window, false)
        super.onCreate(savedInstanceState)

        // Dark background behind the status bar (matches --color-bc-base).
        val content = window.decorView.findViewById<ViewGroup>(android.R.id.content)
        content.setBackgroundColor(Color.parseColor("#0B0D0F"))

        // White status bar icons (light icons on dark background).
        WindowInsetsControllerCompat(window, window.decorView).isAppearanceLightStatusBars = false

        // Apply the status-bar inset as native padding on the content frame.
        ViewCompat.setOnApplyWindowInsetsListener(content) { view, insets ->
            val top = insets.getInsets(WindowInsetsCompat.Type.statusBars()).top
            view.setPadding(0, top, 0, 0)
            insets
        }
    }
}
