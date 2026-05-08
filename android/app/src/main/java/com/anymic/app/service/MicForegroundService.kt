package com.anymic.app.service

import android.app.Notification
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.net.wifi.WifiManager
import android.os.Binder
import android.os.Build
import android.os.IBinder
import android.os.PowerManager
import android.util.Log
import androidx.core.app.NotificationCompat
import androidx.core.content.ContextCompat
import com.anymic.app.AnyMicApplication
import com.anymic.app.MainActivity
import com.anymic.app.R
import com.anymic.app.StreamingClient
import com.anymic.app.model.AppState
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.launchIn
import kotlinx.coroutines.flow.onEach

/**
 * Foreground service that keeps the streaming pipeline alive when the app is in the background.
 *
 * Design:
 *  - The [StreamingClient] is Application-scoped; this service does NOT own it.
 *  - The service owns Wi-Fi high-performance lock + partial wake lock to prevent
 *    the CPU and Wi-Fi radio from sleeping during active streaming.
 *  - Notification is updated whenever [AppState] changes.
 */
class MicForegroundService : Service() {

    companion object {
        private const val TAG             = "MicForegroundService"
        const val NOTIFICATION_ID         = 1
        private const val CHANNEL_ID      = AnyMicApplication.CHANNEL_ID

        fun start(ctx: Context) {
            val intent = Intent(ctx, MicForegroundService::class.java)
            ContextCompat.startForegroundService(ctx, intent)
        }

        fun stop(ctx: Context) {
            ctx.stopService(Intent(ctx, MicForegroundService::class.java))
        }
    }

    // Application-level client (singleton)
    private lateinit var client: StreamingClient

    // Locks
    private var wifiLock:  WifiManager.WifiLock? = null
    private var wakeLock:  PowerManager.WakeLock? = null

    // Coroutine scope for observing state
    private val serviceScope = CoroutineScope(SupervisorJob() + Dispatchers.Main)

    inner class LocalBinder : Binder() {
        fun getService(): MicForegroundService = this@MicForegroundService
    }

    private val binder = LocalBinder()

    // ------------------------------------------------------------------
    // Lifecycle
    // ------------------------------------------------------------------

    override fun onCreate() {
        super.onCreate()
        client = (applicationContext as AnyMicApplication).streamingClient

        // Start foreground immediately with "waiting" notification
        val notification = buildNotification(getString(R.string.notif_waiting))
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            startForeground(
                NOTIFICATION_ID,
                notification,
                ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE,
            )
        } else {
            startForeground(NOTIFICATION_ID, notification)
        }

        acquireWifiLock()
        acquireWakeLock()
        observeState()

        Log.i(TAG, "Service created")
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int =
        START_STICKY

    override fun onDestroy() {
        releaseWifiLock()
        releaseWakeLock()
        serviceScope.cancel()
        Log.i(TAG, "Service destroyed")
        super.onDestroy()
    }

    override fun onBind(intent: Intent?): IBinder = binder

    // ------------------------------------------------------------------
    // State observation → notification updates
    // ------------------------------------------------------------------

    private fun observeState() {
        client.state
            .onEach { state -> updateNotification(state) }
            .launchIn(serviceScope)
    }

    private fun updateNotification(state: AppState) {
        val text = when (state) {
            is AppState.Idle        -> getString(R.string.notif_waiting)
            is AppState.Discovering -> getString(R.string.notif_discovering)
            is AppState.Connecting  -> getString(R.string.notif_connecting, state.target.name)
            is AppState.Streaming   ->
                "推流中：${state.target.name}  RTT ${state.stats.rttMs} ms"
            is AppState.Error       -> getString(R.string.notif_error, state.message.take(60))
        }
        val nm = getSystemService(NOTIFICATION_SERVICE) as android.app.NotificationManager
        nm.notify(NOTIFICATION_ID, buildNotification(text))
    }

    private fun buildNotification(contentText: String): Notification {
        val tapIntent = PendingIntent.getActivity(
            this,
            0,
            Intent(this, MainActivity::class.java).apply {
                flags = Intent.FLAG_ACTIVITY_SINGLE_TOP
            },
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        return NotificationCompat.Builder(this, CHANNEL_ID)
            .setSmallIcon(android.R.drawable.ic_btn_speak_now)
            .setContentTitle(getString(R.string.app_name))
            .setContentText(contentText)
            .setContentIntent(tapIntent)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setPriority(NotificationCompat.PRIORITY_LOW)
            .build()
    }

    // ------------------------------------------------------------------
    // Locks
    // ------------------------------------------------------------------

    @Suppress("DEPRECATION")
    private fun acquireWifiLock() {
        val wm = applicationContext.getSystemService(Context.WIFI_SERVICE) as WifiManager
        wifiLock = wm.createWifiLock(WifiManager.WIFI_MODE_FULL_HIGH_PERF, "anymic-wifi").also {
            it.acquire()
        }
        Log.d(TAG, "Wi-Fi high-perf lock acquired")
    }

    private fun releaseWifiLock() {
        wifiLock?.takeIf { it.isHeld }?.release()
        wifiLock = null
        Log.d(TAG, "Wi-Fi lock released")
    }

    private fun acquireWakeLock() {
        val pm = getSystemService(POWER_SERVICE) as PowerManager
        @Suppress("DEPRECATION")
        wakeLock = pm.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "anymic:streaming").also {
            it.acquire(/* timeout */ 4 * 60 * 60 * 1000L)   // 4 hours max guard
        }
        Log.d(TAG, "Partial wake lock acquired")
    }

    private fun releaseWakeLock() {
        wakeLock?.takeIf { it.isHeld }?.release()
        wakeLock = null
        Log.d(TAG, "Wake lock released")
    }
}
