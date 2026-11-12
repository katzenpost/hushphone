//! The UI thread's side: the UI's callbacks become commands to the client or effects, and the
//! client's updates become models for the UI.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use pigeonhole_protocols::protocols::katzenqt1::client::{ChatView, LineView};
use pigeonhole_protocols::protocols::katzenqt1::engine::{ChatId, NoticeKind, parse_voucher_code, voucher_code};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use tokio::sync::mpsc::UnboundedSender;
use zeroize::Zeroizing;

use crate::backend::{self, Attachments, Cmd, Keys, Link, Source, Update};
use crate::contacts::Contacts;
use crate::effects::Picked;
use crate::model::{self, Files};
use crate::ping::{self, Pings};
use crate::radio::Radio;
use crate::settings::{Settings, ThemeChoice};
use crate::voice::{self, Voice};
use crate::sound;
use crate::{
    Actions, AppState, AppWindow, AttachItem, ChatItem, Connection, LineItem, MemberItem, Overlay, Theme, ThemePreference, effects, text,
};

/// Proof that an effect was asked for by the user, through the UI. Only this module can make
/// one, and only in the UI's callbacks.
pub struct UserIntent(());

impl UserIntent {
    fn new() -> UserIntent {
        UserIntent(())
    }

    #[cfg(test)]
    pub(crate) fn for_test() -> UserIntent {
        UserIntent(())
    }
}

#[derive(Default)]
struct ChatState {
    view: Option<ChatView>,
    lines: Vec<LineView>,
    /// The last line the user has seen.
    seen: usize,
}

/// Where things live and how to reach Pigeonhole.
pub struct Setup {
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
    /// The user's files directory, with `send/` and `received/`; unused where the system
    /// picks files instead.
    pub files_dir: PathBuf,
    pub source: Source,
    pub default_daemon: String,
    /// A name given on the command line, for a first start without asking.
    pub name: Option<String>,
    /// A katzenqt1 wire version given on the command line, saved over the saved one.
    pub wire_version: Option<u64>,
    /// Daemon addresses an older version saved as its default, to move to `default_daemon`.
    pub old_defaults: Vec<String>,
    /// How the state is kept at rest.
    pub keys: Keys,
    /// Linux: the webcam to scan QR codes with; the first there is when none.
    pub camera: Option<PathBuf>,
}

