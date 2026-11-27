// Test boundary only: the real TunnelService worker/lifecycle is compiled,
// but no JNI library, TUN, routing or network engine is loaded.
package com.peakpassvpn.ppvpn.tunnel

object TunnelHost {
    interface Listener {
        fun protect(fd: Int): Boolean
        fun onExit(reason: String)
    }

    @Volatile var starts = 0
    @Volatile var stops = 0
    @Volatile var lastSocketPath: String? = null
    @Volatile var lastStateDir: String? = null
    fun settings(): String? = null
    fun start(fd: Int, socket: String, state: String, options: String?, listener: Listener): String? {
        starts += 1
        lastSocketPath = socket
        lastStateDir = state
        return null
    }
    fun stop() { stops += 1 }
    fun networkChanged(state: String): String? = null
}
