import org.jetbrains.kotlin.gradle.dsl.JvmTarget

plugins {
    alias(libs.plugins.android.library)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.kotlin.compose)
}

android {
    namespace = "dev.beam.core.data"
    compileSdk = 36
    buildToolsVersion = "36.0.0"

    defaultConfig {
        minSdk = 26
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    // §16 设计规范里的通用组件（模态弹窗 / Toast）放在本模块的 ui 包里，
    // 需要 Compose 编译器。
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
    api(project(":core-net"))

    // DataStore 设置与信任主机（§3.12 :core-data）。
    api(libs.androidx.datastore.preferences)
    implementation(libs.kotlinx.coroutines.core)
    implementation(libs.kotlinx.coroutines.android)

    // 设计令牌（dp/sp）对外暴露在 BeamTokens 的公开 API 上。
    api(platform(libs.androidx.compose.bom))
    api(libs.androidx.compose.ui)
    // @Composable / Modifier 出现在 BeamDialog、BeamToast 的公开 API 上。
    api(libs.androidx.compose.runtime)

    // 通用组件（模态弹窗 / Toast）的实现细节，不外泄到调用方的编译期。
    implementation(libs.androidx.compose.foundation)
    implementation(libs.androidx.compose.material3)
    // material3 1.3.2 默认带 lifecycle-common-java8:2.8.7，这里统一到工程统一的
    // 2.9.1（其余模块都是 2.9.1），避免同工程内出现两套 lifecycle 版本。
    implementation(libs.androidx.lifecycle.runtime.ktx)

    testImplementation(libs.junit)
    testImplementation(libs.kotlinx.coroutines.test)
}