struct App {
    ui: slint::Weak<AppWindow>,
    setup: Setup,
    settings: Settings,
    /// Petnames and what was read, kept with the state; saved once the state's files are open.
    contacts: Contacts,
    cmd: Option<UnboundedSender<Cmd>>,
    effects: tokio::runtime::Runtime,
    chats: BTreeMap<ChatId, ChatState>,
    paths: HashMap<String, PathBuf>,
    /// The state's files, opened if it is sealed; from the client's thread once it has the key.
    attachments: Option<Attachments>,
    files: Files,
    current: Option<ChatId>,
    expanded: HashSet<usize>,
    voice: Voice,
    radio: Radio,
    pings: Pings,
    /// What the user picked last: a result for an older pick does not move the selection.
    ticket: u64,
    /// Which group a recording in progress is for.
    recording_for: Option<ChatId>,
    /// Set to stop the webcam scan in progress.
    scanning: Option<Arc<AtomicBool>>,
    /// Set to stop listening for a voucher by sound; and since when it listens.
    listening: Option<(Arc<AtomicBool>, Instant)>,
    sending_sound: bool,
    /// A picture picked to send, waiting for the user's choice: its group, the original, and smaller.
    picture: Option<(ChatId, Picked, Picked)>,
    chat_model: Rc<VecModel<ChatItem>>,
    pending_model: Rc<VecModel<ChatItem>>,
    line_model: Rc<VecModel<LineItem>>,
    member_model: Rc<VecModel<MemberItem>>,
    attach_model: Rc<VecModel<AttachItem>>,
    timer: slint::Timer,
    /// The sockets the sandbox allows, once it is on.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    net: Option<crate::sandbox::Net>,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

fn with_app<T>(f: impl FnOnce(&mut App) -> T) -> Option<T> {
    APP.with(|a| a.borrow_mut().as_mut().map(f))
}

/// Run `f` on the UI thread with the app.
fn later(f: impl FnOnce(&mut App) + Send + 'static) {
    let _ = slint::invoke_from_event_loop(move || {
        with_app(f);
    });
}

pub fn start(ui: &AppWindow, setup: Setup) {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    if let Err(e) = crate::harden::data_dir(&setup.data_dir) {
        eprintln!("hushphone: can't restrict {}: {e}", setup.data_dir.display());
    }
    let (mut settings, settings_note) = Settings::load(&setup.data_dir);
    if settings.name.is_empty()
        && let Some(n) = &setup.name
    {
        settings.name = text::name(n);
        let _ = settings.save(&setup.data_dir);
    }
    if setup.wire_version.is_some() && setup.wire_version != settings.wire_version {
        settings.wire_version = setup.wire_version;
        let _ = settings.save(&setup.data_dir);
    }
    if settings.daemon.is_empty() || setup.old_defaults.contains(&settings.daemon) {
        settings.daemon = setup.default_daemon.clone();
    }
    let chat_model = Rc::new(VecModel::default());
    let pending_model = Rc::new(VecModel::default());
    let line_model = Rc::new(VecModel::default());
    let member_model = Rc::new(VecModel::default());
    let attach_model = Rc::new(VecModel::default());
    let start = std::time::Instant::now();
    ui.global::<crate::Clock>().on_now(move || start.elapsed().as_millis() as i64);
    let st = ui.global::<AppState>();
    st.set_chats(ModelRc::from(chat_model.clone()));
    st.set_pending(ModelRc::from(pending_model.clone()));
    st.set_lines(ModelRc::from(line_model.clone()));
    st.set_members(ModelRc::from(member_model.clone()));
    st.set_attachable(ModelRc::from(attach_model.clone()));
    st.set_files_dir(if cfg!(target_os = "android") { String::new() } else { setup.files_dir.display().to_string() }.into());
    st.set_daemon(settings.daemon.clone().into());
    st.set_mesh(setup.source.is_mesh());
    st.set_choose_transport(cfg!(target_os = "android"));
    st.set_use_mesh(settings.transport == "reticulum");
    st.set_replica(settings.replica().into());
    st.set_screen_security(settings.screen_security);
    st.set_my_name(settings.name.clone().into());
    ui.global::<Theme>().set_preference(match settings.theme {
        ThemeChoice::System => ThemePreference::System,
        ThemeChoice::Light => ThemePreference::Light,
        ThemeChoice::Dark => ThemePreference::Dark,
    });
    let voice = Voice::new(setup.cache_dir.join("voice"));
    let app = App {
        ui: ui.as_weak(),
        setup,
        settings,
        contacts: Contacts::default(),
        cmd: None,
        effects: tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("effects runtime"),
        chats: BTreeMap::new(),
        paths: HashMap::new(),
        attachments: None,
        files: Files::default(),
        current: None,
        expanded: HashSet::new(),
        voice,
        radio: Radio::default(),
        pings: Pings::default(),
        ticket: 0,
        recording_for: None,
        scanning: None,
        listening: None,
        sending_sound: false,
        picture: None,
        chat_model,
        pending_model,
        line_model,
        member_model,
        attach_model,
        timer: slint::Timer::default(),
        #[cfg(any(target_os = "linux", target_os = "android"))]
        net: None,
    };
    APP.with(|a| *a.borrow_mut() = Some(app));
    wire(ui);
    with_app(|app| {
        if let Some(why) = settings_note {
            app.notice(&why, true);
        }
        if app.settings.name.is_empty() {
            if let Some(ui) = app.ui.upgrade() {
                ui.set_overlay(Overlay::FirstRun);
            }
        } else {
            app.connect();
        }
    });
}

fn wire(ui: &AppWindow) {
    let a = ui.global::<Actions>();
    a.on_choose_name(|name| {
        with_app(|app| {
            app.settings.name = text::name(&name);
            let _ = app.settings.save(&app.setup.data_dir);
            app.state(|s| s.set_my_name(app.settings.name.clone().into()));
            app.connect();
        });
    });
    a.on_select_chat(|id| {
        with_app(|app| app.select(id.to_string()));
    });
    a.on_send_text(|t| {
        with_app(|app| {
            if let Some(id) = app.current.clone() {
                app.send(Cmd::Send(id, t.to_string()));
            }
        });
    });
    a.on_create_group(|name| {
        with_app(|app| {
            app.ticket += 1;
            app.send(Cmd::Create(name.trim().to_string(), app.ticket));
        });
    });
    a.on_join_group(|name| {
        with_app(|app| {
            app.ticket += 1;
            app.send(Cmd::Join(name.trim().to_string(), app.ticket));
        });
    });
    a.on_invite(|code| {
        with_app(|app| {
            if let Some(id) = app.current.clone() {
                app.send(Cmd::Invite(id, code.split_whitespace().collect()));
            }
        });
    });
    a.on_scan(|| {
        with_app(|app| {
            if let Some(id) = app.current.clone() {
                app.send(Cmd::Scan(id));
            }
        });
    });
    a.on_leave(|| {
        with_app(|app| {
            if let Some(id) = app.current.take() {
                app.send(Cmd::Leave(id));
                app.render_current();
            }
        });
    });
    a.on_clear(|| {
        with_app(|app| {
            if let Some(id) = app.current.clone() {
                app.send(Cmd::Clear(id));
            }
        });
    });
    a.on_rename(|to| {
        with_app(|app| {
            if let Some(id) = app.current.clone() {
                app.send(Cmd::Rename(id, to.trim().to_string()));
            }
        });
    });
    a.on_remove_member(|key| {
        with_app(|app| {
            if let (Some(id), Ok(key)) = (app.current.clone(), hex::decode(key.as_str())) {
                app.send(Cmd::Remove(id, key));
            }
        });
    });
    a.on_set_petname(|key, name| {
        with_app(|app| {
            app.contacts.set_petname(&key, &name);
            if let Err(e) = app.save_contacts() {
                app.notice(&format!("Couldn't save the name: {e}"), true);
            }
            app.render_all();
        });
    });
    a.on_save_settings(|daemon, mesh, replica, secure| {
        with_app(|app| {
            app.settings.daemon = daemon.trim().to_string();
            if cfg!(target_os = "android") {
                let replica = replica.trim();
                if mesh && crate::mesh::host_port(replica).is_err() {
                    app.notice(&format!("The replica's address is host:port, not {replica:?}."), true);
                    return;
                }
                app.settings.transport = if mesh { "reticulum" } else { "" }.into();
                app.settings.replica = replica.into();
                app.settings.screen_security = secure;
                #[cfg(target_os = "android")]
                crate::android::set_screen_security(secure);
                app.state(|s| {
                    s.set_use_mesh(mesh);
                    s.set_replica(replica.into());
                    s.set_screen_security(secure);
                });
                let now = match &app.setup.source {
                    Source::Mesh(a, _) => Some(a.as_str()),
                    _ => None,
                };
                if now != mesh.then_some(replica) {
                    app.notice("The new transport is used the next time hushphone starts.", false);
                }
            }
            #[cfg(any(target_os = "linux", target_os = "android"))]
            if app.net.is_some_and(|n| !n.allows(crate::sandbox::Net::for_daemon(&app.settings.daemon))) {
                app.notice("This app may only reach kpclientd over a unix socket until it restarts.", true);
            }
            if let Some(ui) = app.ui.upgrade() {
                app.settings.theme = match ui.global::<Theme>().get_preference() {
                    ThemePreference::Light => ThemeChoice::Light,
                    ThemePreference::Dark => ThemeChoice::Dark,
                    _ => ThemeChoice::System,
                };
            }
            if let Err(e) = app.settings.save(&app.setup.data_dir) {
                app.notice(&format!("Couldn't save the settings: {e}"), true);
            }
        });
    });
    a.on_dismiss_notice(|| {
        with_app(|app| app.notice("", false));
    });
    a.on_expand(|id| {
        with_app(|app| {
            app.expanded.insert(id as usize);
            app.render_current();
        });
    });
    a.on_copy_code(|| {
        with_app(|app| {
            let code = app.current_view().map(|v| v.code.clone()).unwrap_or_default();
            if code.is_empty() {
                return;
            }
            match effects::copy_text(&UserIntent::new(), &code) {
                Ok(()) => app.notice("Voucher code copied.", false),
                Err(e) => app.notice(&format!("Couldn't copy: {e}"), true),
            }
        });
    });
    a.on_paste_code(|| {
        with_app(|_| effects::paste_text(&UserIntent::new()).unwrap_or_default())
            .unwrap_or_default()
            .into()
    });
    a.on_scan_code(|| {
        with_app(|app| app.scan_code(UserIntent::new()));
    });
    a.on_stop_scan(|| {
        with_app(|app| app.stop_scan());
    });
    a.on_send_sound(|| {
        with_app(|app| app.send_sound(UserIntent::new()));
    });
    a.on_listen(|| {
        with_app(|app| app.listen(UserIntent::new()));
    });
    a.on_stop_listen(|| {
        with_app(|app| app.stop_listen());
    });
    a.on_open_attach(|| {
        with_app(|app| app.open_attach(UserIntent::new()));
    });
    a.on_attach_file(|name| {
        with_app(|app| app.attach(UserIntent::new(), name.to_string()));
    });
    a.on_send_picture(|original| {
        with_app(|app| app.send_picture(original));
    });
    a.on_drop_picture(|| {
        with_app(|app| app.picture = None);
    });
    a.on_save_file(|line| {
        with_app(|app| app.save(UserIntent::new(), line as usize));
    });
    a.on_toggle_voice(|line| {
        with_app(|app| app.toggle_voice(line as usize));
    });
    a.on_ping(|| {
        with_app(|app| app.ping());
    });
    a.on_toggle_radio(|| {
        with_app(|app| app.toggle_radio());
    });
    a.on_ptt_start(|| {
        with_app(|app| app.ptt_start(UserIntent::new()));
    });
    a.on_ptt_stop(|| {
        with_app(|app| app.ptt_stop());
    });
    a.on_ptt_cancel(|| {
        with_app(|app| {
            app.voice.cancel();
            app.recording_for = None;
            app.state(|s| s.set_recording(false));
            app.pump();
        });
    });
}

impl App {
    fn state(&self, f: impl FnOnce(AppState)) {
        if let Some(ui) = self.ui.upgrade() {
            f(ui.global::<AppState>());
        }
    }

