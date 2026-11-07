// Test boundary only: the real TunnelService worker/lifecycle is compiled,
// but no JNI library, TUN, routing or network engine is loaded.
package com.peakpassvpn.ppvpn.tunnel

object TunnelHost {
    interface Listener {
        fun protect(fd: Int): Boolean
        fun onExit(reason: String)
    }

    @Volatile var stops = 0
    @Volatile var settingsCalls = 0
    @Volatile var networkCalls = 0
    fun settings(): String? { settingsCalls += 1; return null }
    fun start(fd: Int, socket: String, state: String, options: String?, listener: Listener): String? = null
    fun stop() { stops += 1 }
    fun networkChanged(state: String): String? { networkCalls += 1; return null }
}
