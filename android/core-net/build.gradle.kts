import org.jetbrains.kotlin.gradle.dsl.JvmTarget
import java.util.Properties

plugins {
    alias(libs.plugins.android.library)
    alias(libs.plugins.kotlin.android)
}

// =============================================================================
// uniffi 生成物与原生库（§3.12「uniffi 导出(beam-ffi)」落地）
// =============================================================================
//
// 两类产物都是**构建产物**，不入库（规则见 `android/.gitignore`）：
//
// | 产物 | 生成方式 | 落点 |
// |---|---|---|
// | `libbeam_ffi.so` | `cargo ndk -t arm64-v8a -t x86_64 build --release -p beam-ffi --features uniffi` | `src/main/jniLibs/{arm64-v8a,x86_64}/` |
// | uniffi Kotlin 绑定 | `uniffi-bindgen generate --library <so> --language kotlin` | `build/generated/uniffi/`（用 `sourceSets` 挂进编译） |
//
// ## 默认不参与构建
//
// 本机没有 cargo / NDK / uniffi-bindgen 时，`assembleDebug` **仍必须能通过**：
// 降级路径（`:core-net` 反射层 → `BeamNativeUnavailableException` →
// `UnavailableBeamClient` → App 仍可启动）不能被破坏。
// 因此这两个任务默认**不被**编译任务依赖，需要时用 `-Pbeam.native=true` 打开：
//
// ```powershell
// .\gradlew.bat -Pbeam.native=true :core-net:assembleDebug
// ```
//
// ## 需要的外部环境
//
// - `cargo` + `cargo-ndk`；
// - Android NDK（`ANDROID_NDK_HOME` 环境变量 / `-Pbeam.ndk.home=` / `local.properties` 的 `sdk.dir\ndk\<版本>`）；
// - `uniffi-bindgen`（PATH 上的 `uniffi-bindgen` 或 `-Pbeam.uniffi.bindgen=` 指定的可执行文件）。
//
// 编译 Rust 时若打开了可选的 `ffmpeg` feature，还需要 `FFMPEG_DIR` / `LIBCLANG_PATH`
// （`Exec` 默认继承当前进程的环境变量，无需在 Gradle 里重复配置）。

/** 是否把原生构建接进常规构建（默认关闭，见上）。 */
val beamNativeEnabled: Boolean = providers.gradleProperty("beam.native").orNull == "true"

/** 仓库根（`crates/` 在 `android/` 的上一级）。 */
val cargoWorkspace: File = rootProject.file("..")

/** 原生库输出目录（AGP 会把 `jniLibs` 打进 APK 的 `lib/<abi>/`）。 */
val jniLibsDir: File = layout.projectDirectory.dir("src/main/jniLibs").asFile

/** uniffi Kotlin 绑定的生成目录（构建目录，不入库）。 */
val uniffiGeneratedDir: Provider<Directory> = layout.buildDirectory.dir("generated/uniffi")

/** 手工生成 Kotlin 绑定时使用的源码目录（可选，存在即参与编译）。 */
val uniffiManualDir: File = layout.projectDirectory.dir("src/main/java/dev/beam/core/net/uniffi").asFile

/** 读取 `local.properties` 里的 `sdk.dir`。 */
fun readSdkDir(): String? {
    val file = rootProject.file("local.properties")
    if (!file.exists()) return null
    val props = Properties()
    file.inputStream().use { props.load(it) }
    return props.getProperty("sdk.dir")
}

/**
 * Android NDK 根目录。
 *
 * 优先级：`-Pbeam.ndk.home` > `ANDROID_NDK_HOME` 环境变量 > `local.properties` 的 `sdk.dir\ndk\<最新版本>`。
 */
val ndkHome: String? = run {
    val fromProperty = providers.gradleProperty("beam.ndk.home").orNull
    if (!fromProperty.isNullOrBlank()) {
        fromProperty
    } else {
        System.getenv("ANDROID_NDK_HOME")?.takeIf { it.isNotBlank() }
            ?: readSdkDir()?.let { sdk ->
                File(sdk, "ndk")
                    .listFiles { f -> f.isDirectory }
                    ?.maxByOrNull { it.name }
                    ?.absolutePath
            }
    }
}