    fn notice(&self, text: &str, error: bool) {
        self.state(|s| {
            s.set_notice(text.into());
            s.set_notice_error(error);
        });
    }

    fn send(&self, c: Cmd) {
        match &self.cmd {
            Some(tx) => {
                let _ = tx.send(c);
            }
            None => self.notice("Not connected yet.", true),
        }
    }

    fn connect(&mut self) {
        if self.cmd.is_some() {
            return;
        }
        let source = match &self.setup.source {
            // An address on the command line wins over the one saved.
            Source::Daemon(a) if a.is_empty() => Source::Daemon(self.settings.daemon.clone()),
            s => s.clone(),
        };
        #[cfg(any(target_os = "linux", target_os = "android"))]
        if self.net.is_none() {
            use crate::sandbox::{self, Net};
            let net = match &source {
                Source::Daemon(a) => Net::for_daemon(a),
                s if s.is_mesh() => Net::Tcp,
                _ => Net::Unix,
            };
            // Without the filter the app does not go on: HUSHPHONE_SANDBOX=off is the way
            // to run without it, on purpose.
            if let Err(e) = sandbox::enter(net) {
                self.notice(&format!("Couldn't sandbox the app, so it stays off the daemon: {e}. Set HUSHPHONE_SANDBOX=off to run without the sandbox."), true);
                return;
            }
            self.net = Some(net);
        }
        let params = backend::Params {
            source,
            state_dir: self.setup.data_dir.join("state"),
            name: self.settings.name.clone(),
            keys: self.setup.keys.clone(),
            wire_version: self.settings.wire_version.filter(|v| matches!(v, 3 | 4)).unwrap_or(4),
        };
        self.cmd = Some(backend::spawn(params, |u| later(move |app| app.apply(u))));
    }

    fn joined(&self, id: &str) -> bool {
        self.chats.get(id).and_then(|c| c.view.as_ref()).is_some_and(|v| v.joined)
    }

    fn current_view(&self) -> Option<&ChatView> {
        self.chats.get(self.current.as_ref()?)?.view.as_ref()
    }

