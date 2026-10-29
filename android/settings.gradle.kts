// Beam Android —— 多模块 Gradle 工程根设置。
//
// 模块清单严格对应 docs/beam-tech-design.md §3.12 的「Android 模块」表,共 12 个:
//   :app              导航、主题、权限、TV 入口
//   :feature-discovery 主机列表、NSD 兜底
//   :feature-pairing  CameraX + ML Kit 扫码、PIN 键盘
//   :feature-stream   SurfaceView、悬浮工具栏、统计浮层
//   :feature-settings 设置页
//   :feature-perf     性能面板(仅 debugImplementation)
//   :core-decoder     MediaCodec 异步、KEY_LOW_LATENCY、厂商 low_latency 解码器、output fence
//   :core-renderer    Surface 管理、Surface.setFrameRate、帧节奏
//   :core-input       触控/手柄/遥控映射为 InputEvent
//   :core-audio       Opus 解码 + AAudio
//   :core-data        DataStore 设置、信任主机 HostRepo
//   :core-net         uniffi 绑定 BeamClient、回调转 Flow
pluginManagement {
    repositories {
        google {
            content {
                includeGroupByRegex("com\\.android.*")
                includeGroupByRegex("com\\.google.*")
                includeGroupByRegex("androidx.*")
            }
        }
        // Kotlin / AGP 的 plugin marker 均在 Maven Central 与 Google Maven 上;
        // 本机 services.gradle.org 链路不可用,故不依赖 gradlePluginPortal()。
        mavenCentral()
    }
}

dependencyResolutionManagement {
    repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS)
    repositories {
        google()
        mavenCentral()
    }
}

rootProject.name = "Beam"

include(":app")
include(":feature-discovery")
include(":feature-pairing")
include(":feature-stream")
include(":feature-settings")
include(":feature-perf")
include(":core-decoder")
include(":core-renderer")
include(":core-input")
include(":core-audio")
include(":core-data")
include(":core-net")
