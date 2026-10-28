//! Android: the activity's entry point, and the few platform calls the app needs, over JNI.

use jni::JavaVM;
use jni::objects::{JObject, JValue};
use slint::android::AndroidApp;
use slint::android::android_activity::WindowManagerFlags;

use std::os::fd::{FromRawFd, OwnedFd};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::backend::Source;
use crate::bridge::UserIntent;

/// Where kpclientd-android hands out connections to the daemon, to the apps the user allows.
const CONNECT: &str = "content://org.hoi_polloi.kpdaemon.connect";
/// What an older hushphone saved as the daemon's address, when kpclientd-android listened there.
const OLD_DAEMON: &str = "127.0.0.1:64331";
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
    // The thin client dials a unix socket in the app's private directory, where each connection
    // is joined to one kpclientd-android hands out: no other app can reach it, and the app
    // needs no network of its own.
    let relay = data.join("kpclientd.sock");
    start_relay(relay.clone());
    let setup = crate::Setup {
        cache_dir: data.join("cache"),
        data_dir: data,
        source: Source::Daemon(String::new()),
        default_daemon: relay.to_string_lossy().into_owned(),
        name: None,
        old_defaults: vec![OLD_DAEMON.into()],
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

/// Why kpclientd-android gave no connection.
#[derive(Debug)]
enum Refused {
    /// kpclientd-android is not installed, or too old to hand out connections.
    Missing,
    /// The user has not answered yet; the question is open now.
    Asked,
    Denied,
    Stopped,
    Other(String),
}

impl Refused {
    fn sentence(&self) -> String {
        match self {
            Refused::Missing => "kpclientd-android isn't installed, or is too old to let apps in".into(),
            Refused::Asked => "allow hushphone in kpclientd-android".into(),
            Refused::Denied => "kpclientd-android refuses hushphone; forget its answer in kpclientd-android's Apps list to be asked again".into(),
            Refused::Stopped => "the daemon isn't running; start it in kpclientd-android".into(),
            Refused::Other(e) => e.clone(),
        }
    }
}

/// Why the last connection to kpclientd failed, for the app to say.
static PROBLEM: Mutex<Option<String>> = Mutex::new(None);
/// When the question was last opened, so that retrying does not open it again and again.
static ASKED: Mutex<Option<Instant>> = Mutex::new(None);
const ASK_AGAIN: Duration = Duration::from_secs(60);

pub(crate) fn problem() -> Option<String> {
    PROBLEM.lock().unwrap().clone()
}

/// A socket connected to kpclientd, from kpclientd-android.
fn connect() -> Result<OwnedFd, Refused> {
    with_activity(|env, activity| {
        let resolver = env
            .call_method(activity, "getContentResolver", "()Landroid/content/ContentResolver;", &[])?
            .l()?;
        let text = env.new_string(CONNECT)?;
        let uri = env
            .call_static_method("android/net/Uri", "parse", "(Ljava/lang/String;)Landroid/net/Uri;", &[JValue::Object(&text)])?
            .l()?;
        let method = env.new_string("connect")?;
        let null = JObject::null();
        let reply = env.call_method(
            &resolver,
            "call",
            "(Landroid/net/Uri;Ljava/lang/String;Ljava/lang/String;Landroid/os/Bundle;)Landroid/os/Bundle;",
            &[JValue::Object(&uri), JValue::Object(&method), JValue::Object(&null), JValue::Object(&null)],
        );
        // A provider that is not there is an IllegalArgumentException.
        if env.exception_check()? {
            env.exception_clear()?;
            return Ok(Err(Refused::Missing));
        }
        let bundle = reply?.l()?;
        if bundle.is_null() {
            return Ok(Err(Refused::Missing));
        }
        let error = get(env, &bundle, "error", "getString", "(Ljava/lang/String;)Ljava/lang/String;")?;
        if !error.is_null() {
            let error: String = env.get_string(&error.into())?.into();
            return Ok(Err(match error.as_str() {
                "approval" => {
                    let pending = get(env, &bundle, "approve", "getParcelable", "(Ljava/lang/String;)Landroid/os/Parcelable;")?;
                    ask(env, activity, &pending)?;
                    Refused::Asked
                }
                "denied" => Refused::Denied,
                "stopped" => Refused::Stopped,
                other => Refused::Other(format!("kpclientd-android says {other}")),
            }));
        }
        let pfd = get(env, &bundle, "socket", "getParcelable", "(Ljava/lang/String;)Landroid/os/Parcelable;")?;
        if pfd.is_null() {
            return Ok(Err(Refused::Other("kpclientd-android gave no socket".into())));
        }
        let fd = env.call_method(&pfd, "detachFd", "()I", &[])?.i()?;
        // SAFETY: detachFd hands over the descriptor; nothing else owns it now.
        Ok(Ok(unsafe { OwnedFd::from_raw_fd(fd) }))
    })
    .unwrap_or_else(|e| Err(Refused::Other(e)))
}

/// The value at `key` in `bundle`, read with its getter `name` of signature `sig`.
fn get<'a>(env: &mut jni::JNIEnv<'a>, bundle: &JObject, key: &str, name: &str, sig: &str) -> jni::errors::Result<JObject<'a>> {
    let k = env.new_string(key)?;
    env.call_method(bundle, name, sig, &[JValue::Object(&k)])?.l()
}

