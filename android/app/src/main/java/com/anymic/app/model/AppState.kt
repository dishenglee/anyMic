package com.anymic.app.model

import com.anymic.app.net.DiscoveredServer

sealed class AppState {
    data object Idle : AppState()
    data class Discovering(val servers: List<DiscoveredServer>) : AppState()
    data class Connecting(val target: DiscoveredServer) : AppState()
    data class Streaming(val target: DiscoveredServer, val stats: StreamStats) : AppState()
    data class Error(val message: String) : AppState()
}

data class StreamStats(
    val packetsSent: Long = 0,
    val bytesSent: Long = 0,
    val rttMs: Int = 0,
    val droppedFrames: Long = 0,
    val source: String = "",
    val sessionId: String = "",
)
