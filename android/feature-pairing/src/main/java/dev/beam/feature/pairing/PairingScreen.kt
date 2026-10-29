package dev.beam.feature.pairing

import android.annotation.SuppressLint
import androidx.camera.core.CameraSelector
import androidx.camera.core.ImageAnalysis
import androidx.camera.core.Preview
import androidx.camera.lifecycle.ProcessCameraProvider
import androidx.camera.view.PreviewView
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalLifecycleOwner
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.core.content.ContextCompat
import com.google.mlkit.vision.barcode.BarcodeScanning
import com.google.mlkit.vision.common.InputImage
import dev.beam.core.data.ui.BeamTokens
import java.util.concurrent.Executors

/**
 * 配对页（原型「Android 端 → 配对」）。
 *
 * 两条路径：
 * - 扫码（CameraX + ML Kit）；
 * - PIN 键盘（4–6 位）。
 *
 * UI 只负责展示与回调；配对逻辑在 [PairingViewModel]。
 */
@Composable
fun PairingScreen(
    state: PairingUiState,
    onDigit: (Int) -> Unit,
    onBackspace: () -> Unit,
    onSubmit: () -> Unit,
    onQrScanned: (String) -> Unit,
    onBack: () -> Unit,
    modifier: Modifier = Modifier,
) {
    Column(
        modifier = modifier
            .fillMaxSize()
            .background(MaterialTheme.colorScheme.background)
            .padding(BeamTokens.SpacingXl),
    ) {
        TextButton(onClick = onBack) { Text("← 返回") }
        Text(
            text = state.peer?.name?.let { "配对 $it" } ?: "配对主机",
            style = MaterialTheme.typography.titleLarge,
            fontWeight = FontWeight.SemiBold,
            color = MaterialTheme.colorScheme.onBackground,
        )
        Spacer(Modifier.height(BeamTokens.SpacingMd))

        when (state.phase) {
            PairingPhase.IDLE, PairingPhase.SCANNING -> {
                CameraPreview(onQrDetected = onQrScanned)
                Spacer(Modifier.height(BeamTokens.SpacingMd))
                Text(
                    text = "对准主机上的二维码，或输入 PIN",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.fillMaxWidth(),
                    textAlign = TextAlign.Center,
                )
                Spacer(Modifier.height(BeamTokens.SpacingMd))
                PinKeypad(
                    pin = state.pin,
                    pinLength = state.pinLength,
                    enabled = state.phase != PairingPhase.PAIRING,
                    onDigit = onDigit,
                    onBackspace = onBackspace,
                    onSubmit = onSubmit,
                )
            }
            PairingPhase.PIN_ENTRY -> {
                PinKeypad(
                    pin = state.pin,
                    pinLength = state.pinLength,
                    enabled = true,
                    onDigit = onDigit,
                    onBackspace = onBackspace,
                    onSubmit = onSubmit,
                )
            }
            PairingPhase.PAIRING -> {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    CircularProgressIndicator(
                        modifier = Modifier.width(16.dp).height(16.dp),
                        strokeWidth = 2.dp,
                    )
                    Spacer(Modifier.width(BeamTokens.SpacingSm))
                    Text(
                        text = "正在与主机配对…",
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }
            PairingPhase.SUCCESS -> {
                NoticeCard("配对成功，可以开始串流了", BeamTokens.Success)
            }
            PairingPhase.FAILED -> {
                NoticeCard(state.error ?: "配对失败", BeamTokens.Error)
            }
        }
    }
}

@Composable
private fun PinKeypad(
    pin: String,
    pinLength: Int,
    enabled: Boolean,
    onDigit: (Int) -> Unit,
    onBackspace: () -> Unit,
    onSubmit: () -> Unit,
) {
    Column(modifier = Modifier.fillMaxWidth()) {
        PinSlots(pin = pin, pinLength = pinLength)
        Spacer(Modifier.height(BeamTokens.SpacingMd))
        val keys = listOf(1, 2, 3, 4, 5, 6, 7, 8, 9, -1, 0, -2)
        keys.chunked(3).forEach { row ->
            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.spacedBy(BeamTokens.SpacingSm),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                row.forEach { key ->
                    when (key) {
                        -1 -> KeyButton(text = "⌫", enabled = enabled, onClick = onBackspace, modifier = Modifier.weight(1f))
                        -2 -> KeyButton(text = "✓", enabled = enabled && pin.length == pinLength, onClick = onSubmit, modifier = Modifier.weight(1f))
                        else -> KeyButton(text = key.toString(), enabled = enabled, onClick = { onDigit(key) }, modifier = Modifier.weight(1f))
                    }
                }
            }
            Spacer(Modifier.height(BeamTokens.SpacingSm))
        }
    }
}