    fn apply(&mut self, u: Update) {
        match u {
            Update::Files(a, unsealed) => {
                match Contacts::load(&a) {
                    Ok(c) => self.contacts = c,
                    Err(e) => self.notice(&format!("Couldn't read your names for members: {e}"), true),
                }
                self.attachments = Some(a);
                for id in self.contacts.radio.clone() {
                    let last = self.chats.get(&id).map(|c| c.lines.last().map_or(0, |l| l.line.id));
                    self.radio.set(&id, true, last);
                }
                // What an older version kept in the settings moves in with the state.
                if self.contacts.adopt(&mut self.settings) {
                    if let Err(e) = self.save_contacts() {
                        self.notice(&format!("Couldn't move your names for members: {e}"), true);
                    } else {
                        let _ = self.settings.save(&self.setup.data_dir);
                    }
                }
                self.state(|s| s.set_at_rest_warning(unsealed.map_or_else(String::new, |why| {
                    format!("Your chats are stored unencrypted on this device: {why}.")
                }).into()));
            }
            Update::Locked(why) => {
                self.state(|s| s.set_connection(Connection::Unreachable));
                self.notice(&why, true);
            }
            Update::Link(l) => {
                self.state(|s| {
                    s.set_connection(match l {
                        Link::Connecting => Connection::Connecting,
                        Link::Online => Connection::Online,
                        Link::Offline => Connection::Offline,
                        Link::Unreachable => Connection::Unreachable,
                    })
                });
                if l == Link::Online {
                    self.state(|s| {
                        if s.get_notice().starts_with("Can't reach kpclientd") {
                            s.set_notice("".into());
                        }
                    });
                }
            }
            Update::Chat { view, from, lines, dropped, paths, files } => {
                let id = view.id.clone();
                self.paths.extend(paths);
                for (file, info) in files {
                    self.files.types.insert(file.clone(), info.kind);
                    if let Some(d) = info.duration {
                        self.files.durations.insert(file, d);
                    }
                }
                let seen = self.contacts.seen.get(&id).copied().unwrap_or(0);
                let c = self.chats.entry(id.clone()).or_insert_with(|| ChatState { seen, ..Default::default() });
                let joining = c.view.as_ref().is_none_or(|v| !v.joined);
                let before = c.lines.last().map(|l| l.line.id);
                c.view = Some(view);
                if from != usize::MAX {
                    c.lines.retain(|l| l.line.id < from && !dropped.contains(&l.line.id));
                    c.lines.extend(lines);
                } else {
                    c.lines.retain(|l| !dropped.contains(&l.line.id));
                }
                if joining {
                    let fresh = c.lines.iter().filter(|l| before.is_none_or(|b| l.line.id > b));
                    for n in fresh.filter_map(|l| l.line.notice.as_ref()).filter_map(model::join_log) {
                        log_join(&id, &n);
                    }
                }
                let files = &self.files;
                self.radio.arrived(&id, &c.lines, |a| files.types.contains_key(&a.file));
                let arrived = self.pings.arrived(&id, &c.lines);
                let pinged = arrived.iter().any(|n| n.kind == NoticeKind::Pinged);
                let arrived = arrived.last().map(|n| {
                    format!("{} in {}", model::notice_text(n), c.view.as_ref().map(|v| text::name(&v.name)).unwrap_or_default())
                });
                if self.current.as_deref() == Some(id.as_str()) {
                    c.seen = c.lines.last().map_or(0, |l| l.line.id);
                    self.remember_seen(&id);
                }
                self.pump();
                if let Some(text) = arrived {
                    self.notice(&text, false);
                }
                if pinged {
                    self.chime(ping::RECEIVED);
                }
                self.render_chats();
                if self.current.as_deref() == Some(id.as_str()) {
                    self.render_current();
                }
            }
            Update::Removed(id) => {
                self.chats.remove(&id);
                self.radio.set(&id, false, None);
                let had = self.contacts.radio.remove(&id) | self.contacts.played.remove(&id).is_some();
                if self.contacts.seen.remove(&id).is_some() || had {
                    let _ = self.save_contacts();
                }
                if self.current.as_deref() == Some(id.as_str()) {
                    self.current = None;
                    if let Some(ui) = self.ui.upgrade() {
                        ui.set_show_conversation(false);
                    }
                }
                self.render_all();
            }
            Update::Made(id, ticket) => {
                if ticket == self.ticket {
                    self.select(id);
                    if let Some(ui) = self.ui.upgrade() {
                        ui.set_show_conversation(true);
                    }
                }
            }
            Update::Failed(e) => self.notice(&e, true),
            Update::Stopped(e) => {
                self.state(|s| s.set_connection(Connection::Unreachable));
                self.notice(&format!("The client stopped: {e}. Opening it again."), true);
            }
        }
    }

    fn save_contacts(&self) -> Result<(), String> {
        match &self.attachments {
            Some(a) => self.contacts.save(a),
            None => Err("the state is not open yet".into()),
        }
    }

    /// Show what is in the send directory, for the user to pick from; where the system has a
    /// picker of its own, open that instead.
    fn open_attach(&mut self, intent: UserIntent) {
        let Some(chat) = self.current.clone() else { return };
        if !self.joined(&chat) {
            return;
        }
        if cfg!(target_os = "android") {
            self.attach(intent, String::new());
            return;
        }
        let dir = self.setup.files_dir.clone();
        self.effects.spawn(async move {
            let r = effects::list_files(&intent, &dir);
            later(move |app| match r {
                Ok(entries) => {
                    let items = entries
                        .into_iter()
                        .map(|e| AttachItem {
                            name: e.name.into(),
                            size: text::file_size(e.size as usize).into(),
                            too_large: e.size > effects::FILE_MAX,
                        })
                        .collect();
                    sync(&app.attach_model, items);
                    if let Some(ui) = app.ui.upgrade() {
                        ui.set_overlay(Overlay::Attach);
                    }
                }
                Err(e) => app.notice(&format!("Couldn't read {}: {e}", dir.join(effects::SEND).display()), true),
            });
        });
    }

