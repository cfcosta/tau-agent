plugins {
    id("com.android.application")
}

android {
    namespace = "dev.cfcosta.tau"
    compileSdk = 36

    defaultConfig {
        applicationId = "dev.cfcosta.tau"
        // Vulkan is there from API 26 on.
        minSdk = 26
        targetSdk = 36
        versionCode = 1
        versionName = "0.1.0"
        ndk {
            abiFilters += listOf("arm64-v8a")
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

dependencies {
    // The viewfinder: CameraX's preview and frames, without Google Play
    // services, so it works on de-Googled phones. The frames are read by
    // tau's own decoder, in Rust.
    val camerax = "1.6.2"
    implementation("androidx.camera:camera-camera2:$camerax")
    implementation("androidx.camera:camera-lifecycle:$camerax")
    implementation("androidx.camera:camera-view:$camerax")
    implementation("androidx.activity:activity:1.13.0")
}
