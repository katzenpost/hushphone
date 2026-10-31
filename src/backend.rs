//! The client's side: a tokio runtime on its own thread owns the katzenqt1 client, carries out
//! the UI's commands and tells it what changed. Nothing here touches the screen, the clipboard,
//! files outside the state directory or the microphone; the key that seals the state comes from
//! the system's keyring, through [`crate::keys`].

use std::collections::{BTreeSet, HashMap};

use std::path::PathBuf;
use std::time::Duration;

use pigeonhole_protocols::protocols::katzenqt1::client::sealed_file::{seal_file, write_in_place};
use pigeonhole_protocols::protocols::katzenqt1::client::store::StoreFiles;
use pigeonhole_protocols::protocols::katzenqt1::client::{ChatView, Client, ClientError, Config, Files, LineView};
use pigeonhole_protocols::protocols::katzenqt1::engine::ChatId;
use pigeonhole_protocols::secret::Secret;
use pigeonhole_protocols::transport::Backend;
use pigeonhole_protocols::transport::remote::Remote;
use pigeonhole_protocols::transport::sim::World;
use pigeonhole_protocols::transport::thinclient::ThinClientBackend;
use futures_util::FutureExt;
use tokio::sync::mpsc;
use zeroize::Zeroizing;

use crate::keys::{self, Custody};

/// Where Pigeonhole is.
#[derive(Clone)]
pub enum Source {
    /// The client daemon, at `@name`, a socket path or `host:port`.
    Daemon(String),
    /// pigeonhole-sim's socket, for trying things on one machine.
    SimSocket(PathBuf),
    /// An in-memory Pigeonhole shared by the clients of this process.
    Memory(World),
}

/// How the state is kept at rest.
#[derive(Clone)]
pub enum Keys {
    /// Sealed with a key the system keeps; unsealed, and the user told, when it can keep none.
    System,
    /// Unsealed: for tests.
    None,
    /// Sealed with this key: for tests.
    Given(Secret),
}

/// The files the state keeps, read and written sealed when the state is.
#[derive(Clone)]
pub struct Attachments {
    dir: PathBuf,
    key: Option<Secret>,
}

/// Why a file could not be read: not there, or something else.
#[derive(Debug)]
pub struct ReadError {
    pub not_found: bool,
    pub what: String,
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.what)
    }
}

impl Attachments {
    pub fn new(dir: PathBuf, key: Option<Secret>) -> Attachments {
        Attachments { dir, key }
    }

    pub fn read(&self, file: &str) -> Result<Zeroizing<Vec<u8>>, ReadError> {
        StoreFiles::new(self.dir.clone(), self.key.clone()).read(file).map(Zeroizing::new).map_err(|e| ReadError {
            not_found: e.kind() == std::io::ErrorKind::NotFound,
            what: e.to_string(),
        })
    }

    /// Write `bytes` as the file `name` right under the state directory, sealed if the state
    /// is, in place of any file there.
    pub fn write(&self, name: &str, bytes: &[u8]) -> Result<(), String> {
        let mut parts = std::path::Path::new(name).components();
        if !matches!((parts.next(), parts.next()), (Some(std::path::Component::Normal(_)), None)) {
            return Err(format!("{name:?} is not a file in the state directory"));
        }
        std::fs::create_dir_all(&self.dir).map_err(|e| e.to_string())?;
        let sealed;
        let out = match &self.key {
            Some(k) => {
                sealed = seal_file(k, bytes);
                sealed.as_slice()
            }
            None => bytes,
        };
        write_in_place(&self.dir.join(name), out, false).map_err(|e| e.to_string())
    }
}

/// What a received file's bytes say about it, found once it has arrived.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileInfo {
    /// Its type, from its bytes.
    pub kind: String,
    /// Its length, if it is a voice clip.
    pub duration: Option<Duration>,
}

impl std::fmt::Debug for Attachments {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Attachments")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Link {
    Connecting,
    /// The daemon answers and is on the mixnet.
    Online,
    /// The daemon answers but is not on the mixnet.
    Offline,
    /// The daemon does not answer.
    Unreachable,
}

#[derive(Debug)]
pub enum Cmd {
    Create(String, u64),
    Join(String, u64),
    Invite(ChatId, String),
    Send(ChatId, String),
    SendFile(ChatId, String, Zeroizing<Vec<u8>>),
    Scan(ChatId),
    Remove(ChatId, Vec<u8>),
    Leave(ChatId),
    Clear(ChatId),
    Rename(ChatId, String),
}

