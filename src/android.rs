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
    crate::harden::process();
    let _ = APP.set(app.clone());
    // Keep every window out of screenshots, screen recording and the recent-apps thumbnails.
    // Only test builds, made to take screenshots of, leave this out.
    if !cfg!(feature = "test-screenshots") {
        app.set_window_flags(WindowManagerFlags::SECURE, WindowManagerFlags::empty());
    }
    let data = private_dir(&app);
    slint::android::init(app).expect("Slint on Android");
    // The thin client dials a unix socket in the app's private directory, where each connection
    // is joined to one kpclientd-android hands out: no other app can reach it, and the app
    // needs no network of its own.
    let relay = data.join("kpclientd.sock");
    start_relay(relay.clone());
    let setup = crate::Setup {
        cache_dir: data.join("cache"),
        data_dir: data,
        // Always the relay: it is part of the app, not a setting, and its place has moved before.
        source: Source::Daemon(relay.to_string_lossy().into_owned()),
        default_daemon: relay.to_string_lossy().into_owned(),
        name: None,
        old_defaults: vec![OLD_DAEMON.into()],
        keys: crate::backend::Keys::System,
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

/// The app's directory that Android leaves out of cloud backups and device-to-device transfers,
/// `no_backup`, beside its files directory: the keys of every group live there. What an older
/// version kept in the files directory moves over.
fn private_dir(app: &AndroidApp) -> PathBuf {
    let files = app.internal_data_path().unwrap_or_else(|| "/data/local/tmp/hushphone/files".into());
    let dir = files.parent().map(|p| p.join("no_backup")).unwrap_or_else(|| files.join("no_backup"));
    let _ = std::fs::create_dir_all(&dir);
    for name in ["state", "settings.toml", "cache"] {
        let (old, new) = (files.join(name), dir.join(name));
        if old.exists() && !new.exists() {
            let _ = std::fs::rename(&old, &new);
        }
    }
    let _ = std::fs::remove_file(files.join("kpclientd.sock"));
    dir
}

/// The Keystore's name for the key that wraps the state's key.
const STATE_KEY: &str = "hushphone-state";

/// The Keystore's AES key that wraps the state's key, made if `make` and there is none. It never
/// leaves the Keystore: in StrongBox where the phone has one, else in its TEE.
fn keystore_key<'a>(env: &mut jni::JNIEnv<'a>, make: bool) -> jni::errors::Result<Option<JObject<'a>>> {
    let kind = env.new_string("AndroidKeyStore")?;
    let ks = env
        .call_static_method("java/security/KeyStore", "getInstance", "(Ljava/lang/String;)Ljava/security/KeyStore;", &[JValue::Object(&kind)])?
        .l()?;
    env.call_method(&ks, "load", "(Ljava/security/KeyStore$LoadStoreParameter;)V", &[JValue::Object(&JObject::null())])?;
    let alias = env.new_string(STATE_KEY)?;
    let key = env
        .call_method(&ks, "getKey", "(Ljava/lang/String;[C)Ljava/security/Key;", &[JValue::Object(&alias), JValue::Object(&JObject::null())])?
        .l()?;
    if !key.is_null() || !make {
        return Ok((!key.is_null()).then_some(key));
    }
    // Not every phone has StrongBox, nor a lock screen to require; the next best is tried.
    let mut last = None;
    for (strongbox, unlocked) in [(true, true), (false, true), (false, false)] {
        match generate_key(env, strongbox, unlocked) {
            Ok(k) => return Ok(Some(k)),
            Err(e) => {
                if env.exception_check()? {
                    env.exception_clear()?;
                }
                last = Some(e);
            }
        }
    }
    Err(last.unwrap())
}

fn generate_key<'a>(env: &mut jni::JNIEnv<'a>, strongbox: bool, unlocked: bool) -> jni::errors::Result<JObject<'a>> {
    const BUILDER: &str = "android/security/keystore/KeyGenParameterSpec$Builder";
    const RETURNS: &str = "Landroid/security/keystore/KeyGenParameterSpec$Builder;";
    let alias = env.new_string(STATE_KEY)?;
    // PURPOSE_ENCRYPT | PURPOSE_DECRYPT
    let b = env.new_object(BUILDER, "(Ljava/lang/String;I)V", &[JValue::Object(&alias), JValue::Int(3)])?;
    let gcm = env.new_string("GCM")?;
    let modes = env.new_object_array(1, "java/lang/String", &gcm)?;
    env.call_method(&b, "setBlockModes", format!("([Ljava/lang/String;){RETURNS}"), &[JValue::Object(&modes)])?;
    let none = env.new_string("NoPadding")?;
    let paddings = env.new_object_array(1, "java/lang/String", &none)?;
    env.call_method(&b, "setEncryptionPaddings", format!("([Ljava/lang/String;){RETURNS}"), &[JValue::Object(&paddings)])?;
    env.call_method(&b, "setKeySize", format!("(I){RETURNS}"), &[JValue::Int(256)])?;
    if unlocked {
        env.call_method(&b, "setUnlockedDeviceRequired", format!("(Z){RETURNS}"), &[JValue::Bool(1)])?;
    }
    if strongbox {
        env.call_method(&b, "setIsStrongBoxBacked", format!("(Z){RETURNS}"), &[JValue::Bool(1)])?;
    }
    let spec = env.call_method(&b, "build", "()Landroid/security/keystore/KeyGenParameterSpec;", &[])?.l()?;
    let (aes, provider) = (env.new_string("AES")?, env.new_string("AndroidKeyStore")?);
    let kg = env
        .call_static_method(
            "javax/crypto/KeyGenerator",
            "getInstance",
            "(Ljava/lang/String;Ljava/lang/String;)Ljavax/crypto/KeyGenerator;",
            &[JValue::Object(&aes), JValue::Object(&provider)],
        )?
        .l()?;
    env.call_method(&kg, "init", "(Ljava/security/spec/AlgorithmParameterSpec;)V", &[JValue::Object(&spec)])?;
    env.call_method(&kg, "generateKey", "()Ljavax/crypto/SecretKey;", &[])?.l()
}

