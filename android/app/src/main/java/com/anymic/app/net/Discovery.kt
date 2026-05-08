package com.anymic.app.net

import android.content.Context
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import android.net.wifi.WifiManager
import android.util.Log
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow

/**
 * DNS-SD / mDNS discovery for anyMic servers advertising _anymic._udp.local.
 *
 * Uses Android's [NsdManager] to browse for services. For each found service,
 * resolves host + port + TXT then adds it to [servers] (deduplicated by fid+host:port).
 *
 * A multicast lock is acquired to prevent some Android ROMs (notably MIUI)
 * from filtering multicast traffic at the Wi-Fi driver layer.
 */
class Discovery(private val ctx: Context) {

    companion object {
        private const val TAG          = "Discovery"
        private const val SERVICE_TYPE = "_anymic._udp."   // Android NSD omits "local."
    }

    private val nsdManager = ctx.getSystemService(Context.NSD_SERVICE) as NsdManager

    private val _servers = MutableStateFlow<List<DiscoveredServer>>(emptyList())
    val servers: StateFlow<List<DiscoveredServer>> = _servers

    @Volatile private var discoveryListener: NsdManager.DiscoveryListener? = null
    @Volatile private var multicastLock: WifiManager.MulticastLock? = null

    fun start() {
        // Acquire multicast lock so the Wi-Fi driver forwards mDNS packets.
        acquireMulticastLock()

        val listener = object : NsdManager.DiscoveryListener {
            override fun onStartDiscoveryFailed(serviceType: String, errorCode: Int) {
                Log.e(TAG, "onStartDiscoveryFailed: $serviceType errorCode=$errorCode")
            }
            override fun onStopDiscoveryFailed(serviceType: String, errorCode: Int) {
                Log.e(TAG, "onStopDiscoveryFailed: $serviceType errorCode=$errorCode")
            }
            override fun onDiscoveryStarted(serviceType: String) {
                Log.i(TAG, "Discovery started for $serviceType")
            }
            override fun onDiscoveryStopped(serviceType: String) {
                Log.i(TAG, "Discovery stopped for $serviceType")
            }
            override fun onServiceFound(serviceInfo: NsdServiceInfo) {
                Log.d(TAG, "Service found: ${serviceInfo.serviceName}")
                resolveService(serviceInfo)
            }
            override fun onServiceLost(serviceInfo: NsdServiceInfo) {
                Log.d(TAG, "Service lost: ${serviceInfo.serviceName}")
                removeServer(serviceInfo)
            }
        }

        discoveryListener = listener
        nsdManager.discoverServices(SERVICE_TYPE, NsdManager.PROTOCOL_DNS_SD, listener)
    }

    fun stop() {
        val listener = discoveryListener ?: return
        discoveryListener = null
        try {
            nsdManager.stopServiceDiscovery(listener)
        } catch (e: Exception) {
            Log.w(TAG, "stopServiceDiscovery threw: ${e.message}")
        }
        releaseMulticastLock()
    }

    // -------------------------------------------------------------------------
    // Internal helpers
    // -------------------------------------------------------------------------

    private fun resolveService(serviceInfo: NsdServiceInfo) {
        nsdManager.resolveService(serviceInfo, object : NsdManager.ResolveListener {
            override fun onResolveFailed(serviceInfo: NsdServiceInfo, errorCode: Int) {
                Log.w(TAG, "Resolve failed for ${serviceInfo.serviceName} errorCode=$errorCode")
            }
            override fun onServiceResolved(resolved: NsdServiceInfo) {
                Log.d(TAG, "Resolved: ${resolved.serviceName} host=${resolved.host} port=${resolved.port}")

                val txt = parseTxt(resolved)
                val host = resolved.host?.hostAddress ?: return

                // Port from SRV record is the UDP data port (50127).
                // TCP control port is in TXT "ctl" field.
                val dataPort = resolved.port
                val controlPort = txt["ctl"]?.toIntOrNull() ?: 50128

                val server = DiscoveredServer(
                    name           = resolved.serviceName,
                    host           = host,
                    dataPort       = dataPort,
                    controlPort    = controlPort,
                    txt            = txt,
                    nsdServiceInfo = resolved,
                )

                addServer(server)
            }
        })
    }

    /** Parse the raw TXT attribute map from NsdServiceInfo into String→String. */
    private fun parseTxt(info: NsdServiceInfo): Map<String, String> {
        val result = mutableMapOf<String, String>()
        try {
            @Suppress("DEPRECATION")
            val attrs = info.attributes
            attrs?.forEach { (key, valueBytes) ->
                if (valueBytes != null) {
                    result[key] = String(valueBytes, Charsets.UTF_8)
                }
            }
        } catch (e: Exception) {
            Log.w(TAG, "TXT parse error: ${e.message}")
        }
        return result
    }

    private fun addServer(server: DiscoveredServer) {
        val key = dedupKey(server)
        val current = _servers.value.toMutableList()
        if (current.none { dedupKey(it) == key }) {
            current.add(server)
            _servers.value = current
            Log.i(TAG, "Server added: ${server.name} @ ${server.host}:${server.dataPort}")
        }
    }

    private fun removeServer(info: NsdServiceInfo) {
        _servers.value = _servers.value.filterNot { it.name == info.serviceName }
    }

    private fun dedupKey(s: DiscoveredServer): String =
        "${s.txt["fid"] ?: s.name}@${s.host}:${s.dataPort}"

    private fun acquireMulticastLock() {
        val wifi = ctx.applicationContext.getSystemService(Context.WIFI_SERVICE) as? WifiManager
        multicastLock = wifi?.createMulticastLock("anymic")?.also { lock ->
            lock.setReferenceCounted(true)
            lock.acquire()
            Log.i(TAG, "Multicast lock acquired")
        }
    }

    private fun releaseMulticastLock() {
        multicastLock?.let { lock ->
            if (lock.isHeld) {
                lock.release()
                Log.i(TAG, "Multicast lock released")
            }
        }
        multicastLock = null
    }
}

/** Information about a discovered anyMic server. */
data class DiscoveredServer(
    val name: String,
    val host: String,
    val dataPort: Int,
    val controlPort: Int,
    val txt: Map<String, String>,
    val nsdServiceInfo: NsdServiceInfo,
) {
    val codec: String?           get() = txt["codec"]
    val protocolVersion: String? get() = txt["v"]
    val fingerprint: String?     get() = txt["fid"]
}
