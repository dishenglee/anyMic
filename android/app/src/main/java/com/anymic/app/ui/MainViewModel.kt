package com.anymic.app.ui

import android.app.Application
import android.content.Context
import androidx.lifecycle.AndroidViewModel
import com.anymic.app.AnyMicApplication
import com.anymic.app.StreamingClient
import com.anymic.app.model.AppState
import com.anymic.app.net.DiscoveredServer
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

/**
 * Production ViewModel.
 *
 * Obtains the [StreamingClient] singleton from [AnyMicApplication] so that
 * screen rotation never causes a second client to be created.
 *
 * Instantiation: use [ViewModelProvider.AndroidViewModelFactory] in MainActivity
 * (the framework passes the Application automatically).
 */
class MainViewModel(app: Application) : AndroidViewModel(app) {

    private val client: StreamingClient =
        (app as AnyMicApplication).streamingClient

    /** Full application state — drives all three screens. */
    val state: StateFlow<AppState> = client.state

    /** Discovered servers — exposed separately so DeviceListScreen can collect independently. */
    val servers = client.discovery.servers

    // ── Light / dark theme mode ─────────────────────────────────────────────
    enum class ThemeMode { System, Light, Dark }

    private val prefs = app.getSharedPreferences("anymic_prefs", Context.MODE_PRIVATE)

    private val _themeMode = MutableStateFlow(loadThemeMode())
    val themeMode: StateFlow<ThemeMode> = _themeMode.asStateFlow()

    private fun loadThemeMode(): ThemeMode = when (prefs.getString("theme_mode", null)) {
        "light" -> ThemeMode.Light
        "dark"  -> ThemeMode.Dark
        else    -> ThemeMode.System
    }

    /** Toggles between Light and Dark.  System (default on first run) becomes Dark. */
    fun toggleThemeMode() {
        val next = when (_themeMode.value) {
            ThemeMode.System, ThemeMode.Light -> ThemeMode.Dark
            ThemeMode.Dark -> ThemeMode.Light
        }
        _themeMode.value = next
        prefs.edit().putString("theme_mode", next.name.lowercase()).apply()
    }

    fun discover() {
        client.startDiscovery()
    }

    fun stopDiscovery() {
        client.stopDiscovery()
    }

    fun connect(target: DiscoveredServer) {
        client.connect(target)
    }

    /** Manual IP connection — bypasses mDNS for environments where multicast is
     *  blocked (MIUI Wi-Fi optimisation, AP isolation, corporate networks). */
    fun connectByIp(host: String, dataPort: Int = 50127, controlPort: Int = 50128) {
        client.connectDirect(host, dataPort, controlPort)
    }

    fun stop() {
        client.stop()
    }

    override fun onCleared() {
        // Do NOT close the client here — it is Application-scoped.
        super.onCleared()
    }
}