#[derive(Debug)]
pub enum Update {
    /// The state's files; and, when the state is not sealed, why not.
    Files(Attachments, Option<String>),
    /// The state cannot be opened, and the client will not start; why.
    Locked(String),
    Link(Link),
    /// A group, and its lines from `from` on (all of them when `from` is 0).
    Chat {
        view: ChatView,
        from: usize,
        lines: Vec<LineView>,
        dropped: Vec<usize>,
        paths: Vec<(String, PathBuf)>,
        /// What the files that have arrived hold, by file, for those not told of before.
        files: Vec<(String, FileInfo)>,
    },
    Removed(ChatId),
    /// A group made at the user's request; `ticket` is the request's.
    Made(ChatId, u64),
    Failed(String),
    /// The client stopped and cannot go on.
    Stopped(String),
}

pub struct Params {
    pub source: Source,
    pub state_dir: PathBuf,
    pub name: String,
    pub keys: Keys,
}

/// Start the client's thread; `tell` is called on that thread for every update.
pub fn spawn(p: Params, tell: impl Fn(Update) + Send + Sync + 'static) -> mpsc::UnboundedSender<Cmd> {
    let (tx, rx) = mpsc::unbounded_channel();
    std::thread::Builder::new()
        .name("hushphone-client".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("tokio runtime");
            rt.block_on(run(p, rx, tell));
        })
        .expect("client thread");
    tx
}

/// The key the state is sealed with, after sealing it if it was not; `None` when it is to stay
/// unsealed. The UI is given the state's files to read attachments with.
async fn key(p: &Params, tell: &impl Fn(Update)) -> Result<Option<Secret>, String> {
    let mut unsealed = None;
    let key = match &p.keys {
        Keys::None => None,
        Keys::Given(k) => Some(k.clone()),
        Keys::System => match keys::custody(&keys::System, &p.state_dir).await {
            Custody::Sealed(k) => Some(k),
            Custody::Unsealed(why) => {
                unsealed = Some(why);
                None
            }
            Custody::Lost(why) => return Err(lost(&p.state_dir, &why)),
        },
    };
    if let Some(k) = &key {
        keys::seal_existing(&p.state_dir, k).map_err(|e| format!("Couldn't seal your chats: {e}."))?;
    }
    tell(Update::Files(Attachments::new(p.state_dir.clone(), key.clone()), unsealed));
    Ok(key)
}

fn lost(dir: &std::path::Path, why: &str) -> String {
    let fix = if cfg!(target_os = "android") {
        "To start over without them, clear hushphone's storage in Android's settings.".to_string()
    } else {
        format!("If it is gone for good, move {} aside to start over without them.", dir.display())
    };
    format!("Your chats are sealed, but their key can't be had: {why}. {fix}")
}

async fn run(p: Params, mut rx: mpsc::UnboundedReceiver<Cmd>, tell: impl Fn(Update) + Send + Sync + 'static) {
    let key = match key(&p, &tell).await {
        Ok(k) => k,
        Err(why) => {
            tell(Update::Locked(why));
            while rx.recv().await.is_some() {}
            return;
        }
    };
    let att = Attachments::new(p.state_dir.clone(), key.clone());
    let cfg = || Config {
        state_dir: p.state_dir.clone(),
        name: p.name.clone(),
        epoch_poll: Duration::from_secs(60),
        no_sync: false,
        key: key.clone(),
    };
    match &p.source {
        Source::Memory(w) => {
            tell(Update::Link(Link::Online));
            serve(cfg(), w.clone(), None, &att, &mut rx, &tell).await;
        }
        Source::SimSocket(s) => {
            tell(Update::Link(Link::Online));
            serve(cfg(), Remote { socket: s.clone() }, None, &att, &mut rx, &tell).await;
        }
        Source::Daemon(addr) => {
            let mut wait = Duration::from_secs(1);
            loop {
                tell(Update::Link(Link::Connecting));
                match ThinClientBackend::dial(addr).await {
                    Ok(b) => {
                        let thin = b.client.clone();
                        // A client that stops, or panics, is opened again on a new connection
                        // rather than leaving the app without its groups.
                        let served = std::panic::AssertUnwindSafe(serve(cfg(), b, Some(thin.clone()), &att, &mut rx, &tell))
                            .catch_unwind()
                            .await;
                        thin.stop().await;
                        match served {
                            Ok(Served::Quit) => return,
                            Ok(Served::Stopped) => {}
                            Err(_) => tell(Update::Stopped("the client failed".into())),
                        }
                        wait = Duration::from_secs(1);
                        tokio::time::sleep(wait).await;
                    }
                    Err(e) => {
                        #[cfg(target_os = "android")]
                        let e = crate::android::problem().unwrap_or_else(|| e.to_string());
                        tell(Update::Link(Link::Unreachable));
                        tell(Update::Failed(format!(
                            "Can't reach kpclientd at {addr}: {e}. Trying again in {} s.",
                            wait.as_secs()
                        )));
                        tokio::time::sleep(wait).await;
                        wait = (wait * 2).min(Duration::from_secs(30));
                    }
                }
            }
        }
    }
}