/// Open kpclientd-android's question about this app, unless it was opened a moment ago.
fn ask(env: &mut jni::JNIEnv, activity: &JObject, pending: &JObject) -> jni::errors::Result<()> {
    let mut asked = ASKED.lock().unwrap();
    if pending.is_null() || asked.is_some_and(|t| t.elapsed() < ASK_AGAIN) {
        return Ok(());
    }
    *asked = Some(Instant::now());
    let sender = env.call_method(pending, "getIntentSender", "()Landroid/content/IntentSender;", &[])?.l()?;
    let options = env
        .call_static_method("android/app/ActivityOptions", "makeBasic", "()Landroid/app/ActivityOptions;", &[])?
        .l()?;
    let sdk = env.get_static_field("android/os/Build$VERSION", "SDK_INT", "I")?.i()?;
    if sdk >= 34 {
        // We are in the foreground: let the question open from here.
        env.call_method(&options, "setPendingIntentBackgroundActivityStartMode", "(I)Landroid/app/ActivityOptions;", &[JValue::Int(1)])?;
    }
    let bundle = env.call_method(&options, "toBundle", "()Landroid/os/Bundle;", &[])?.l()?;
    let null = JObject::null();
    env.call_method(
        activity,
        "startIntentSender",
        "(Landroid/content/IntentSender;Landroid/content/Intent;IIILandroid/os/Bundle;)V",
        &[JValue::Object(&sender), JValue::Object(&null), JValue::Int(0), JValue::Int(0), JValue::Int(0), JValue::Object(&bundle)],
    )?;
    Ok(())
}

/// Listen at `path`, in the app's private directory, and join each connection to one from
/// kpclientd-android.
fn start_relay(path: PathBuf) {
    std::thread::Builder::new()
        .name("kpclientd-relay".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("relay runtime");
            rt.block_on(async move {
                let _ = std::fs::remove_file(&path);
                let listener = match tokio::net::UnixListener::bind(&path) {
                    Ok(l) => l,
                    Err(e) => {
                        *PROBLEM.lock().unwrap() = Some(format!("can't listen at {}: {e}", path.display()));
                        return;
                    }
                };
                while let Ok((mut app, _)) = listener.accept().await {
                    tokio::spawn(async move {
                        match tokio::task::spawn_blocking(connect).await {
                            Ok(Ok(fd)) => {
                                *PROBLEM.lock().unwrap() = None;
                                let s = std::os::unix::net::UnixStream::from(fd);
                                if s.set_nonblocking(true).is_err() {
                                    return;
                                }
                                if let Ok(mut daemon) = tokio::net::UnixStream::from_std(s) {
                                    let _ = tokio::io::copy_bidirectional(&mut app, &mut daemon).await;
                                }
                            }
                            Ok(Err(why)) => *PROBLEM.lock().unwrap() = Some(why.sentence()),
                            Err(e) => *PROBLEM.lock().unwrap() = Some(e.to_string()),
                        }
                    });
                }
            });
        })
        .expect("relay thread");
}
