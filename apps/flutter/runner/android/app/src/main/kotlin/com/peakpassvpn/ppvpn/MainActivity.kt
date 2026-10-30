package com.peakpassvpn.ppvpn

import android.content.ActivityNotFoundException
import android.content.Intent
import android.net.Uri
import android.os.Bundle
import io.flutter.embedding.android.FlutterActivity
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.plugin.common.MethodChannel

// The app's one activity (#448), in the Play package (ci-build.sh). Its
// channel tells main.dart the app's own directory: the client's data and
// logs go under it, and the tunnel's socket lives there; and it opens links
// for lib/links.dart (#473). The system VPN's channels are VpnPlugin's.
class MainActivity : FlutterActivity() {
    private var vpn: VpnPlugin? = null

    override fun configureFlutterEngine(flutterEngine: FlutterEngine) {
        super.configureFlutterEngine(flutterEngine)
        MethodChannel(flutterEngine.dartExecutor.binaryMessenger, "com.peakpassvpn.ppvpn/app")
            .setMethodCallHandler { call, result ->
                when (call.method) {
                    "filesDir" -> result.success(filesDir.absolutePath)
                    "openUrl" -> result.success(open(call.arguments as? String))
                    else -> result.notImplemented()
                }
            }
        vpn = VpnPlugin(this, flutterEngine.dartExecutor.binaryMessenger)
    }

    // A Custom Tab where the browser offers them, else the browser: the
    // Custom Tabs protocol is this extra on the view intent, so no library.
    private fun open(url: String?): Boolean {
        val uri = url?.let(Uri::parse) ?: return false
        val intent = Intent(Intent.ACTION_VIEW, uri)
        if (uri.scheme == "http" || uri.scheme == "https") {
            intent.putExtras(Bundle().apply { putBinder(CUSTOM_TABS_SESSION, null) })
        }
        return try {
            startActivity(intent)
            true
        } catch (e: ActivityNotFoundException) {
            false
        }
    }

    override fun cleanUpFlutterEngine(flutterEngine: FlutterEngine) {
        vpn?.close()
        vpn = null
        super.cleanUpFlutterEngine(flutterEngine)
    }

    private companion object {
        const val CUSTOM_TABS_SESSION = "android.support.customtabs.extra.SESSION"
    }

    @Deprecated("Deprecated in Java")
    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        if (vpn?.onActivityResult(requestCode, resultCode) != true) {
            super.onActivityResult(requestCode, resultCode, data)
        }
    }
}
