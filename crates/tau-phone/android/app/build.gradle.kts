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
