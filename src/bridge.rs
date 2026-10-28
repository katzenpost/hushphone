//! The UI thread's side: the UI's callbacks become commands to the client or effects, and the
//! client's updates become models for the UI.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use pigeonhole_katzenqt1_client::{ChatView, LineView};
use pigeonhole_katzenqt1_engine::ChatId;
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use tokio::sync::mpsc::UnboundedSender;

use crate::backend::{self, Cmd, Link, Source, Update};
use crate::model::{self, Files};
use crate::settings::{Settings, ThemeChoice};
use crate::voice::{self, Voice};
use crate::{Actions, AppState, AppWindow, ChatItem, Connection, LineItem, MemberItem, Overlay, Theme, ThemePreference, effects, text};

/// Proof that an effect was asked for by the user, through the UI. Only this module can make
/// one, and only in the UI's callbacks.
pub struct UserIntent(());

impl UserIntent {
    fn new() -> UserIntent {
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
    pub source: Source,
    pub default_daemon: String,
    /// A name given on the command line, for a first start without asking.
    pub name: Option<String>,
    /// Daemon addresses an older version saved as its default, to move to `default_daemon`.
    pub old_defaults: Vec<String>,
}

struct App {
    ui: slint::Weak<AppWindow>,
    setup: Setup,
    settings: Settings,
    cmd: Option<UnboundedSender<Cmd>>,
    effects: tokio::runtime::Runtime,
    chats: BTreeMap<ChatId, ChatState>,
    paths: HashMap<String, PathBuf>,
    files: Files,
    current: Option<ChatId>,
    expanded: HashSet<usize>,
    voice: Voice,
    /// What the user picked last: a result for an older pick does not move the selection.
    ticket: u64,
    /// Which group a recording in progress is for.
    recording_for: Option<ChatId>,
    chat_model: Rc<VecModel<ChatItem>>,
    line_model: Rc<VecModel<LineItem>>,
    member_model: Rc<VecModel<MemberItem>>,
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
    let mut settings = Settings::load(&setup.data_dir);
    if settings.name.is_empty()
        && let Some(n) = &setup.name
    {
        settings.name = text::name(n);
        let _ = settings.save(&setup.data_dir);
    }
    if settings.daemon.is_empty() || setup.old_defaults.contains(&settings.daemon) {
        settings.daemon = setup.default_daemon.clone();
    }
    let chat_model = Rc::new(VecModel::default());
    let line_model = Rc::new(VecModel::default());
    let member_model = Rc::new(VecModel::default());
    let start = std::time::Instant::now();
    ui.global::<crate::Clock>().on_now(move || start.elapsed().as_millis() as i64);
    let st = ui.global::<AppState>();
    st.set_chats(ModelRc::from(chat_model.clone()));
    st.set_lines(ModelRc::from(line_model.clone()));
    st.set_members(ModelRc::from(member_model.clone()));
    st.set_daemon(settings.daemon.clone().into());
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
        cmd: None,
        effects: tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("effects runtime"),
        chats: BTreeMap::new(),
        paths: HashMap::new(),
        files: Files::default(),
        current: None,
        expanded: HashSet::new(),
        voice,
        ticket: 0,
        recording_for: None,
        chat_model,
        line_model,
        member_model,
        timer: slint::Timer::default(),
        #[cfg(any(target_os = "linux", target_os = "android"))]
        net: None,
    };
    APP.with(|a| *a.borrow_mut() = Some(app));
    wire(ui);
    with_app(|app| {
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
            app.settings.set_petname(&key, &name);
            if let Err(e) = app.settings.save(&app.setup.data_dir) {
                app.notice(&format!("Couldn't save the name: {e}"), true);
            }
            app.render_all();
        });
    });
    a.on_save_settings(|daemon| {
        with_app(|app| {
            app.settings.daemon = daemon.trim().to_string();
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
    a.on_attach_file(|| {
        with_app(|app| {
            let Some(chat) = app.current.clone() else { return };
            let intent = UserIntent::new();
            app.effects.spawn(async move {
                let r = effects::pick_file(intent).await;
                later(move |app| match r {
                    Ok(Some(f)) => {
                        // The group the file was picked for must still be there to send it.
                        if app.joined(&chat) {
                            app.send(Cmd::SendFile(chat, text::identifier(&f.name, 200), f.bytes));
                        }
                    }
                    Ok(None) => {}
                    Err(e) => app.notice(&e, true),
                });
            });
        });
    });
    a.on_save_file(|line| {
        with_app(|app| {
            let Some((path, name)) = app.attachment(line as usize) else { return };
            let intent = UserIntent::new();
            app.effects.spawn(async move {
                let r = effects::save_file(intent, path, name).await;
                later(move |app| match r {
                    Ok(true) => app.notice("Saved.", false),
                    Ok(false) => {}
                    Err(e) => app.notice(&format!("Couldn't save: {e}"), true),
                });
            });
        });
    });
    a.on_open_file(|line| {
        with_app(|app| {
            let Some((path, _)) = app.attachment(line as usize) else { return };
            let intent = UserIntent::new();
            app.effects.spawn(async move {
                if let Err(e) = effects::open_with(intent, path).await {
                    later(move |app| app.notice(&format!("Couldn't open: {e}"), true));
                }
            });
        });
    });
    a.on_toggle_voice(|line| {
        with_app(|app| app.toggle_voice(line as usize));
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
                _ => Net::Unix,
            };
            if let Err(e) = sandbox::enter(net) {
                self.notice(&format!("Couldn't sandbox the app: {e}"), true);
            }
            self.net = Some(net);
        }
        let params = backend::Params {
            source,
            state_dir: self.setup.data_dir.join("state"),
            name: self.settings.name.clone(),
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
            Update::Chat { view, from, lines, dropped, paths } => {
                let id = view.id.clone();
                self.paths.extend(paths);
                let seen = self.settings.seen.get(&id).copied().unwrap_or(0);
                let c = self.chats.entry(id.clone()).or_insert_with(|| ChatState { seen, ..Default::default() });
                c.view = Some(view);
                if from != usize::MAX {
                    c.lines.retain(|l| l.line.id < from && !dropped.contains(&l.line.id));
                    c.lines.extend(lines);
                } else {
                    c.lines.retain(|l| !dropped.contains(&l.line.id));
                }
                if self.current.as_deref() == Some(id.as_str()) {
                    c.seen = c.lines.last().map_or(0, |l| l.line.id);
                    self.remember_seen(&id);
                }
                self.learn_files(&id);
                self.render_chats();
                if self.current.as_deref() == Some(id.as_str()) {
                    self.render_current();
                }
            }
            Update::Removed(id) => {
                self.chats.remove(&id);
                if self.settings.seen.remove(&id).is_some() {
                    let _ = self.settings.save(&self.setup.data_dir);
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

    /// Voice clip lengths and file types, for the files that have arrived.
    fn learn_files(&mut self, id: &str) {
        let Some(c) = self.chats.get(id) else { return };
        for a in c.lines.iter().filter_map(|l| l.line.attachment.as_ref()) {
            if self.files.types.contains_key(&a.file) {
                continue;
            }
            let Some(path) = self.paths.get(&a.file) else { continue };
            if !path.exists() {
                continue;
            }
            self.files.types.insert(a.file.clone(), effects::sniff(path));
            if voice::is_voice(&a.filetype, &a.name)
                && let Ok(b) = std::fs::read(path)
            {
                self.files.durations.insert(a.file.clone(), voice::duration(&b).unwrap_or_default());
            }
        }
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
        if self.settings.seen.get(id) != Some(&seen) {
            self.settings.seen.insert(id.to_string(), seen);
            let _ = self.settings.save(&self.setup.data_dir);
        }
    }

    fn attachment(&self, line: usize) -> Option<(PathBuf, String)> {
        let c = self.chats.get(self.current.as_ref()?)?;
        let l = c.lines.iter().find(|l| l.line.id == line)?;
        let a = l.line.attachment.as_ref()?;
        let path = self.paths.get(&a.file)?.clone();
        path.exists().then(|| (path, text::identifier(&a.name, 200)))
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
                let who = model::names(v, &self.settings, &self.settings.name);
                let last = c.lines.iter().rev().find(|l| l.line.notice.is_none());
                let unread = if self.current.as_deref() == Some(v.id.as_str()) {
                    0
                } else {
                    c.lines.iter().filter(|l| l.line.id > c.seen && !l.mine && l.line.notice.is_none()).count()
                };
                let when = c.lines.last().map_or(i64::MAX, |l| l.line.time);
                Some((when, model::chat_item(v, last, unread, &who, now)))
            })
            .collect();
        items.sort_by_key(|i| std::cmp::Reverse(i.0));
        sync(&self.chat_model, items.into_iter().map(|(_, i)| i).collect());
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
        let who = model::names(v, &self.settings, &self.settings.name);
        let mut items = model::line_items(&c.lines, &who, &self.files, &self.expanded, chrono::Local::now());
        if let Some(p) = &self.voice.playing
            && p.chat == v.id
            && let Some(i) = items.iter_mut().find(|i| i.id as usize == p.line)
        {
            i.playing = true;
            i.progress = (p.started.elapsed().as_secs_f32() / p.length.as_secs_f32().max(0.001)).min(1.0);
        }
        st.set_current(v.id.clone().into());
        st.set_current_name(text::name(&v.name).into());
        st.set_current_joined(v.joined);
        st.set_current_members(v.members.len() as i32);
        st.set_code(group_code(&v.code).into());
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
            self.render_current();
            return;
        }
        let Some((path, _)) = self.attachment(line) else {
            self.notice("That voice message hasn't arrived yet.", true);
            return;
        };
        if let Err(e) = self.voice.play(&chat, line, &path) {
            self.notice(&format!("Couldn't play it: {e}"), true);
            return;
        }
        self.tick();
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
                Ok(Some(clip)) if app.joined(&chat) => app.send(Cmd::SendFile(chat, clip.name, clip.bytes)),
                Ok(Some(_)) => app.notice("The group is gone: the voice message was not sent.", true),
                Ok(None) => app.notice("Too short to send: hold the button while you talk.", false),
                Err(e) => app.notice(&format!("Couldn't record: {e}"), true),
            });
        });
    }

    /// While recording or playing, update the time shown, and stop at the longest clip.
    fn tick(&self) {
        if self.timer.running() {
            return;
        }
        self.timer.start(slint::TimerMode::Repeated, Duration::from_millis(100), || {
            with_app(|app| {
                let mut busy = false;
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
                        busy = app.voice.recording().is_some();
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

/// A voucher code in groups of eight characters, for reading out.
fn group_code(code: &str) -> String {
    code.as_bytes()
        .chunks(8)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect::<Vec<_>>()
        .join(" ")
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
    fn sync_touches_only_changes() {
        let m = VecModel::from(vec![1, 2, 3]);
        sync(&m, vec![1, 5]);
        assert_eq!(m.iter().collect::<Vec<_>>(), vec![1, 5]);
        sync(&m, vec![1, 5, 6, 7]);
        assert_eq!(m.iter().collect::<Vec<_>>(), vec![1, 5, 6, 7]);
    }
}
