// 根构建脚本:只声明插件版本,各模块自行 apply。
// 版本集中在 gradle/libs.versions.toml(版本目录)。
//
// 版本对齐(见 docs/beam-tech-design.md §3.12 与 §7):
//   Gradle 8.13 / AGP 8.11.1 / Kotlin 2.1.21 / compileSdk 36 / buildTools 36.0.0
//   minSdk 26 / targetSdk 36 / Compose BOM 2025.06.00
plugins {
    alias(libs.plugins.android.application) apply false
    alias(libs.plugins.android.library) apply false
    alias(libs.plugins.kotlin.android) apply false
    alias(libs.plugins.kotlin.compose) apply false
}