/** uniffi-bindgen 可执行文件：`-Pbeam.uniffi.bindgen` 优先，否则用 PATH 上的。 */
val uniffiBindgen: String = providers.gradleProperty("beam.uniffi.bindgen").orNull ?: "uniffi-bindgen"

/** 交叉编译出来的原生库（arm64-v8a 是必选目标，x86_64 供模拟器用）。 */
val nativeLibFile: File = File(jniLibsDir, "arm64-v8a/libbeam_ffi.so")

/**
 * 用 cargo-ndk 交叉编译 `libbeam_ffi.so`。
 *
 * 产物落在 `src/main/jniLibs/{arm64-v8a,x86_64}/`，由 AGP 自动打进 APK。
 */
val cargoNdkBuild by tasks.registering(Exec::class) {
    group = "beam native"
    description = "用 cargo-ndk 交叉编译 libbeam_ffi.so（arm64-v8a + x86_64）"
    workingDir(cargoWorkspace)
    commandLine(
        "cargo", "ndk",
        "-t", "arm64-v8a",
        "-t", "x86_64",
        "-o", jniLibsDir.absolutePath,
        "build", "--release", "-p", "beam-ffi", "--features", "uniffi",
    )
    doFirst {
        val home = ndkHome
            ?: throw GradleException(
                "找不到 Android NDK：请设置 ANDROID_NDK_HOME 环境变量、" +
                    "或用 -Pbeam.ndk.home=<ndk 路径> 指定。",
            )
        environment("ANDROID_NDK_HOME", home)
    }
}

/**
 * 用 uniffi-bindgen 生成 Kotlin 绑定。
 *
 * 直接读**刚编出来的** `.so`，保证绑定与原生库来自同一份契约。
 */
val generateUniffiBindings by tasks.registering(Exec::class) {
    group = "beam native"
    description = "用 uniffi-bindgen 生成 Kotlin 绑定（uniffi.beam_ffi.*）"
    dependsOn(cargoNdkBuild)
    inputs.file(nativeLibFile)
    outputs.dir(uniffiGeneratedDir)
    workingDir(cargoWorkspace)
    commandLine(
        uniffiBindgen,
        "generate",
        "--library", nativeLibFile.absolutePath,
        "--language", "kotlin",
        "--out-dir", uniffiGeneratedDir.get().asFile.absolutePath,
    )
    doFirst {
        if (!nativeLibFile.exists()) {
            throw GradleException("找不到 ${nativeLibFile.absolutePath}，请先执行 :core-net:cargoNdkBuild。")
        }
    }
}

android {
    namespace = "dev.beam.core.net"
    compileSdk = 36
    buildToolsVersion = "36.0.0"

    defaultConfig {
        minSdk = 26
        consumerProguardFiles("consumer-rules.pro")
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    sourceSets {
        getByName("main") {
            // 生成的 Kotlin 绑定挂进编译。
            // 手工生成的副本（src/main/java/.../uniffi）优先，避免重复类。
            if (!uniffiManualDir.exists()) {
                java.srcDir(uniffiGeneratedDir)
            }
        }
    }

    testOptions {
        unitTests.isReturnDefaultValues = true
    }
}

kotlin {
    compilerOptions {
        jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17)
    }
}

dependencies {
    // BeamClient / Session 的公开 API 暴露 Flow，因此 coroutines 需要 api 可见。
    api(libs.kotlinx.coroutines.core)
    implementation(libs.kotlinx.coroutines.android)

    // 反射构造 uniffi 生成的 record 时需要按参数名装配（callBy）。
    implementation(libs.kotlin.reflect)

    // uniffi 生成的 Kotlin 绑定依赖 JNA（com.sun.jna.*）。
    // 只有真的要编译生成物时才引入，避免给降级构建塞无用依赖。
    if (beamNativeEnabled || uniffiGeneratedDir.get().asFile.exists() || uniffiManualDir.exists()) {
        implementation(libs.jna)
    }

    testImplementation(libs.junit)
    testImplementation(libs.kotlinx.coroutines.test)
}

// 只有显式打开时才把原生构建接进常规构建（见文件头说明）。
if (beamNativeEnabled) {
    tasks.matching { it.name.endsWith("NativeLibs") }.configureEach {
        dependsOn(cargoNdkBuild)
    }
    tasks.matching { it.name.startsWith("compile") && it.name.endsWith("Kotlin") }.configureEach {
        dependsOn(generateUniffiBindings)
    }
}