    /// Read a voucher code from a QR code, off this thread, into the field a pasted one goes in.
    /// Pressed again while the webcam looks, it stops.
    fn scan_code(&mut self, intent: UserIntent) {
        if self.scanning.is_some() {
            self.stop_scan();
            return;
        }
        self.state(|s| s.set_scanned_code("".into()));
        let stop = Arc::new(AtomicBool::new(false));
        self.scanning = Some(stop.clone());
        let (dir, camera) = (self.setup.files_dir.clone(), self.setup.camera.clone());
        self.effects.spawn(async move {
            let mine = stop.clone();
            let r = tokio::task::spawn_blocking(move || {
                let mut preview = |frame| later(move |app| app.state(|s| s.set_scan_preview(slint::Image::from_rgb8(frame))));
                effects::scan_code(&intent, &dir, camera.as_deref(), &stop, &mut preview)
            })
            .await
            .unwrap_or_else(|e| Err(e.to_string()));
            later(move |app| {
                if app.scanning.as_ref().is_some_and(|s| Arc::ptr_eq(s, &mine)) {
                    app.scanning = None;
                }
                app.state(|s| s.set_scan_preview(slint::Image::default()));
                match r {
                    Ok(Some(code)) => app.state(|s| s.set_scanned_code(code.into())),
                    Ok(None) => {}
                    Err(e) => app.notice(&e, true),
                }
            });
        });
    }

    fn stop_scan(&mut self) {
        if let Some(stop) = self.scanning.take() {
            stop.store(true, Ordering::Relaxed);
        }
    }

    /// Play the current voucher by sound, off this thread.
    fn send_sound(&mut self, intent: UserIntent) {
        let Some(v) = self.current_view().and_then(|v| parse_voucher_code(&v.code)) else { return };
        if self.sending_sound {
            return;
        }
        self.voice.stop_playing();
        self.sending_sound = true;
        self.state(|s| s.set_sending_sound(true));
        self.effects.spawn(async move {
            let v: [u8; sound::PAYLOAD] = v.try_into().expect("32 bytes");
            let r = tokio::task::spawn_blocking(move || sound::send(&intent, &v)).await.unwrap_or_else(|e| Err(e.to_string()));
            later(move |app| {
                app.sending_sound = false;
                app.state(|s| s.set_sending_sound(false));
                if let Err(e) = r {
                    app.notice(&format!("Couldn't send it by sound: {e}"), true);
                }
            });
        });
    }

    /// Listen for a voucher by sound, off this thread, into the field a pasted one goes in.
    /// Pressed again, it stops.
    fn listen(&mut self, intent: UserIntent) {
        if self.listening.is_some() {
            self.stop_listen();
            return;
        }
        #[cfg(target_os = "android")]
        if !crate::android::microphone_allowed(&intent) {
            self.state(|s| s.set_sound_note("hushphone needs the microphone to listen. Allow it and choose Listen again.".into()));
            return;
        }
        let stop = Arc::new(AtomicBool::new(false));
        self.listening = Some((stop.clone(), Instant::now()));
        self.state(|s| {
            s.set_scanned_code("".into());
            s.set_listening(true);
        });
        self.tick();
        self.effects.spawn(async move {
            let mine = stop.clone();
            let r = tokio::task::spawn_blocking(move || sound::listen(&intent, &stop)).await.unwrap_or_else(|e| Err(e.to_string()));
            later(move |app| {
                if !app.listening.as_ref().is_some_and(|(s, _)| Arc::ptr_eq(s, &mine)) {
                    return;
                }
                app.listening = None;
                let note = match r {
                    Ok(Some(v)) => {
                        app.state(|s| s.set_scanned_code(voucher_code(&v).into()));
                        format!("Heard a voucher code, check {}. If their screen shows the same, choose Add member.", sound::check(&v))
                    }
                    Ok(None) => String::new(),
                    Err(e) => format!("Not heard: {e}. Hold the devices closer and choose Listen again."),
                };
                app.state(|s| {
                    s.set_listening(false);
                    s.set_sound_note(note.into());
                });
            });
        });
    }

    fn stop_listen(&mut self) {
        if let Some((stop, _)) = self.listening.take() {
            stop.store(true, Ordering::Relaxed);
        }
        self.state(|s| {
            s.set_listening(false);
            s.set_sound_note("".into());
        });
    }

    /// Send the file `name` from the send directory to the current group; on Android, the
    /// file the system's picker hands over.
    fn attach(&mut self, intent: UserIntent, name: String) {
        let Some(chat) = self.current.clone() else { return };
        if !self.joined(&chat) {
            return;
        }
        let dir = self.setup.files_dir.clone();
        self.effects.spawn(async move {
            let r = tokio::task::spawn_blocking(move || {
                #[cfg(not(target_os = "android"))]
                let r = effects::pick_file(&intent, &dir, &name).map(Some);
                #[cfg(target_os = "android")]
                let r = {
                    let _ = (&dir, &name);
                    crate::android::pick_document(&intent)
                };
                r.map(|f| f.map(|f| (crate::shrink::shrink(&f), f)))
            })
            .await
            .unwrap_or_else(|e| Err(e.to_string()));
            later(move |app| match r {
                // The group the file was picked for must still be there to send it.
                Ok(Some(_)) if !app.joined(&chat) => app.notice("The group is gone: the file was not sent.", true),
                Ok(Some((Some(small), f))) => {
                    let sizes = format!("{} → {}", text::file_size(f.bytes.len()), text::file_size(small.bytes.len()));
                    app.state(|s| {
                        s.set_picture_name(f.name.as_str().into());
                        s.set_picture_sizes(sizes.into());
                    });
                    app.picture = Some((chat, f, small));
                    if let Some(ui) = app.ui.upgrade() {
                        ui.set_overlay(Overlay::Picture);
                    }
                }
                Ok(Some((None, f))) => app.send(Cmd::SendFile(chat, f.name, f.bytes)),
                Ok(None) => {}
                Err(e) => app.notice(&e, true),
            });
        });
    }

