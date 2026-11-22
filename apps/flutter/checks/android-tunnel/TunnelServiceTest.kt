package com.peakpassvpn.ppvpn

import android.content.Intent
import com.peakpassvpn.ppvpn.tunnel.TunnelHost
import java.util.concurrent.CountDownLatch
import java.util.concurrent.ExecutorService
import java.util.concurrent.TimeUnit
import org.junit.Assert.assertEquals
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.Robolectric
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [26, 35])
class TunnelServiceTest {
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
