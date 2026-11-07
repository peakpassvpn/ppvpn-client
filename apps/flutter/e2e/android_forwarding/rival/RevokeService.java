package com.peakpassvpn.ppvpn.acceptance.revoke;

import android.content.Intent;
import android.net.VpnService;
import android.os.ParcelFileDescriptor;
import java.io.FileOutputStream;
import java.io.IOException;

public final class RevokeService extends VpnService {
    private ParcelFileDescriptor tun;
    @Override public int onStartCommand(Intent intent, int flags, int startId) {
        if (tun == null) {
            tun = new Builder().setSession("PPVPN acceptance rival")
                .addAddress("10.61.160.1", 32)
                .addRoute("192.0.2.10", 32).establish();
            if (tun == null) throw new IllegalStateException("Fixture establish refused");
            try (FileOutputStream file = openFileOutput("established", MODE_PRIVATE)) {
                file.write("established".getBytes(java.nio.charset.StandardCharsets.US_ASCII));
            } catch (IOException error) { throw new IllegalStateException(error); }
        }
        return START_NOT_STICKY;
    }
    @Override public void onDestroy() {
        if (tun != null) {
            try { tun.close(); } catch (IOException ignored) { }
            tun = null;
        }
        super.onDestroy();
    }
}
