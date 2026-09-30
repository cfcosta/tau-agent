//! The app Android's NativeActivity starts.

use std::{path::PathBuf, sync::Arc};

use gpui::{App, AppContext, Application, WindowOptions};
use gpui_mobile::android::jni as mobile;
use jni::objects::JValue;
use tau_ui::{
    Workspace,
    assets::Assets,
    catalog::Catalog,
    remote::{self, Platform},
};

/// tau's own Java, in the APK beside gpui-pre-mobile's.
const SCANNER: &str = "dev.cfcosta.tau.TauScanner";

/// Called by `android-activity` on its own thread once NativeActivity
/// loads this library. Returns when the activity is gone.
#[unsafe(no_mangle)]
fn android_main(app: android_activity::AndroidApp) {
    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(log::LevelFilter::Info)
            .with_tag("tau"),
    );
    mobile::install_panic_hook();

    let _platform = mobile::init_platform(&app);
    let Some(platform) = mobile::shared_platform() else {
        log::error!("tau: no Android platform");
        return;
    };
    let dir = app
        .internal_data_path()
        .unwrap_or_else(|| PathBuf::from("/data/local/tmp/tau"));

    // Blocks, driving Android's event loop. The closure runs once the
    // system hands over a surface.
    Application::with_platform(platform.into_rc())
        .with_assets(Assets)
        .run(move |cx: &mut App| {
            tau_ui::init(cx);
            // The system gives the window its size.
            let options = WindowOptions {
                window_bounds: None,
                ..Default::default()
            };
            let opened = cx.open_window(options, |window, cx| {
                cx.new(|cx| {
                    Workspace::new(
                        "tau",
                        Vec::new(),
                        Catalog::default(),
                        window,
                        cx,
                    )
                })
            });
            match opened.and_then(|window| window.entity(cx)) {
                Ok(workspace) => {
                    let platform = Platform {
                        dir,
                        scan: Arc::new(scan),
                        name: device_name(),
                    };
                    remote::connect(platform, &workspace, cx);
                }
                Err(error) => {
                    log::error!("tau: could not open a window: {error:#}")
                }
            }
            cx.activate(true);
        });
}

/// Photographs the computer's pairing code with the camera app and reads
/// it. Blocks until the camera closes.
fn scan() -> Result<Option<String>, String> {
    let path = mobile::with_env(|env| {
        let activity = mobile::activity(env)?;
        let class = mobile::find_app_class(env, SCANNER)?;
        let path = env
            .call_static_method(
                &class,
                jni::jni_str!("capture"),
                jni::jni_sig!("(Landroid/app/Activity;)Ljava/lang/String;"),
                &[JValue::Object(&activity)],
            )
            .and_then(|value| value.l())
            .map_err(|error| {
                env.exception_clear();
                format!("The camera did not open: {error}")
            })?;
        Ok((!path.is_null()).then(|| mobile::get_string(env, &path)))
    })?;
    let Some(path) = path.map(PathBuf::from) else {
        return Ok(None);
    };
    let read = crate::qr::read(&path);
    let _ = std::fs::remove_file(&path);
    read.map(Some)
}

/// The phone's model, as the computer lists it.
fn device_name() -> String {
    let name = mobile::with_env(|env| {
        let class = mobile::find_app_class(env, SCANNER)?;
        let name = env
            .call_static_method(
                &class,
                jni::jni_str!("deviceName"),
                jni::jni_sig!("()Ljava/lang/String;"),
                &[],
            )
            .and_then(|value| value.l())
            .map_err(|error| {
                env.exception_clear();
                error.to_string()
            })?;
        Ok(mobile::get_string(env, &name))
    });
    match name {
        Ok(name) if !name.is_empty() => name,
        _ => "Android phone".into(),
    }
}
