package com.peakpassvpn.ppvpn

import android.app.Notification
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import com.peakpassvpn.ppvpn.tunnel.TunnelHost
import java.util.concurrent.CountDownLatch
import java.util.concurrent.ExecutorService
import java.util.concurrent.TimeUnit
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Assert.assertNull
import org.junit.Assert.assertNotNull
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.Robolectric
import org.robolectric.RobolectricTestRunner
import org.robolectric.Shadows.shadowOf
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
            setField(service, "ownsHost", true)
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
            check(worker.awaitTermination(5, TimeUnit.SECONDS))
        }
    }

    @Test fun foregroundPromotionPrecedesQueuedStartAndDestroyDoesNotBlockMain() {
        val controller = Robolectric.buildService(TunnelService::class.java).create()
        val service = controller.get()
        val worker = field(service, "worker") as ExecutorService
        val entered = CountDownLatch(1); val release = CountDownLatch(1)
        TunnelHost.settingsCalls = 0
        worker.execute { entered.countDown(); check(release.await(5, TimeUnit.SECONDS)) }
        check(entered.await(5, TimeUnit.SECONDS))
        try {
            service.onStartCommand(Intent().setAction(TunnelService.ACTION_START), 0, 20)
            val shadow = shadowOf(service)
            val notification = shadow.lastForegroundNotification
            assertNotNull(notification)
            assertTrue(notification!!.flags and Notification.FLAG_ONGOING_EVENT != 0)
            if (Build.VERSION.SDK_INT >= 34) {
                assertEquals(ServiceInfo.FOREGROUND_SERVICE_TYPE_SYSTEM_EXEMPTED, service.foregroundServiceType)
            }
            // Main returns while the worker is deliberately still blocked.
            controller.destroy()
            assertTrue(worker.isShutdown)
            assertEquals(true, field(service, "destroyed"))
        } finally {
            if (!worker.isShutdown) controller.destroy()
            release.countDown()
            check(worker.awaitTermination(5, TimeUnit.SECONDS))
        }
        assertEquals("destroyed service must not open a queued tunnel", 0, TunnelHost.settingsCalls)
        assertNull(shadowOf(service).lastForegroundNotification)
    }

    @Test fun foregroundRefusalCannotOpenATunnel() {
        val controller = Robolectric.buildService(TunnelService::class.java).create()
        val service = controller.get(); val worker = field(service, "worker") as ExecutorService
        TunnelHost.settingsCalls = 0
        try {
            shadowOf(service).setThrowInStartForeground(SecurityException("not eligible"))
            service.onStartCommand(Intent().setAction(TunnelService.ACTION_START), 0, 21)
            worker.submit {}.get(5, TimeUnit.SECONDS)
            assertEquals(0, TunnelHost.settingsCalls)
            assertEquals(TunnelService.STATE_OFF, field(service, "state"))
            assertTrue(shadowOf(service).isStoppedBySelf)
        } finally {
            controller.destroy(); check(worker.awaitTermination(5, TimeUnit.SECONDS))
        }
    }

    @Test fun repeatedDestroyedCleanupCannotStopANewerHostsGlobalSlot() {
        val controller = Robolectric.buildService(TunnelService::class.java).create()
        val service = controller.get(); val worker = field(service, "worker") as ExecutorService
        TunnelHost.stops = 0
        worker.submit {
            setField(service, "state", TunnelService.STATE_ON)
            setField(service, "ownsHost", true)
        }.get(5, TimeUnit.SECONDS)
        service.onStartCommand(Intent().setAction(TunnelService.ACTION_STOP), 0, 22)
        worker.submit {}.get(5, TimeUnit.SECONDS)
        assertEquals(1, TunnelHost.stops)
        controller.destroy(); check(worker.awaitTermination(5, TimeUnit.SECONDS))
        assertEquals("second cleanup no longer owns the native host", 1, TunnelHost.stops)
    }

    @Test fun closedPhysicalNetworkWatchCannotPublishIntoANewerHost() {
        val controller = Robolectric.buildService(TunnelService::class.java).create()
        val service = controller.get(); val worker = field(service, "worker") as ExecutorService
        val type = Class.forName("com.peakpassvpn.ppvpn.NetworkWatch")
        val constructor = type.getDeclaredConstructor(android.net.VpnService::class.java).apply { isAccessible = true }
        val watch = constructor.newInstance(service)
        val push = type.getDeclaredMethod("push").apply { isAccessible = true }
        val stop = type.getDeclaredMethod("stop").apply { isAccessible = true }
        TunnelHost.networkCalls = 0
        try {
            push.invoke(watch)
            assertEquals(1, TunnelHost.networkCalls)
            stop.invoke(watch)
            type.getDeclaredField("sent").apply { isAccessible = true }.set(watch, null)
            push.invoke(watch)
            assertEquals("closed watcher delivered a stale network", 1, TunnelHost.networkCalls)
        } finally {
            stop.invoke(watch)
            val thread = type.getDeclaredField("thread").apply { isAccessible = true }.get(watch) as Thread
            thread.join(5000)
            controller.destroy(); check(worker.awaitTermination(5, TimeUnit.SECONDS))
        }
    }

    private fun field(service: TunnelService, name: String): Any? =
        TunnelService::class.java.getDeclaredField(name).apply { isAccessible = true }.get(service)

    private fun setField(service: TunnelService, name: String, value: Any) {
        TunnelService::class.java.getDeclaredField(name).apply { isAccessible = true }.set(service, value)
    }
}
