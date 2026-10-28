//! Android: the activity's entry point, and the few platform calls the app needs, over JNI.

use jni::JavaVM;
use jni::objects::{JObject, JValue};
use slint::android::AndroidApp;
use slint::android::android_activity::WindowManagerFlags;

use std::sync::OnceLock;

use crate::backend::Source;
use crate::bridge::UserIntent;

/// kpclientd-android serves the thin client here.
const DAEMON: &str = "127.0.0.1:64331";
const RECORD_AUDIO: &str = "android.permission.RECORD_AUDIO";

/// The app, for its activity: a permission request needs the activity, not the application.
static APP: OnceLock<AndroidApp> = OnceLock::new();

#[unsafe(no_mangle)]
fn android_main(app: AndroidApp) {
    let _ = APP.set(app.clone());
    // Keep every window out of screenshots, screen recording and the recent-apps thumbnails.
    // Only test builds, made to take screenshots of, leave this out.
    if !cfg!(feature = "test-screenshots") {
        app.set_window_flags(WindowManagerFlags::SECURE, WindowManagerFlags::empty());
    }
    let data = app.internal_data_path().unwrap_or_else(|| "/data/local/tmp/hushphone".into());
    slint::android::init(app).expect("Slint on Android");
    let setup = crate::Setup {
        cache_dir: data.join("cache"),
        data_dir: data,
        source: Source::Daemon(String::new()),
        default_daemon: DAEMON.into(),
        name: None,
    };
    if let Err(e) = crate::run(setup) {
        eprintln!("hushphone: {e}");
    }
}

fn with_activity<T>(f: impl FnOnce(&mut jni::JNIEnv, &JObject) -> jni::errors::Result<T>) -> Result<T, String> {
    let app = APP.get().ok_or("no activity")?;
    let vm = unsafe { JavaVM::from_raw(app.vm_as_ptr().cast()) }.map_err(|e| e.to_string())?;
    let mut env = vm.attach_current_thread().map_err(|e| e.to_string())?;
    let activity = unsafe { JObject::from_raw(app.activity_as_ptr().cast()) };
    let r = f(&mut env, &activity);
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_describe();
        let _ = env.exception_clear();
        return Err("the platform refused".into());
    }
    r.map_err(|e| e.to_string())
}

/// Whether we may record; if not, ask, and the user presses again once they have answered.
pub fn microphone_allowed(_: &UserIntent) -> bool {
    with_activity(|env, activity| {
        let perm = env.new_string(RECORD_AUDIO)?;
        let granted = env
            .call_method(activity, "checkSelfPermission", "(Ljava/lang/String;)I", &[JValue::Object(&perm)])?
            .i()?
            == 0;
        if !granted {
            let arr = env.new_object_array(1, "java/lang/String", &perm)?;
            env.call_method(activity, "requestPermissions", "([Ljava/lang/String;I)V", &[JValue::Object(&arr), JValue::Int(1)])?;
        }
        Ok(granted)
    })
    .unwrap_or(false)
}

fn clipboard<'a>(env: &mut jni::JNIEnv<'a>, activity: &JObject) -> jni::errors::Result<JObject<'a>> {
    let name = env.new_string("clipboard")?;
    env.call_method(activity, "getSystemService", "(Ljava/lang/String;)Ljava/lang/Object;", &[JValue::Object(&name)])?
        .l()
}

/// Put `text` on the clipboard; `sensitive` keeps it out of the clipboard preview.
pub fn copy_text(text: &str, sensitive: bool) -> Result<(), String> {
    with_activity(|env, activity| {
        let cm = clipboard(env, activity)?;
        let label = env.new_string("hushphone")?;
        let text = env.new_string(text)?;
        let clip = env
            .call_static_method(
                "android/content/ClipData",
                "newPlainText",
                "(Ljava/lang/CharSequence;Ljava/lang/CharSequence;)Landroid/content/ClipData;",
                &[JValue::Object(&label), JValue::Object(&text)],
            )?
            .l()?;
        if sensitive {
            let desc = env.call_method(&clip, "getDescription", "()Landroid/content/ClipDescription;", &[])?.l()?;
            let extras = env.new_object("android/os/PersistableBundle", "()V", &[])?;
            let key = env.new_string("android.content.extra.IS_SENSITIVE")?;
            env.call_method(&extras, "putBoolean", "(Ljava/lang/String;Z)V", &[JValue::Object(&key), JValue::Bool(1)])?;
            env.call_method(&desc, "setExtras", "(Landroid/os/PersistableBundle;)V", &[JValue::Object(&extras)])?;
        }
        env.call_method(&cm, "setPrimaryClip", "(Landroid/content/ClipData;)V", &[JValue::Object(&clip)])?;
        Ok(())
    })
}

pub fn paste_text() -> Result<String, String> {
    with_activity(|env, activity| {
        let cm = clipboard(env, activity)?;
        let clip = env.call_method(&cm, "getPrimaryClip", "()Landroid/content/ClipData;", &[])?.l()?;
        if clip.is_null() {
            return Ok(String::new());
        }
        let item = env.call_method(&clip, "getItemAt", "(I)Landroid/content/ClipData$Item;", &[JValue::Int(0)])?.l()?;
        let cs = env.call_method(&item, "getText", "()Ljava/lang/CharSequence;", &[])?.l()?;
        if cs.is_null() {
            return Ok(String::new());
        }
        let s = env.call_method(&cs, "toString", "()Ljava/lang/String;", &[])?.l()?;
        Ok(env.get_string(&s.into())?.into())
    })
}