    /// Send the picture picked, as the user chose: smaller, or the original untouched.
    fn send_picture(&mut self, original: bool) {
        let Some((chat, f, small)) = self.picture.take() else { return };
        if !self.joined(&chat) {
            return self.notice("The group is gone: the file was not sent.", true);
        }
        let f = if original { f } else { small };
        self.send(Cmd::SendFile(chat, f.name, f.bytes));
    }

    /// Save a line's attachment, read and written off this thread.
    fn save(&mut self, intent: UserIntent, line: usize) {
        let Some((file, name)) = self.current.as_ref().and_then(|c| self.attachment(c, line)) else { return };
        let Some(att) = self.attachments.clone() else { return };
        let dir = self.setup.files_dir.clone();
        self.effects.spawn(async move {
            let r = tokio::task::spawn_blocking(move || {
                let bytes = att.read(&file).map_err(|e| e.to_string())?;
                #[cfg(not(target_os = "android"))]
                {
                    effects::save_file(&intent, &dir, &name, &bytes).map(|p| Some(format!("Saved as {}.", p.display())))
                }
                #[cfg(target_os = "android")]
                {
                    let _ = &dir;
                    crate::android::create_document(&intent, &name, &bytes).map(|saved| saved.then(|| "Saved.".to_string()))
                }
            })
            .await
            .unwrap_or_else(|e| Err(e.to_string()));
            later(move |app| match r {
                Ok(Some(said)) => app.notice(&said, false),
                Ok(None) => {}
                Err(e) => app.notice(&format!("Couldn't save: {e}"), true),
            });
        });
    }

    fn select(&mut self, id: String) {
        if !self.chats.contains_key(&id) {
            return;
        }
        self.ticket += 1;
        if self.current.as_ref() != Some(&id) {
            self.expanded.clear();
        }
        if let Some(c) = self.chats.get_mut(&id) {
            c.seen = c.lines.last().map_or(0, |l| l.line.id);
        }
        self.remember_seen(&id);
        self.current = Some(id);
        self.render_all();
    }

    fn remember_seen(&mut self, id: &str) {
        let Some(seen) = self.chats.get(id).map(|c| c.seen) else { return };
        if self.contacts.seen.get(id) != Some(&seen) {
            self.contacts.seen.insert(id.to_string(), seen);
            let _ = self.save_contacts();
        }
    }

    /// A line's attachment: its file in the state, and the name to give it; `None` until it has
    /// arrived.
    fn attachment(&self, chat: &str, line: usize) -> Option<(String, String)> {
        let c = self.chats.get(chat)?;
        let l = c.lines.iter().find(|l| l.line.id == line)?;
        let a = l.line.attachment.as_ref()?;
        if !self.paths.get(&a.file)?.exists() {
            return None;
        }
        Some((a.file.clone(), text::identifier(&a.name, 200)))
    }

    fn render_all(&mut self) {
        self.render_chats();
        self.render_current();
    }

    fn render_chats(&self) {
        let now = chrono::Local::now();
        let mut items: Vec<(i64, ChatItem)> = self
            .chats
            .values()
            .filter_map(|c| {
                let v = c.view.as_ref()?;
                let who = model::names(v, &self.contacts, &self.settings.name);
                let last = c.lines.iter().rev().find(|l| l.line.notice.is_none());
                let unread = if self.current.as_deref() == Some(v.id.as_str()) {
                    0
                } else {
                    c.lines.iter().filter(|l| l.line.id > c.seen && !l.mine && l.line.notice.is_none()).count()
                };
                let when = c.lines.last().map_or(i64::MAX, |l| l.line.time);
                let mut item = model::chat_item(v, last, unread, &who, now);
                if item.joining {
                    // The latest failure stays on the row while joining, not only in a notice that goes.
                    item.preview = model::joining_note(&c.lines).into();
                }
                Some((when, item))
            })
            .collect();
        items.sort_by_key(|i| std::cmp::Reverse(i.0));
        let items: Vec<ChatItem> = items.into_iter().map(|(_, i)| i).collect();
        sync(&self.pending_model, items.iter().filter(|i| i.joining).cloned().collect());
        sync(&self.chat_model, items);
    }

