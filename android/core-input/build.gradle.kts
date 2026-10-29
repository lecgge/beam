import org.jetbrains.kotlin.gradle.dsl.JvmTarget

plugins {
    alias(libs.plugins.android.library)
    alias(libs.plugins.kotlin.android)
    // §3.12 `:core-input` 提供 Compose 输入采集 Modifier，需要 Compose 编译器插件。
    alias(libs.plugins.kotlin.compose)
}

android {
    namespace = "dev.beam.core.input"
    compileSdk = 36
    buildToolsVersion = "36.0.0"

    defaultConfig {
        minSdk = 26
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    buildFeatures {
        compose = true
    }

    testOptions {
        unitTests.isReturnDefaultValues = true
    }
}

kotlin {
    compilerOptions {
        jvmTarget.set(JvmTarget.JVM_17)
    }
}

dependencies {
    // InputEvent 的定义在 :core-net，对外暴露因此使用 api。
    api(project(":core-net"))

    implementation(libs.kotlinx.coroutines.core)
    implementation(libs.kotlinx.coroutines.android)

    // 输入采集 Modifier（触控 pointerInput / 按键 onKeyEvent / 手柄 generic motion）。
    implementation(platform(libs.androidx.compose.bom))
    implementation(libs.androidx.compose.ui)
    implementation(libs.androidx.compose.ui.graphics)

    testImplementation(libs.junit)
    testImplementation(libs.kotlinx.coroutines.test)
}
