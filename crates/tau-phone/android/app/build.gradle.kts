plugins {
    id("com.android.application")
}

android {
    namespace = "dev.cfcosta.tau"
    compileSdk = 36

    defaultConfig {
        applicationId = "dev.cfcosta.tau"
        // Android 12 (API 31) and later: libc exports
        // `android_get_device_api_level`, which gpui-mobile calls, only
        // from API 29, and tau supports Android 12 on.
        minSdk = 31
        targetSdk = 36
        versionCode = 1
        versionName = "0.1.0"
        ndk {
            abiFilters += listOf("arm64-v8a")
        }
    }

    // A debug key committed with the project, so every build, from the
    // dev shell or from `nix build .#tau-phone-apk`, signs with the same
    // key and installs over the last one. It is public: never sign a
    // release with it.
    signingConfigs {
        getByName("debug") {
            storeFile = rootProject.file("debug.keystore")
            storePassword = "android"
            keyAlias = "androiddebugkey"
            keyPassword = "android"
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