    fn render_current(&mut self) {
        let Some(ui) = self.ui.upgrade() else { return };
        let st = ui.global::<AppState>();
        let Some(c) = self.current.as_ref().and_then(|id| self.chats.get(id)) else {
            st.set_current("".into());
            st.set_current_name("".into());
            sync(&self.line_model, Vec::new());
            sync(&self.member_model, Vec::new());
            return;
        };
        let Some(v) = c.view.as_ref() else { return };
        let who = model::names(v, &self.contacts, &self.settings.name);
        let mut items = model::line_items(&c.lines, &who, &self.files, &self.expanded, chrono::Local::now());
        if let Some(p) = &self.voice.playing
            && p.chat == v.id
            && let Some(i) = items.iter_mut().find(|i| i.id as usize == p.line)
        {
            i.playing = true;
            i.progress = (p.started.elapsed().as_secs_f32() / p.length.as_secs_f32().max(0.001)).min(1.0);
        }
        let played = self.contacts.played.get(&v.id);
        for i in items.iter_mut().filter(|i| i.kind == crate::LineKind::Voice && !i.mine) {
            i.unplayed = !played.is_some_and(|p| p.contains(&(i.id as usize)));
        }
        st.set_current_radio(self.radio.is_on(&v.id));
        let talker = self.talker(&v.id);
        st.set_radio_receiving(talker.is_some());
        st.set_radio_talker(talker.unwrap_or_default().into());
        st.set_current(v.id.clone().into());
        st.set_current_name(text::name(&v.name).into());
        st.set_current_joined(v.joined);
        st.set_current_members(v.members.len() as i32);
        st.set_code(group_code(&v.code).into());
        st.set_code_qr(qr_image(&v.code));
        st.set_code_check(parse_voucher_code(&v.code).map(|v| sound::check(&v)).unwrap_or_default().into());
        st.set_minting(v.minting);
        st.set_joining_note(if v.joined { String::new() } else { model::joining_note(&c.lines) }.into());
        st.set_adding(v.adding);
        sync(&self.line_model, items);
        sync(&self.member_model, model::member_items(v, &who));
    }

    fn toggle_voice(&mut self, line: usize) {
        let Some(chat) = self.current.clone() else { return };
        if self.voice.playing.as_ref().is_some_and(|p| p.chat == chat && p.line == line) {
            self.voice.stop_playing();
            self.pump();
            self.render_current();
            return;
        }
        self.play(&chat, line);
    }

    /// Play a received clip and mark it played.
    fn play(&mut self, chat: &str, line: usize) {
        let bytes = self.attachment(chat, line).and_then(|(file, _)| self.attachments.as_ref()?.read(&file).ok());
        let Some(bytes) = bytes else {
            self.notice("That voice message hasn't arrived yet.", true);
            return;
        };
        if let Err(e) = self.voice.play(chat, line, bytes) {
            self.notice(&format!("Couldn't play it: {e}"), true);
            return;
        }
        if self.contacts.played.entry(chat.to_string()).or_default().insert(line) {
            let _ = self.save_contacts();
        }
        self.tick();
        self.render_current();
    }

    /// Who the clip playing is from, and from which group when it is not `current`.
    fn talker(&self, current: &str) -> Option<String> {
        let p = self.voice.playing.as_ref()?;
        let c = self.chats.get(&p.chat)?;
        let v = c.view.as_ref()?;
        let l = c.lines.iter().find(|l| l.line.id == p.line)?;
        let who = model::names(v, &self.contacts, &self.settings.name);
        let from = who.get(&l.line.sender).map_or_else(|| text::name(&l.from), |w| w.shown.clone());
        Some(if p.chat == current { from } else { format!("{from} in {}", text::name(&v.name)) })
    }

    /// Radio mode: play the next clip queued, once nothing is playing or recording.
    fn pump(&mut self) {
        if self.voice.playing.is_some() || self.voice.recording().is_some() {
            return;
        }
        while self.voice.playing.is_none()
            && let Some((chat, line)) = self.radio.pop()
        {
            self.play(&chat, line);
        }
    }

    /// Ask the current group's members to play their ping sound, at most once every few seconds.
    fn ping(&mut self) {
        let Some(chat) = self.current.clone() else { return };
        if !self.joined(&chat) || !self.pings.may_send(Instant::now()) {
            return;
        }
        self.send(Cmd::Ping(chat));
        self.chime(ping::SENT);
    }

    /// Play a ping sound, off this thread.
    fn chime(&self, wav: &'static [u8]) {
        self.effects.spawn(async move {
            if let Ok(Err(e)) = tokio::task::spawn_blocking(move || sound::chime(wav)).await {
                eprintln!("hushphone: ping sound: {e}");
            }
        });
    }

    fn toggle_radio(&mut self) {
        let Some(chat) = self.current.clone() else { return };
        let on = !self.radio.is_on(&chat);
        let last = self.chats.get(&chat).map(|c| c.lines.last().map_or(0, |l| l.line.id));
        self.radio.set(&chat, on, last);
        if on {
            self.contacts.radio.insert(chat);
        } else {
            self.contacts.radio.remove(&chat);
        }
        if let Err(e) = self.save_contacts() {
            self.notice(&format!("Couldn't save radio mode: {e}"), true);
        }
        self.render_current();
    }

    fn ptt_start(&mut self, intent: UserIntent) {
        let Some(chat) = self.current.clone() else { return };
        if !self.joined(&chat) {
            return;
        }
        #[cfg(target_os = "android")]
        if !crate::android::microphone_allowed(&intent) {
            self.notice("hushphone needs the microphone to record. Allow it and press again.", true);
            return;
        }
        match self.voice.start(&intent) {
            Ok(()) => {
                self.recording_for = Some(chat);
                self.state(|s| {
                    s.set_notice("".into());
                    s.set_recording(true);
                    s.set_record_time("0:00".into());
                });
                self.tick();
            }
            Err(e) => self.notice(&format!("Couldn't open the microphone: {e}"), true),
        }
    }

    fn ptt_stop(&mut self) {
        self.state(|s| s.set_recording(false));
        let Some(chat) = self.recording_for.take() else { return };
        self.voice.stop(move |r| {
            later(move |app| match r {
                // The copy for the client is wiped once it has taken it.
                Ok(Some(clip)) if app.joined(&chat) => {
                    app.send(Cmd::SendFile(chat, clip.name, Zeroizing::new(clip.audio.bytes().to_vec())))
                }
                Ok(Some(_)) => app.notice("The group is gone: the voice message was not sent.", true),
                Ok(None) => app.notice("Too short to send: hold the button while you talk.", false),
                Err(e) => app.notice(&format!("Couldn't record: {e}"), true),
            });
        });
        self.pump();
    }

