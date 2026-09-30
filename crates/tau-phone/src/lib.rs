//! tau-ui on an Android phone.
//!
//! For now it shows the demo session with no host: runs, history and
//! screens as `tau-ui --demo` has them, answered the way a host would.
//! Connecting to a running tau is decision 0013.
//!
//! To build the APK, in the flake's `android` shell (`nix develop
//! .#android`), from the repository's root:
//!
//! ```text
//! cargo ndk -t arm64-v8a -P 26 -o crates/tau-phone/android/app/src/main/jniLibs \
//!     build -p tau-phone --release
//! gradle -p crates/tau-phone/android assembleDebug
//! ```
//!
//! The APK is `crates/tau-phone/android/app/build/outputs/apk/debug/app-debug.apk`;
//! `adb install` it.

#![cfg(target_os = "android")]

use gpui::{App, AppContext, Application, WindowOptions};
use gpui_mobile::android::jni;
use tau_ui::{Workspace, assets::Assets, demo};

/// Called by `android-activity` on its own thread once NativeActivity
/// loads this library. Returns when the activity is gone.
#[unsafe(no_mangle)]
fn android_main(app: android_activity::AndroidApp) {
    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(log::LevelFilter::Info)
            .with_tag("tau"),
    );
    jni::install_panic_hook();

    let _platform = jni::init_platform(&app);
    let Some(platform) = jni::shared_platform() else {
        log::error!("tau: no Android platform");
        return;
    };

    // Blocks, driving Android's event loop. The closure runs once the
    // system hands over a surface.
    Application::with_platform(platform.into_rc())
        .with_assets(Assets)
        .run(|cx: &mut App| {
            tau_ui::init(cx);
            // The system gives the window its size.
            let options = WindowOptions {
                window_bounds: None,
                ..Default::default()
            };
            let opened = cx.open_window(options, |window, cx| {
                cx.new(|cx| {
                    let mut runs = vec![demo::retry_after()];
                    runs.extend(demo::history());
                    let mut workspace = Workspace::new(
                        "tau-agent",
                        runs,
                        demo::catalog(),
                        window,
                        cx,
                    );
                    workspace.replay(demo::run_id(), demo::script(), cx);
                    workspace
                })
            });
            match opened.and_then(|window| window.entity(cx)) {
                Ok(workspace) => demo::respond(&workspace, cx),
                Err(error) => log::error!("tau: could not open a window: {error:#}"),
            }
            cx.activate(true);
        });
}
