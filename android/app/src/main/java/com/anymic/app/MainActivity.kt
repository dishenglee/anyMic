package com.anymic.app

import android.Manifest
import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.activity.viewModels
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.lifecycle.ViewModelProvider
import com.anymic.app.service.MicForegroundService
import com.anymic.app.ui.AnyMicTheme
import com.anymic.app.ui.AppNavigation
import com.anymic.app.ui.MainViewModel

/**
 * Production MainActivity.
 *
 * Responsibilities:
 *  1. Request RECORD_AUDIO + POST_NOTIFICATIONS at startup.
 *  2. Instantiate [MainViewModel] via AndroidViewModelFactory (so the Application
 *     is passed in correctly and the client is never double-created on rotation).
 *  3. Host the Compose Navigation graph inside [AnyMicTheme].
 *  4. Start / stop [MicForegroundService] based on ViewModel state transitions.
 */
class MainActivity : ComponentActivity() {

    private val viewModel: MainViewModel by viewModels {
        ViewModelProvider.AndroidViewModelFactory.getInstance(application)
    }

    // -----------------------------------------------------------------------
    // Permissions
    // -----------------------------------------------------------------------

    private val requestPermissionsLauncher =
        registerForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) { grants ->
            // Streaming will fail gracefully with an Error state if RECORD_AUDIO is denied.
            val recordGranted = grants[Manifest.permission.RECORD_AUDIO] == true
            if (!recordGranted) {
                // Optionally surface a snackbar — handled by HomeScreen Error state.
            }
        }

    // -----------------------------------------------------------------------
    // onCreate
    // -----------------------------------------------------------------------

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        requestNeededPermissions()
        startForegroundService()

        setContent {
            val themeMode by viewModel.themeMode.collectAsState()
            val systemDark = isSystemInDarkTheme()
            val isDark = when (themeMode) {
                MainViewModel.ThemeMode.Dark   -> true
                MainViewModel.ThemeMode.Light  -> false
                MainViewModel.ThemeMode.System -> systemDark
            }
            AnyMicTheme(darkTheme = isDark) {
                AppNavigation(viewModel = viewModel)
            }
        }
    }

    override fun onDestroy() {
        super.onDestroy()
        // Service stays alive; it self-manages lifecycle via START_STICKY.
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    private fun requestNeededPermissions() {
        val perms = buildList {
            add(Manifest.permission.RECORD_AUDIO)
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                add(Manifest.permission.POST_NOTIFICATIONS)
            }
        }.toTypedArray()
        requestPermissionsLauncher.launch(perms)
    }

    private fun startForegroundService() {
        MicForegroundService.start(applicationContext)
    }
}