/// AES-GCM through the Keystore's key: `iv` given to decrypt, made by the Keystore to encrypt.
fn keystore_cipher<'a>(env: &mut jni::JNIEnv<'a>, key: &JObject, iv: Option<&[u8]>) -> jni::errors::Result<JObject<'a>> {
    let name = env.new_string("AES/GCM/NoPadding")?;
    let c = env
        .call_static_method("javax/crypto/Cipher", "getInstance", "(Ljava/lang/String;)Ljavax/crypto/Cipher;", &[JValue::Object(&name)])?
        .l()?;
    match iv {
        None => {
            env.call_method(&c, "init", "(ILjava/security/Key;)V", &[JValue::Int(1), JValue::Object(key)])?;
        }
        Some(iv) => {
            let iv = env.byte_array_from_slice(iv)?;
            let spec = env.new_object("javax/crypto/spec/GCMParameterSpec", "(I[B)V", &[JValue::Int(128), JValue::Object(&iv)])?;
            env.call_method(
                &c,
                "init",
                "(ILjava/security/Key;Ljava/security/spec/AlgorithmParameterSpec;)V",
                &[JValue::Int(2), JValue::Object(key), JValue::Object(&spec)],
            )?;
        }
    }
    Ok(c)
}

/// Run `input` through the cipher, and wipe the Java copies of it and of what comes out.
fn through<'a>(env: &mut jni::JNIEnv<'a>, c: &JObject, input: &[u8]) -> jni::errors::Result<zeroize::Zeroizing<Vec<u8>>> {
    let input = env.byte_array_from_slice(input)?;
    let out: jni::objects::JByteArray = env.call_method(c, "doFinal", "([B)[B", &[JValue::Object(&input)])?.l()?.into();
    let bytes = zeroize::Zeroizing::new(env.convert_byte_array(&out)?);
    env.call_static_method("java/util/Arrays", "fill", "([BB)V", &[JValue::Object(&out), JValue::Byte(0)])?;
    env.call_static_method("java/util/Arrays", "fill", "([BB)V", &[JValue::Object(&input), JValue::Byte(0)])?;
    Ok(bytes)
}

/// Wrap the state's key with the Keystore's, and keep it in `file`: the IV's length, the IV, and
/// the wrapped key.
pub fn wrap_state_key(file: &std::path::Path, key: &[u8]) -> Result<(), String> {
    let kept = with_activity(|env, _| {
        let k = keystore_key(env, true)?.expect("made");
        let c = keystore_cipher(env, &k, None)?;
        let iv: jni::objects::JByteArray = env.call_method(&c, "getIV", "()[B", &[])?.l()?.into();
        let iv = env.convert_byte_array(&iv)?;
        let wrapped = through(env, &c, key)?;
        let mut out = vec![iv.len() as u8];
        out.extend(&iv);
        out.extend(wrapped.iter());
        Ok(out)
    })
    .map_err(|e| format!("the Android Keystore refused to keep a key: {e}"))?;
    let tmp = file.with_extension("tmp");
    crate::settings::open_private(&tmp)
        .and_then(|mut f| {
            use std::io::Write;
            f.write_all(&kept)?;
            f.sync_all()
        })
        .and_then(|()| std::fs::rename(&tmp, file))
        .map_err(|e| e.to_string())
}

/// The state's key kept in `file`, unwrapped by the Keystore; `None` if none is kept.
pub fn unwrap_state_key(file: &std::path::Path) -> Result<Option<pigeonhole_protocols::secret::Secret>, String> {
    let kept = match std::fs::read(file) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let n = *kept.first().ok_or("the kept key is empty")? as usize;
    let (iv, wrapped) = kept[1..].split_at_checked(n).ok_or("the kept key is cut short")?;
    let key = with_activity(|env, _| {
        let Some(k) = keystore_key(env, false)? else { return Ok(None) };
        let c = keystore_cipher(env, &k, Some(iv))?;
        through(env, &c, wrapped).map(Some)
    })
    .map_err(|e| format!("the Android Keystore would not unwrap the key: {e}"))?
    .ok_or("the Android Keystore no longer has hushphone's key")?;
    Ok(Some(pigeonhole_protocols::secret::Secret::new(&key)))
}
