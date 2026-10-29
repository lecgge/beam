# Beam Android release 混淆规则
#
# §3.13 / §7：release 构建必须可执行校验「无 perf 符号」。
# 本工程通过「:feature-perf 仅 debugImplementation」实现，因此 release 不需要
# 任何保留 perf 类的规则；这里显式注释掉常见误区，避免有人误加 keep。

# --- Kotlin / 协程 ---
-dontwarn kotlinx.coroutines.**
-keepclassmembers class kotlinx.coroutines.** { volatile <fields>; }

# --- uniffi 生成的 Kotlin 绑定（core-net）---
# uniffi 生成的类通过反射/接口实现被 Rust 侧回调，需要保留其方法签名。
-keep class dev.beam.core.net.uniffi.** { *; }
-keepclassmembers class dev.beam.core.net.uniffi.** { *; }
-keep interface dev.beam.core.net.uniffi.** { *; }
# 回调接口的实现由 Kotlin 侧提供，Rust 侧通过 JNI 反射调用。
-keep class * implements dev.beam.core.net.FrameSink { *; }

# --- CameraX / ML Kit ---
-dontwarn com.google.mlkit.**
-dontwarn com.google.android.gms.**
-keep class com.google.mlkit.** { *; }
-keep class com.google.android.gms.internal.mlkit_** { *; }
