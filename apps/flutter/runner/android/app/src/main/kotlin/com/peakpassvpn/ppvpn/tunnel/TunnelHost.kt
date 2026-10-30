package com.peakpassvpn.ppvpn.tunnel

// The engine in the tunnel process (#448): libppvpn_tunnel.so
// (crates/tunnel-host, src/android.rs), for TunnelService only, in the
// :tunnel process. The functions are the C ones of
// crates/tunnel-host/include/ppvpn_tunnel.h; an error comes back as its
// JSON ({"code", "message", "retryable"}), null meaning success.
//
// The UI process talks Core API v1 to the engine over socketPath
// (docs/mobile.md); one host per process.
object TunnelHost {
    init {
        System.loadLibrary("ppvpn_tunnel")
    }

    /** Called on the host's own threads. */
    interface Listener {
        /** VpnService.protect: the engine's socket leaves outside the tunnel. */
        fun protect(fd: Int): Boolean

        /**
         * The host gave up on the tunnel ({"code", "message"} as JSON), at
         * most once per start: stop the host and the service.
         */
        fun onExit(reason: String)
    }

    /** The TUN to build (ppvpn_core::TunSettings as JSON). */
    @JvmStatic external fun settings(): String?

    /**
     * Starts the host on the TUN's fd (the engine takes a duplicate; the
     * caller keeps its own), serving socketPath; null once it serves.
     * options: JSON, {"max_connections"?, "log_level"?: "info" | "debug"}.
     */
    @JvmStatic external fun start(
        tunFd: Int,
        socketPath: String,
        stateDir: String,
        options: String?,
        listener: Listener,
    ): String?

    @JvmStatic external fun stop()

    /** ppvpn_core::NetworkState as JSON; null once taken. */
    @JvmStatic external fun networkChanged(networkState: String): String?

    @JvmStatic external fun sleep()

    @JvmStatic external fun wake()

    @JvmStatic external fun version(): String
}
