package dev.beam.app.stream

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import dev.beam.app.MainActivity
import dev.beam.app.R

/**
 * 串流前台服务（AN-52 / §3.12 `:app` 职责「权限」）。
 *
 * ## 生命周期
 *
 * - **起**：串流开始（会话进入 `STREAMING`）时由 `:app` 调用 [start]；
 * - **停**：串流结束 / 断开 / 出错 / 离开串流页时由 `:app` 调用 [stop]。
 *
 * ## 权限
 *
 * - `FOREGROUND_SERVICE` / `FOREGROUND_SERVICE_MEDIA_PLAYBACK`：manifest 声明；
 * - `POST_NOTIFICATIONS`（Android 13+ 运行时权限）：用户拒绝时**服务照常运行**，
 *   只是通知不可见 —— Android 的既定行为，这里如实说明，不做任何绕过。
 *
 * ## 通知
 *
 * 常驻通知：标题「Beam 正在串流」，内容是当前连接的主机名，点击回到串流页。
 */
class StreamForegroundService : Service() {

    override fun onCreate() {
        super.onCreate()
        ensureNotificationChannel(this)
    }

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_STOP -> {
                stopSelf()
                return START_NOT_STICKY
            }
            else -> {
                val host = intent?.getStringExtra(EXTRA_HOST).orEmpty()
                startForegroundCompat(buildNotification(host))
            }
        }
        return START_STICKY
    }

    override fun onDestroy() {
        stopForegroundCompat()
        super.onDestroy()
    }

    // ------------------------------------------------------------- 内部实现

    private fun startForegroundCompat(notification: Notification) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            startForeground(
                NOTIFICATION_ID,
                notification,
                ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PLAYBACK,
            )
        } else {
            startForeground(NOTIFICATION_ID, notification)
        }
    }

    private fun stopForegroundCompat() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) {
            stopForeground(STOP_FOREGROUND_REMOVE)
        } else {
            @Suppress("DEPRECATION")
            stopForeground(true)
        }
    }

    private fun buildNotification(host: String): Notification {
        val pendingIntent = PendingIntent.getActivity(
            this,
            0,
            Intent(this, MainActivity::class.java).apply {
                flags = Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_CLEAR_TOP
            },
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )

        val builder = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            Notification.Builder(this, CHANNEL_ID)
        } else {
            @Suppress("DEPRECATION")
            Notification.Builder(this)
        }

        return builder
            .setContentTitle("Beam 正在串流")
            .setContentText(if (host.isBlank()) "串流进行中" else "已连接到 $host")
            .setSmallIcon(R.drawable.ic_launcher_foreground)
            .setContentIntent(pendingIntent)
            .setOngoing(true)
            .setCategory(Notification.CATEGORY_SERVICE)
            .build()
    }

    companion object {

        /** 启动动作。 */
        const val ACTION_START: String = "dev.beam.app.stream.action.START"

        /** 停止动作。 */
        const val ACTION_STOP: String = "dev.beam.app.stream.action.STOP"

        /** 附加信息：主机名。 */
        const val EXTRA_HOST: String = "host"

        /** 通知渠道 ID。 */
        const val CHANNEL_ID: String = "beam_stream"

        /** 通知 ID。 */
        const val NOTIFICATION_ID: Int = 0x4245

        /**
         * 启动前台服务（串流开始时调用）。
         *
         * @param host 当前连接的主机名（显示在通知里）。
         */
        fun start(context: Context, host: String) {
            val intent = Intent(context, StreamForegroundService::class.java).apply {
                action = ACTION_START
                putExtra(EXTRA_HOST, host)
            }
            runCatching { context.startForegroundService(intent) }
        }

        /**
         * 停止前台服务（串流结束时调用）。
         */
        fun stop(context: Context) {
            val intent = Intent(context, StreamForegroundService::class.java).apply {
                action = ACTION_STOP
            }
            runCatching { context.stopService(intent) }
        }

        /**
         * 创建通知渠道（Android 8+ 必需）。
         */
        fun ensureNotificationChannel(context: Context) {
            if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
            val manager = context.getSystemService(Context.NOTIFICATION_SERVICE) as? NotificationManager
                ?: return
            if (manager.getNotificationChannel(CHANNEL_ID) != null) return
            val channel = NotificationChannel(
                CHANNEL_ID,
                "串流状态",
                NotificationManager.IMPORTANCE_LOW,
            ).apply {
                description = "串流期间的前台通知"
                setShowBadge(false)
            }
            manager.createNotificationChannel(channel)
        }
    }
}
