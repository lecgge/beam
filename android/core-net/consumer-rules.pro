# 保留 uniffi 生成的 Rust 侧数据类与 JNI 回调方法名，
# 避免 R8 混淆导致 Rust 侧反射调用失败。
-keep class dev.beam.ffi.** { *; }
-keepclassmembers class dev.beam.ffi.** {
    native <methods>;
}
-keepclassmembers class * implements dev.beam.ffi.** {
    native <methods>;
}
