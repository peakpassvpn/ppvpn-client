package com.peakpassvpn.ppvpn

import android.content.Intent
import android.os.ParcelFileDescriptor
import com.peakpassvpn.ppvpn.tunnel.TunnelHost
import java.io.File
import java.util.concurrent.CountDownLatch
import java.util.concurrent.ExecutorService
import java.util.concurrent.TimeUnit
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.Robolectric
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [26, 35])
class TunnelServiceTest {
    @Test fun startPlacesSocketInClientDataDirectoryWhenMissingOrExisting() {
        val controller = Robolectric.buildService(TunnelService::class.java).create()
        val service = controller.get()
        val socketFile = TunnelService::class.java.getDeclaredMethod("socketFile").apply { isAccessible = true }
        val startHost = TunnelService::class.java.getDeclaredMethod(
            "startHost",
            ParcelFileDescriptor::class.java,
            File::class.java,
        ).apply { isAccessible = true }
        val shutDown = TunnelService::class.java.getDeclaredMethod("shutDown").apply { isAccessible = true }
        TunnelHost.starts = 0
        TunnelHost.lastSocketPath = null
        TunnelHost.lastStateDir = null
        try {
            val dataDir = File(service.filesDir, "data")
            assertFalse(dataDir.exists())

            val firstPipe = ParcelFileDescriptor.createPipe()
            firstPipe[1].close()
            val firstSocket = socketFile.invoke(service) as File
            assertTrue("client data directory was not created", dataDir.isDirectory)
            assertNull(startHost.invoke(service, firstPipe[0], firstSocket))
            assertEquals(1, TunnelHost.starts)
            assertEquals(File(dataDir, "tunnel.sock").path, TunnelHost.lastSocketPath)
            assertEquals(File(service.filesDir, "tunnel-state").path, TunnelHost.lastStateDir)
            shutDown.invoke(service)

            // The UI process normally creates filesDir/data before starting the VPN;
            // starting again with the directory already present must succeed identically.
            assertTrue(dataDir.isDirectory)
            val secondPipe = ParcelFileDescriptor.createPipe()
            secondPipe[1].close()
            val secondSocket = socketFile.invoke(service) as File
            assertNull(startHost.invoke(service, secondPipe[0], secondSocket))
            assertEquals(2, TunnelHost.starts)
            assertEquals(File(dataDir, "tunnel.sock").path, TunnelHost.lastSocketPath)
            shutDown.invoke(service)
        } finally {
            controller.destroy()
        }
    }

    @Test fun queuedOldExitDoesNotStopANewerTunnel() {
        val controller = Robolectric.buildService(TunnelService::class.java).create()
        val service = controller.get()
        val worker = field(service, "worker") as ExecutorService
        fun drain() { worker.submit {}.get(5, TimeUnit.SECONDS) }
        fun active(generation: Long) {
            setField(service, "generation", generation)
            setField(service, "state", TunnelService.STATE_ON)
        }
        fun listener(generation: Long): TunnelHost.Listener {
            val method = TunnelService::class.java.getDeclaredMethod("listener", Long::class.javaPrimitiveType!!)
            method.isAccessible = true
            return method.invoke(service, generation) as TunnelHost.Listener
        }
        val entered = CountDownLatch(1)
        val release = CountDownLatch(1)
        try {
            TunnelHost.stops = 0
            worker.submit { active(1) }.get(5, TimeUnit.SECONDS)
            val old = listener(1)
            worker.execute {
                entered.countDown()
                check(release.await(5, TimeUnit.SECONDS))
            }
            check(entered.await(5, TimeUnit.SECONDS))
            // A user stop is ahead of a new start, but the old native
            // callback posts its stop after that new start was queued.
            service.onStartCommand(Intent().setAction(TunnelService.ACTION_STOP), 0, 10)
            worker.execute { active(3) }
            old.onExit("old host exit")
            release.countDown()
            drain()
            assertEquals("old exit must not stop the new run", 1, TunnelHost.stops)
            assertEquals(TunnelService.STATE_ON, field(service, "state"))

            listener(3).onExit("current host exit")
            drain()
            assertEquals("current exit must still stop its own run", 2, TunnelHost.stops)
            assertEquals(TunnelService.STATE_OFF, field(service, "state"))
        } finally {
            release.countDown()
            drain()
            controller.destroy()
        }
    }

    private fun field(service: TunnelService, name: String): Any? =
        TunnelService::class.java.getDeclaredField(name).apply { isAccessible = true }.get(service)

    private fun setField(service: TunnelService, name: String, value: Any) {
        TunnelService::class.java.getDeclaredField(name).apply { isAccessible = true }.set(service, value)
    }
}
