package com.peakpassvpn.ppvpn.acceptance.revoke;

import android.app.Activity;
import android.content.Intent;
import android.net.VpnService;
import android.os.Bundle;

// Test APK only, never copied into the product. The host grants its VPN app-op
// on a disposable emulator; establish() causes real system takeover/onRevoke.
public final class RevokeActivity extends Activity {
    @Override public void onCreate(Bundle state) {
        super.onCreate(state);
        if (VpnService.prepare(this) != null) {
            throw new IllegalStateException("Fixture VPN app-op not granted");
        }
        startService(new Intent(this, RevokeService.class));
    }
}
