package dev.beam.app

import android.Manifest
import android.content.pm.PackageManager
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.remember
import androidx.compose.ui.platform.LocalContext
import androidx.core.content.ContextCompat

/**
 * 权限清单（按需声明，不做一次性全量申请）。
 *
 * | 权限 | 何时申请 |
 * |---|---|
 * | `CAMERA` | 进入配对页扫码时 |
 * | `POST_NOTIFICATIONS` | 首次进入串流页（串流状态通知） |
 * | `FOREGROUND_SERVICE` / `FOREGROUND_SERVICE_MEDIA_PLAYBACK` | 声明式，随串流前台服务使用 |
 * | `NEARBY_WIFI_DEVICES` / `ACCESS_WIFI_STATE` / `CHANGE_WIFI_MULTICAST_STATE` | 局域网发现（mDNS 组播） |
 */
object BeamPermissions {

    /** 相机（扫码）。 */
    const val CAMERA: String = Manifest.permission.CAMERA

    /** 通知（Android 13+ 运行时权限）。 */
    const val POST_NOTIFICATIONS: String = Manifest.permission.POST_NOTIFICATIONS

    /** 前台服务。 */
    const val FOREGROUND_SERVICE: String = Manifest.permission.FOREGROUND_SERVICE

    /** 前台服务（媒体播放）。 */
    const val FOREGROUND_SERVICE_MEDIA_PLAYBACK: String = Manifest.permission.FOREGROUND_SERVICE_MEDIA_PLAYBACK

    /** 附近的 Wi-Fi 设备（Android 13+ 局域网发现）。 */
    const val NEARBY_WIFI_DEVICES: String = Manifest.permission.NEARBY_WIFI_DEVICES

    /** 组播锁（mDNS 发现需要）。 */
    const val CHANGE_WIFI_MULTICAST_STATE: String = Manifest.permission.CHANGE_WIFI_MULTICAST_STATE
}

/**
 * 记住一个「按需申请权限」的触发器。
 *
 * 已授权时什么都不做；未授权时弹系统授权框。
 */
@Composable
fun rememberPermissionRequest(permission: String): () -> Unit {
    val context = LocalContext.current
    val launcher = rememberLauncherForActivityResult(
        contract = ActivityResultContracts.RequestPermission(),
    ) { /* 结果由调用方按需处理 */ }

    return remember(permission, launcher) {
        {
            val granted = ContextCompat.checkSelfPermission(context, permission) ==
                PackageManager.PERMISSION_GRANTED
            if (!granted) launcher.launch(permission)
        }
    }
}

/**
 * 首次进入应用时申请通知权限。
 */
@Composable
fun RequestNotificationPermissionOnFirstLaunch() {
    val request = rememberPermissionRequest(BeamPermissions.POST_NOTIFICATIONS)
    LaunchedEffect(Unit) {
        request()
    }
}