/// The files told of already, so that each is read once.
type Known = std::collections::HashSet<String>;

/// The lines of a group from the one before `first` on, where their files are, and what the
/// files that have arrived hold, read here rather than on the UI's thread.
async fn lines_from(c: &Client, id: &str, first: usize, att: &Attachments, known: &mut Known) -> Lines {
    let lines = c.lines(id, first.checked_sub(1)).await.unwrap_or_default();
    let mut paths = Vec::new();
    let mut files = Vec::new();
    for a in lines.iter().filter_map(|l| l.line.attachment.as_ref()) {
        let path = c.file_path(a);
        if !known.contains(&a.file)
            && path.exists()
            && let Ok(bytes) = att.read(&a.file)
        {
            let kind = crate::effects::sniff(&bytes);
            let duration = crate::voice::is_voice(&a.filetype, &a.name).then(|| crate::voice::duration(&bytes)).flatten();
            known.insert(a.file.clone());
            files.push((a.file.clone(), FileInfo { kind, duration }));
        }
        paths.push((a.file.clone(), path));
    }
    Lines { lines, paths, files }
}

struct Lines {
    lines: Vec<LineView>,
    paths: Vec<(String, PathBuf)>,
    files: Vec<(String, FileInfo)>,
}

/// Our own lines not yet acknowledged, by group: an acknowledgement changes a line without the
/// client's event naming it, so updates are read from the oldest of these on.
type Unacked = HashMap<ChatId, BTreeSet<usize>>;

/// What serving a client carries along.
struct Serving<'a> {
    att: &'a Attachments,
    known: Known,
    unacked: Unacked,
}

async fn send_chat(c: &Client, id: &str, first: usize, dropped: Vec<usize>, s: &mut Serving<'_>, tell: &impl Fn(Update)) {
    let Ok(view) = c.chat(id).await else { return };
    let waiting = s.unacked.entry(id.to_string()).or_default();
    let first = waiting.first().map_or(first, |&u| u.min(first));
    let Lines { lines, paths, files } = lines_from(c, id, first, s.att, &mut s.known).await;
    waiting.retain(|&u| u < first);
    waiting.extend(lines.iter().filter(|l| l.mine && !l.acked).map(|l| l.line.id));
    tell(Update::Chat { view, from: first, lines, dropped, paths, files });
}

/// How serving a client ended.
enum Served {
    /// The app is closing.
    Quit,
    /// The client stopped; a new one may be opened.
    Stopped,
}

