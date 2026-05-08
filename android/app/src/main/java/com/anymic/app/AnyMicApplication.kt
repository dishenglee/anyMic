package com.anymic.app

import android.app.Application
import android.app.NotificationChannel
import android.app.NotificationManager
import android.os.Build

/**
 * Application singleton.
 *
 * Responsibilities:
 *  1. Holds the single [StreamingClient] instance shared across ViewModel & Service.
 *  2. Creates the notification channel required before any foreground-service notification
 *     is posted (must happen before Service.startForeground is called).
 */
class AnyMicApplication : Application() {

    val streamingClient: StreamingClient by lazy { StreamingClient(this) }

    override fun onCreate() {
        super.onCreate()
        createNotificationChannel()
    }

    private fun createNotificationChannel() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val channel = NotificationChannel(
                CHANNEL_ID,
                "anyMic 麦克风服务",
                NotificationManager.IMPORTANCE_LOW,
            ).apply {
                description = "anyMic 推流保活通知"
                setShowBadge(false)
            }
            val nm = getSystemService(NOTIFICATION_SERVICE) as NotificationManager
            nm.createNotificationChannel(channel)
        }
    }

    companion object {
        const val CHANNEL_ID = "anymic_mic"
    }
}
