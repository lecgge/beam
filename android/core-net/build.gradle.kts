plugins {
    alias(libs.plugins.android.library)
    alias(libs.plugins.kotlin.android)
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

    testImplementation(libs.junit)
    testImplementation(libs.kotlinx.coroutines.test)
}