@Composable
private fun PinSlots(pin: String, pinLength: Int) {
    Row(
        modifier = Modifier.fillMaxWidth(),
        horizontalArrangement = Arrangement.Center,
    ) {
        repeat(pinLength) { index ->
            val filled = index < pin.length
            Box(
                modifier = Modifier
                    .padding(horizontal = BeamTokens.SpacingXs)
                    .width(44.dp)
                    .height(56.dp)
                    .border(
                        width = 1.dp,
                        color = if (filled) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.outline,
                        shape = RoundedCornerShape(BeamTokens.RadiusButton),
                    ),
                contentAlignment = Alignment.Center,
            ) {
                Text(
                    text = if (filled) "•" else "",
                    style = MaterialTheme.typography.titleLarge,
                    color = MaterialTheme.colorScheme.onSurface,
                )
            }
        }
    }
}

@Composable
private fun KeyButton(
    text: String,
    enabled: Boolean,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
) {
    Button(
        onClick = onClick,
        enabled = enabled,
        modifier = modifier.height(48.dp),
        shape = RoundedCornerShape(BeamTokens.RadiusButton),
    ) {
        Text(text = text, style = MaterialTheme.typography.titleMedium)
    }
}

@Composable
private fun NoticeCard(text: String, accent: Color) {
    Surface(
        modifier = Modifier.fillMaxWidth(),
        shape = RoundedCornerShape(BeamTokens.RadiusCard),
        color = MaterialTheme.colorScheme.surface,
        border = androidx.compose.foundation.BorderStroke(1.dp, accent),
    ) {
        Text(
            text = text,
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurface,
            modifier = Modifier.padding(BeamTokens.SpacingLg),
        )
    }
}

/**
 * 相机预览 + ML Kit 条码识别。
 *
 * 识别到 `beam://pair?` 开头的载荷才回调，避免扫到无关二维码。
 */
@SuppressLint("UnsafeOptInUsageError")
@Composable
private fun CameraPreview(onQrDetected: (String) -> Unit) {
    val context = LocalContext.current
    val lifecycleOwner = LocalLifecycleOwner.current
    val executor = remember { Executors.newSingleThreadExecutor() }
    val scanner = remember { BarcodeScanning.getClient() }
    var detected by remember { mutableStateOf<String?>(null) }

    DisposableEffect(Unit) {
        onDispose {
            executor.shutdown()
            runCatching { scanner.close() }
        }
    }

    Box(
        modifier = Modifier
            .fillMaxWidth()
            .aspectRatio(4f / 3f)
            .background(Color(0xFF14204A), RoundedCornerShape(BeamTokens.RadiusCard)),
        contentAlignment = Alignment.Center,
    ) {
        AndroidView(
            modifier = Modifier.fillMaxSize(),
            factory = { ctx ->
                PreviewView(ctx).apply {
                    scaleType = PreviewView.ScaleType.FILL_CENTER
                }
            },
            update = { previewView ->
                val providerFuture = ProcessCameraProvider.getInstance(context)
                providerFuture.addListener({
                    val provider = runCatching { providerFuture.get() }.getOrNull() ?: return@addListener
                    val preview = Preview.Builder().build().also {
                        it.setSurfaceProvider(previewView.surfaceProvider)
                    }
                    val analysis = ImageAnalysis.Builder()
                        .setBackpressureStrategy(ImageAnalysis.STRATEGY_KEEP_ONLY_LATEST)
                        .build()
                    analysis.setAnalyzer(executor) { imageProxy ->
                        val mediaImage = imageProxy.image
                        if (mediaImage == null) {
                            imageProxy.close()
                            return@setAnalyzer
                        }
                        val image = InputImage.fromMediaImage(mediaImage, imageProxy.imageInfo.rotationDegrees)
                        scanner.process(image)
                            .addOnSuccessListener { barcodes ->
                                val value = barcodes.firstOrNull()?.rawValue
                                if (value != null && value.startsWith("beam://pair") && detected == null) {
                                    detected = value
                                    onQrDetected(value)
                                }
                            }
                            .addOnCompleteListener { imageProxy.close() }
                    }
                    runCatching {
                        provider.unbindAll()
                        provider.bindToLifecycle(
                            lifecycleOwner,
                            CameraSelector.DEFAULT_BACK_CAMERA,
                            preview,
                            analysis,
                        )
                    }
                }, ContextCompat.getMainExecutor(context))
            },
        )
        Box(
            modifier = Modifier
                .width(110.dp)
                .height(110.dp)
                .border(
                    width = 3.dp,
                    color = Color(0xFF7AA2FF),
                    shape = RoundedCornerShape(10.dp),
                ),
        )
    }
}

/** 供预览/测试使用的空实现。 */
@Composable
fun PairingPreviewPlaceholder() {
    OutlinedButton(onClick = {}) { Text("配对") }
}
