import org.jetbrains.kotlin.gradle.dsl.JvmTarget

plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.kotlin.compose)
}

android {
    namespace = "dev.beam.app"
    compileSdk = 36
    buildToolsVersion = "36.0.0"

    defaultConfig {
        applicationId = "dev.beam.app"
        minSdk = 26
        targetSdk = 36
        versionCode = 1
        versionName = "0.1.0"
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
    }

    buildTypes {
        debug {
            isMinifyEnabled = false
            // §3.13 门控：debug 开启性能统计。
            buildConfigField("boolean", "PERF_STATS", "true")
        }
        release {
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro",
            )
            // §3.13：release 必须关闭性能统计。
            buildConfigField("boolean", "PERF_STATS", "false")
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    buildFeatures {
        compose = true
        buildConfig = true
    }

    packaging {
        resources {
            excludes += setOf(
                "/META-INF/{AL2.0,LGPL2.1}",
                "META-INF/DEPENDENCIES",
            )
        }
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
    implementation(project(":core-net"))
    implementation(project(":core-data"))
    implementation(project(":core-decoder"))
    implementation(project(":core-renderer"))
    implementation(project(":core-input"))
    implementation(project(":core-audio"))

    implementation(project(":feature-discovery"))
    implementation(project(":feature-pairing"))
    implementation(project(":feature-settings"))
    implementation(project(":feature-stream"))

    // §3.12 / §3.13：性能面板只允许出现在 debug 依赖图里。
    // release 变体不含本行，因此 release APK 中不存在任何 feature-perf 类。
    debugImplementation(project(":feature-perf"))

    implementation(platform(libs.androidx.compose.bom))
    implementation(libs.androidx.compose.ui)
    implementation(libs.androidx.compose.ui.graphics)
    implementation(libs.androidx.compose.foundation)
    implementation(libs.androidx.compose.material3)
    implementation(libs.androidx.activity.compose)
    implementation(libs.androidx.lifecycle.runtime.compose)
    implementation(libs.androidx.lifecycle.viewmodel.compose)
    implementation(libs.androidx.lifecycle.viewmodel.ktx)
    implementation(libs.androidx.navigation.compose)

    debugImplementation(libs.androidx.compose.ui.tooling)
    implementation(libs.androidx.compose.ui.tooling.preview)

    testImplementation(libs.junit)
    testImplementation(libs.kotlinx.coroutines.test)
}