    /// While recording or playing, update the time shown, and stop at the longest clip.
    fn tick(&self) {
        if self.timer.running() {
            return;
        }
        self.timer.start(slint::TimerMode::Repeated, Duration::from_millis(100), || {
            with_app(|app| {
                let mut busy = false;
                if let Some((_, since)) = &app.listening {
                    busy = true;
                    let left = sound::LISTEN.saturating_sub(since.elapsed()).as_secs_f32().ceil();
                    let note = format!("Listening… {left} s. Hold this device 10–30 cm from theirs while they choose Send by sound.");
                    app.state(|s| s.set_sound_note(note.into()));
                }
                if let Some(d) = app.voice.recording() {
                    busy = true;
                    app.state(|s| s.set_record_time(voice::show_duration(d).into()));
                    if d >= voice::MAX_CLIP {
                        app.ptt_stop();
                    }
                }
                if app.voice.playing.is_some() {
                    busy = true;
                    if app.voice.progress().is_none() {
                        app.pump();
                        busy = app.voice.recording().is_some() || app.voice.playing.is_some();
                    }
                    app.render_current();
                }
                if !busy {
                    app.timer.stop();
                }
            });
        });
    }
}

/// A line about a join to logcat, or stderr off Android, under `hushphone-join`: the group by a
/// short hash of its local id, so the log names no group.
fn log_join(chat: &str, what: &str) {
    use sha2::Digest;
    let line = format!("group {} {what}", &hex::encode(sha2::Sha256::digest(chat))[..8]);
    #[cfg(target_os = "android")]
    crate::android::log("hushphone-join", &line);
    #[cfg(not(target_os = "android"))]
    eprintln!("hushphone-join: {line}");
}

/// A voucher code in groups of eight characters, for reading out.
fn group_code(code: &str) -> String {
    code.as_bytes()
        .chunks(8)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The voucher code as a QR code, one pixel per module, dark on light with a quiet zone of four:
/// byte mode, level M, the smallest version that fits, as the T-Deck shows it.
fn qr_image(code: &str) -> slint::Image {
    use qrcodegen::{QrCode, QrCodeEcc, QrSegment, Version};
    let segs = [QrSegment::make_bytes(code.as_bytes())];
    let Ok(qr) = QrCode::encode_segments_advanced(&segs, QrCodeEcc::Medium, Version::MIN, Version::MAX, None, false) else {
        return slint::Image::default();
    };
    let n = qr.size() + 8;
    let mut buf = slint::SharedPixelBuffer::<slint::Rgb8Pixel>::new(n as u32, n as u32);
    for (i, p) in buf.make_mut_slice().iter_mut().enumerate() {
        let (x, y) = (i as i32 % n - 4, i as i32 / n - 4);
        let v = if qr.get_module(x, y) { 0 } else { 255 };
        *p = slint::Rgb8Pixel::new(v, v, v);
    }
    slint::Image::from_rgb8(buf)
}

/// Make `model` hold `new`, touching only the rows that changed.
fn sync<T: Clone + PartialEq + 'static>(model: &VecModel<T>, new: Vec<T>) {
    let old = model.row_count();
    for (i, item) in new.iter().enumerate() {
        if i < old {
            if model.row_data(i).as_ref() != Some(item) {
                model.set_row_data(i, item.clone());
            }
        } else {
            model.push(item.clone());
        }
    }
    for i in (new.len()..old).rev() {
        model.remove(i);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_grouped_for_reading() {
        assert_eq!(group_code("AQIDBAUGBwgJ"), "AQIDBAUG BwgJ");
        assert_eq!(group_code(""), "");
    }

    #[test]
    fn qr_decodes_to_the_code() {
        let code = "AQIDBAUGBwgJCgsMDQ4PEBESExQVFhcYGRobHB0eHyAhIiMkJSYnKCkqKywtLi8wMTIzNDU2Nzg5Ojs8PT4/QEFCQ0RFRkdISUpLTE1OT1BRUlNUVVZXWFlaW1xdXl9gYWJjZGVmZ2hpamtsbW5vcHFyc3R1dnd4eXp7fH0=";
        let img = qr_image(code).to_rgb8().unwrap();
        let n = img.width() as usize;
        // 53 modules is version 9, the smallest that holds 168 bytes at level M; plus the quiet zone.
        assert_eq!(n, 53 + 8);
        let px = img.as_slice();
        assert!(px[..4 * n].iter().all(|p| p.r == 255), "quiet zone");
        let scale = 4;
        let mut prep = rqrr::PreparedImage::prepare_from_greyscale(n * scale, n * scale, |x, y| px[y / scale * n + x / scale].r);
        let grids = prep.detect_grids();
        assert_eq!(grids.len(), 1);
        let (meta, text) = grids[0].decode().unwrap();
        assert_eq!(text, code);
        assert_eq!(meta.version.0, 9);
        assert_eq!(meta.ecc_level, 0); // M
        assert!(qr_image("").size().width > 0);
    }

    #[test]
    fn sync_touches_only_changes() {
        let m = VecModel::from(vec![1, 2, 3]);
        sync(&m, vec![1, 5]);
        assert_eq!(m.iter().collect::<Vec<_>>(), vec![1, 5]);
        sync(&m, vec![1, 5, 6, 7]);
        assert_eq!(m.iter().collect::<Vec<_>>(), vec![1, 5, 6, 7]);
    }
}
