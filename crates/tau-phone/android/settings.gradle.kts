// Packages tau-phone's library, built beforehand into
// app/src/main/jniLibs, as an APK: `nix build .#tau-phone-apk` does both,
// offline, from deps.json. The steps are in ../src/lib.rs.

pluginManagement {
    repositories {
        google()
        mavenCentral()
        gradlePluginPortal()
    }
}

dependencyResolutionManagement {
    repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS)
    repositories {
        google()
        mavenCentral()
    }
}

rootProject.name = "tau"
include(":app")
