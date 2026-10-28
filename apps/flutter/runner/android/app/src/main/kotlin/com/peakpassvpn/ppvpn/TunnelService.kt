package com.peakpassvpn.ppvpn

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.net.VpnService
import android.os.Build

// The system VPN (#448), in its own process (:tunnel, ci-build.sh's
// manifest entry): started and stopped by VpnPlugin, its state broadcast
// within the package for the app's process.
//
// A shell for now: the tunnel host (#448 step 2: its JNI, the TUN built
// from tun_settings, tunnel.sock served from the app's files directory)
// is not in it yet, so start stays at "connecting" and builds no TUN. The
// client gives up after its wait for the host (TUNNEL_NOT_READY) and the
// app stops the service again. A start from the system (always-on, no
// action) is not served yet either.
class TunnelService : VpnService() {
    private var state = STATE_OFF

    private val query = object : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) = publish()
    }

    override fun onCreate() {
        super.onCreate()
        val filter = IntentFilter(ACTION_QUERY)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            registerReceiver(query, filter, Context.RECEIVER_NOT_EXPORTED)
        } else {
            registerReceiver(query, filter)
        }
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_START -> set(STATE_CONNECTING)
            ACTION_STOP -> {
                if (state != STATE_OFF) set(STATE_DISCONNECTING)
                stopSelf()
            }
            else -> stopSelf()
        }
        return START_NOT_STICKY
    }

    /** Another VPN app took over, or the user revoked the permission. */
    override fun onRevoke() {
        stopSelf()
    }

    override fun onDestroy() {
        unregisterReceiver(query)
        set(STATE_OFF)
        super.onDestroy()
    }

    private fun set(next: String) {
        state = next
        publish()
    }

    private fun publish() {
        sendBroadcast(Intent(ACTION_STATE).setPackage(packageName).putExtra(EXTRA_STATE, state))
    }

    companion object {
        const val ACTION_START = "com.peakpassvpn.ppvpn.vpn.START"
        const val ACTION_STOP = "com.peakpassvpn.ppvpn.vpn.STOP"
        const val ACTION_STATE = "com.peakpassvpn.ppvpn.vpn.STATE"
        const val ACTION_QUERY = "com.peakpassvpn.ppvpn.vpn.QUERY"
        const val EXTRA_STATE = "state"
        const val STATE_OFF = "off"
        const val STATE_CONNECTING = "connecting"
        const val STATE_ON = "on"
        const val STATE_DISCONNECTING = "disconnecting"

        fun intent(context: Context, action: String): Intent =
            Intent(context, TunnelService::class.java).setAction(action)
    }
}
