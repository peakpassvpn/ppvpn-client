/*
 * ppvpn_tunnel.h: the tunnel host's C ABI (#448). The native tunnel
 * (NEPacketTunnelProvider; Android reaches the same calls through JNI)
 * builds the tunnel and calls these functions. The UI process talks Core
 * API v1 to the host over the Unix socket (#448 comment 6393069163).
 *
 * One tunnel per process. Every function may be called from any thread;
 * start and stop are serialised inside.
 *
 * Strings are UTF-8 and NUL-terminated. A `char *` returned by a function
 * belongs to the caller, who frees it with ppvpn_tunnel_free_string.
 * "Error" is NULL on success, else
 * {"code": "...", "message": "...", "retryable": bool}.
 */
#ifndef PPVPN_TUNNEL_H
#define PPVPN_TUNNEL_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define PPVPN_TUNNEL_PLATFORM_IOS 1
#define PPVPN_TUNNEL_PLATFORM_ANDROID 2

/*
 * The tunnel to build before ppvpn_tunnel_start, as JSON:
 * {"mtu": 9000, "ipv4_address": "10.60.159.89/30",
 *  "ipv4_routes": ["0.0.0.0/0"], "dns_servers": ["10.60.159.90"]}
 * (ppvpn_core::tun_settings). NULL for an unknown platform.
 * iOS: NEIPv4Settings(addresses:subnetMasks:) from ipv4_address,
 * includedRoutes from ipv4_routes, NEDNSSettings(servers:) and the MTU.
 */
char *ppvpn_tunnel_settings(int32_t platform);

/*
 * Called once, on a host thread, when the host stops the tunnel by itself:
 * the engine failed and its recovery gave up. `reason` is JSON
 * {"code": "...", "message": "..."}, valid during the call only. The native
 * side then ends the tunnel (iOS: cancelTunnelWithError) and calls
 * ppvpn_tunnel_stop. Never called for a stop the native side asked for.
 */
typedef void (*ppvpn_tunnel_exit_fn)(void *context, const char *reason);

/*
 * Starts the host and returns once it serves the socket; the UI process
 * then sends apply-profile and start over it, as it does on desktop.
 * - tun_fd: the tunnel's fd (iOS: the utun fd of the packet flow). The
 *   host dup()s it for each engine start and never closes it: the caller
 *   closes it after ppvpn_tunnel_stop returns, if it owns it.
 * - socket_path: <App Group container>/tunnel.sock (Android:
 *   <filesDir>/tunnel.sock). Longer than 104 bytes: the host binds it
 *   relative to its directory. A stale file there is replaced.
 * - state_dir: a directory the host may write (rule-set cache, logs);
 *   created if missing. iOS: inside the App Group container.
 * - options: JSON or NULL. Fields (all optional):
 *     "max_connections": 0 means unlimited; default 128 on iOS (#388),
 *         512 on Android;
 *     "log_level": "info" | "debug" (one line per connection and DNS
 *         query, domains included: diagnosis only), default "info".
 * - on_exit, context: see ppvpn_tunnel_exit_fn; on_exit may be NULL.
 * Error codes: TUNNEL_ALREADY_RUNNING, REQUEST_INVALID (an argument),
 * TUNNEL_SOCKET_FAILED, and the engine's CORE_* codes.
 */
char *ppvpn_tunnel_start(int32_t tun_fd, const char *socket_path, const char *state_dir,
                         int32_t platform, const char *options,
                         ppvpn_tunnel_exit_fn on_exit, void *context);

/*
 * Stops the engine (routes are the system's: nothing to undo), closes the
 * socket and its watch connections, removes the socket file. Blocks until
 * done (a few seconds at most). Without a running host: nothing.
 */
void ppvpn_tunnel_stop(void);

/*
 * The network the device is on, from NWPathMonitor (Android:
 * ConnectivityManager), before start too: JSON
 * {"interface": "en0", "type": "wifi" | "cellular" | "ethernet" | "other",
 *  "addresses": ["192.168.1.20"], "gateway": "192.168.1.1",
 *  "expensive": false, "constrained": false}
 * or {} when offline (ppvpn_core::NetworkState; absent fields default).
 * Before start, the host keeps the last one and hands it to the engine at
 * start. Error: REQUEST_INVALID.
 */
char *ppvpn_tunnel_network_changed(const char *network_state);

/* NEPacketTunnelProvider sleep / wake. Without a running host: nothing. */
void ppvpn_tunnel_sleep(void);
void ppvpn_tunnel_wake(void);

/* The host's version, "x.y.z"; static, not to be freed. */
const char *ppvpn_tunnel_version(void);

void ppvpn_tunnel_free_string(char *s);

#ifdef __cplusplus
}
#endif

#endif /* PPVPN_TUNNEL_H */
