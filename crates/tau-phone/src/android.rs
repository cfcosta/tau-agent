//! The app Android's NativeActivity starts.

use std::{path::PathBuf, sync::Arc};

use gpui::{App, AppContext, Application, WindowOptions};
use gpui_mobile::android::jni as mobile;
use jni::{
    EnvUnowned,
    errors::{Error, ThrowRuntimeExAndDefault},
    objects::{JByteBuffer, JClass, JObject, JString, JValue},
    sys::jint,
};
use tau_ui_remote::{
    Workspace,
    assets::Assets,
    catalog::Catalog,
    remote::{self, Platform},
};

use crate::qr::{self, Frame};

/// tau's own Java, in the APK beside gpui-pre-mobile's.
const SCANNER: &str = "dev.cfcosta.tau.TauScanner";

/// What shows agents' notices as Android notifications.
const NOTICES: &str = "dev.cfcosta.tau.TauNotices";

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
            tau_ui_remote::init(cx);
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
                    follow_agents(&workspace, cx);
                }
                Err(error) => {
                    log::error!("tau: could not open a window: {error:#}")
                }
            }
            cx.activate(true);
        });
}

/// Opens tau's viewfinder and waits for the computer's pairing code in
/// it. Blocks until the viewfinder closes.
fn scan() -> Result<Option<String>, String> {
    mobile::with_env(|env| {
        let activity = mobile::activity(env)?;
        let class = mobile::find_app_class(env, SCANNER)?;
        let code = env
            .call_static_method(
                &class,
                jni::jni_str!("scan"),
                jni::jni_sig!("(Landroid/app/Activity;)Ljava/lang/String;"),
                &[JValue::Object(&activity)],
            )
            .and_then(|value| value.l())
            .map_err(|error| match env.exception_catch() {
                // The viewfinder says what went wrong in words.
                Err(Error::CaughtJavaException { msg, .. }) => msg,
                _ => format!("The camera did not open: {error}"),
            })?;
        Ok((!code.is_null()).then(|| mobile::get_string(env, &code)))
    })
}

/// `TauViewfinder.decode`: the pairing code in a camera frame's
/// luminance, or null. The viewfinder calls it on its analysis thread,
/// one frame at a time, and drops the frames that come meanwhile.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_cfcosta_tau_TauViewfinder_decode<'caller>(
    mut env: EnvUnowned<'caller>,
    _class: JClass<'caller>,
    luma: JByteBuffer<'caller>,
    width: jint,
    height: jint,
    stride: jint,
) -> JObject<'caller> {
    env.with_env(|env| -> jni::errors::Result<JObject<'caller>> {
        let address = env.get_direct_buffer_address(&luma)?;
        let len = env.get_direct_buffer_capacity(&luma)?;
        // SAFETY: a direct buffer's memory; the viewfinder closes the
        // frame it belongs to only once this returns.
        let luma = unsafe { std::slice::from_raw_parts(address, len) };
        let size = |n: jint| usize::try_from(n).unwrap_or(0);
        let code = Frame::new(luma, size(width), size(height), size(stride))
            .and_then(qr::pairing_code);
        Ok(match code {
            Some(code) => JString::from_str(env, code)?.into(),
            None => JObject::null(),
        })
    })
    .resolve::<ThrowRuntimeExAndDefault>()
}

/// Shows what the computer's agents leave the person as Android
/// notifications, while the phone is connected (ADR 0037): each note or
/// question, and each agent that reaches its cap, once.
fn follow_agents(workspace: &gpui::Entity<Workspace>, cx: &mut App) {
    let mut watch = tau_agents::alert::Watch::default();
    cx.observe(workspace, move |workspace, cx| {
        let overview = workspace.read(cx).agents().cloned().unwrap_or_default();
        for alert in watch.observe(&overview) {
            let tag = format!("{}:{:?}", alert.agent, alert.kind);
            if let Err(error) = show(&tag, &alert.title, &alert.body) {
                log::error!("tau: cannot show a notification: {error}");
            }
        }
    })
    .detach();
}

/// Shows one notification, `tag` its own: the same tag again replaces it.
fn show(tag: &str, title: &str, body: &str) -> Result<(), String> {
    mobile::with_env(|env| {
        let activity = mobile::activity(env)?;
        let class = mobile::find_app_class(env, NOTICES)?;
        let tag = env.new_string(tag).map_err(|error| error.to_string())?;
        let title = env.new_string(title).map_err(|error| error.to_string())?;
        let body = env.new_string(body).map_err(|error| error.to_string())?;
        env.call_static_method(
            &class,
            jni::jni_str!("show"),
            jni::jni_sig!(
                "(Landroid/app/Activity;Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)V"
            ),
            &[
                JValue::Object(&activity),
                JValue::Object(&tag),
                JValue::Object(&title),
                JValue::Object(&body),
            ],
        )
        .map(|_| ())
        .map_err(|error| {
            env.exception_clear();
            error.to_string()
        })
    })
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
