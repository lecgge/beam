import org.jetbrains.kotlin.gradle.dsl.JvmTarget

plugins {
    alias(libs.plugins.android.library)
    alias(libs.plugins.kotlin.android)
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

    testImplementation(libs.junit)
    testImplementation(libs.kotlinx.coroutines.test)
}