async fn serve<B: Backend>(
    cfg: Config,
    backend: B,
    thin: Option<std::sync::Arc<katzenpost_thin_client::ThinClient>>,
    att: &Attachments,
    rx: &mut mpsc::UnboundedReceiver<Cmd>,
    tell: &impl Fn(Update),
) -> Served {
    let c = match Client::open(cfg, backend).await {
        Ok(c) => c,
        Err(e @ ClientError::Locked) => {
            tell(Update::Locked(sentence(&e)));
            return Served::Quit;
        }
        Err(e) => {
            tell(Update::Stopped(e.to_string()));
            return Served::Stopped;
        }
    };
    let mut served = Served::Quit;
    let mut events = c.subscribe();
    let mut s = Serving { att, known: Known::new(), unacked: Unacked::new() };
    for ch in c.chats().await.unwrap_or_default() {
        send_chat(&c, &ch.id, 0, Vec::new(), &mut s, tell).await;
    }
    let mut link = None;
    let mut tick = tokio::time::interval(Duration::from_secs(2));
    loop {
        tokio::select! {
            cmd = rx.recv() => {
                let Some(cmd) = cmd else { break };
                if let Err(e) = carry_out(&c, cmd, &mut s, tell).await {
                    tell(Update::Failed(sentence(&e)));
                }
            }
            Some(e) = events.recv() => {
                if let Some(err) = e.error {
                    tell(Update::Stopped(err));
                    served = Served::Stopped;
                    break;
                }
                if e.removed {
                    tell(Update::Removed(e.chat));
                    continue;
                }
                // Gather the burst: one update per group, from its oldest changed line.
                let mut changed: std::collections::BTreeMap<ChatId, (BTreeSet<usize>, Vec<usize>, bool)> = Default::default();
                let mut note = |e: pigeonhole_protocols::protocols::katzenqt1::client::Event| {
                    let ent = changed.entry(e.chat).or_default();
                    ent.0.extend(e.lines);
                    ent.1.extend(e.dropped);
                    ent.2 |= e.removed;
                };
                note(e);
                while let Ok(e) = events.try_recv() {
                    note(e);
                }
                for (id, (lines, dropped, removed)) in changed {
                    if removed {
                        tell(Update::Removed(id));
                    } else {
                        let first = lines.first().copied().unwrap_or(usize::MAX);
                        send_chat(&c, &id, first, dropped, &mut s, tell).await;
                    }
                }
            }
            _ = tick.tick() => {
                let now = match &thin {
                    Some(t) if t.is_connected() => Link::Online,
                    Some(_) => Link::Offline,
                    None => Link::Online,
                };
                if link != Some(now) {
                    link = Some(now);
                    tell(Update::Link(now));
                }
            }
        }
    }
    c.close().await;
    served
}

async fn carry_out(c: &Client, cmd: Cmd, s: &mut Serving<'_>, tell: &impl Fn(Update)) -> Result<(), ClientError> {
    match cmd {
        Cmd::Create(name, ticket) => {
            let id = c.create(&name).await?;
            send_chat(c, &id, 0, Vec::new(), s, tell).await;
            tell(Update::Made(id, ticket));
        }
        Cmd::Join(name, ticket) => {
            let id = c.join(&name).await?;
            send_chat(c, &id, 0, Vec::new(), s, tell).await;
            tell(Update::Made(id, ticket));
        }
        Cmd::Invite(id, code) => c.invite(&id, code.trim()).await?,
        Cmd::Send(id, text) => c.send(&id, &text).await?,
        Cmd::SendFile(id, name, mut bytes) => {
            // The client takes a plain vector and seals it; the wiped buffer stays behind.
            c.send_file(&id, &name, std::mem::take(&mut *bytes)).await?
        }
        Cmd::Scan(id) => c.scan(&id).await?,
        Cmd::Remove(id, key) => c.remove(&id, key).await?,
        Cmd::Leave(id) => c.leave(&id).await?,
        Cmd::Clear(id) => {
            c.clear(&id).await?;
            send_chat(c, &id, 0, Vec::new(), s, tell).await;
        }
        Cmd::Rename(id, to) => {
            c.rename(&id, &to).await?;
            send_chat(c, &id, usize::MAX, Vec::new(), s, tell).await;
        }
    }
    Ok(())
}

/// A client error as the user reads it.
pub fn sentence(e: &ClientError) -> String {
    use ClientError::*;
    match e {
        Closed => "The client has stopped.".into(),
        NameTaken => "You already have a group with that name.".into(),
        EmptyName => "A group needs a name.".into(),
        NoSuchChat => "That group is gone.".into(),
        NotJoined => "You haven't joined that group yet.".into(),
        AlreadyAdding => "A member is already being added. Wait for that to finish.".into(),
        NotAVoucher => "That isn't a voucher code. Ask the newcomer to copy it again.".into(),
        NoSuchMember => "That member is no longer in the group.".into(),
        RemoveSelf => "You can't remove yourself. Leave the group instead.".into(),
        FileTooLarge => "That file is too large to send: 16 MiB at most.".into(),
        EmptyMessage => "There's nothing to send.".into(),
        Locked => "Your chats are sealed with another key, or were changed, and can't be opened.".into(),
        Unsealed => "Your chats aren't sealed yet.".into(),
        Store(s) => format!("Couldn't save: {s}"),
    }
}
