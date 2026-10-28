//! The client's side: a tokio runtime on its own thread owns the katzenqt1 client, carries out
//! the UI's commands and tells it what changed. Nothing here touches the screen, the clipboard,
//! files outside the state directory or the microphone.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::time::Duration;

use pigeonhole_katzenqt1_client::{ChatView, Client, ClientError, Config, LineView};
use pigeonhole_katzenqt1_engine::{Attachment, ChatId};
use pigeonhole_transport::Backend;
use pigeonhole_transport::remote::Remote;
use pigeonhole_transport::sim::World;
use pigeonhole_transport_thinclient::ThinClientBackend;
use futures_util::FutureExt;
use tokio::sync::mpsc;

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
    SendFile(ChatId, String, Vec<u8>),
    Scan(ChatId),
    Remove(ChatId, Vec<u8>),
    Leave(ChatId),
    Clear(ChatId),
    Rename(ChatId, String),
}

#[derive(Debug)]
pub enum Update {
    Link(Link),
    /// A group, and its lines from `from` on (all of them when `from` is 0).
    Chat {
        view: ChatView,
        from: usize,
        lines: Vec<LineView>,
        dropped: Vec<usize>,
        paths: Vec<(String, PathBuf)>,
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

async fn run(p: Params, mut rx: mpsc::UnboundedReceiver<Cmd>, tell: impl Fn(Update) + Send + Sync + 'static) {
    let cfg = || Config {
        state_dir: p.state_dir.clone(),
        name: p.name.clone(),
        epoch_poll: Duration::from_secs(60),
        no_sync: false,
    };
    match &p.source {
        Source::Memory(w) => {
            tell(Update::Link(Link::Online));
            serve(cfg(), w.clone(), None, &mut rx, &tell).await;
        }
        Source::SimSocket(s) => {
            tell(Update::Link(Link::Online));
            serve(cfg(), Remote { socket: s.clone() }, None, &mut rx, &tell).await;
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
                        let served = std::panic::AssertUnwindSafe(serve(cfg(), b, Some(thin.clone()), &mut rx, &tell))
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

/// The lines of a group from the one before `first` on.
async fn lines_from(c: &Client, id: &str, first: usize) -> (Vec<LineView>, Vec<(String, PathBuf)>) {
    let lines = c.lines(id, first.checked_sub(1)).await.unwrap_or_default();
    let paths = lines
        .iter()
        .filter_map(|l| l.line.attachment.as_ref())
        .map(|a: &Attachment| (a.file.clone(), c.file_path(a)))
        .collect();
    (lines, paths)
}

/// Our own lines not yet acknowledged, by group: an acknowledgement changes a line without the
/// client's event naming it, so updates are read from the oldest of these on.
type Unacked = HashMap<ChatId, BTreeSet<usize>>;

async fn send_chat(c: &Client, id: &str, first: usize, dropped: Vec<usize>, unacked: &mut Unacked, tell: &impl Fn(Update)) {
    let Ok(view) = c.chat(id).await else { return };
    let waiting = unacked.entry(id.to_string()).or_default();
    let first = waiting.first().map_or(first, |&u| u.min(first));
    let (lines, paths) = lines_from(c, id, first).await;
    waiting.retain(|&u| u < first);
    waiting.extend(lines.iter().filter(|l| l.mine && !l.acked).map(|l| l.line.id));
    tell(Update::Chat { view, from: first, lines, dropped, paths });
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
    rx: &mut mpsc::UnboundedReceiver<Cmd>,
    tell: &impl Fn(Update),
) -> Served {
    let c = match Client::open(cfg, backend).await {
        Ok(c) => c,
        Err(e) => {
            tell(Update::Stopped(e.to_string()));
            return Served::Stopped;
        }
    };
    let mut served = Served::Quit;
    let mut events = c.subscribe();
    let mut unacked = Unacked::new();
    for ch in c.chats().await.unwrap_or_default() {
        send_chat(&c, &ch.id, 0, Vec::new(), &mut unacked, tell).await;
    }
    let mut link = None;
    let mut tick = tokio::time::interval(Duration::from_secs(2));
    loop {
        tokio::select! {
            cmd = rx.recv() => {
                let Some(cmd) = cmd else { break };
                if let Err(e) = carry_out(&c, cmd, &mut unacked, tell).await {
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
                let mut note = |e: pigeonhole_katzenqt1_client::Event| {
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
                        send_chat(&c, &id, first, dropped, &mut unacked, tell).await;
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

async fn carry_out(c: &Client, cmd: Cmd, unacked: &mut Unacked, tell: &impl Fn(Update)) -> Result<(), ClientError> {
    match cmd {
        Cmd::Create(name, ticket) => {
            let id = c.create(&name).await?;
            send_chat(c, &id, 0, Vec::new(), unacked, tell).await;
            tell(Update::Made(id, ticket));
        }
        Cmd::Join(name, ticket) => {
            let id = c.join(&name).await?;
            send_chat(c, &id, 0, Vec::new(), unacked, tell).await;
            tell(Update::Made(id, ticket));
        }
        Cmd::Invite(id, code) => c.invite(&id, code.trim()).await?,
        Cmd::Send(id, text) => c.send(&id, &text).await?,
        Cmd::SendFile(id, name, bytes) => c.send_file(&id, &name, bytes).await?,
        Cmd::Scan(id) => c.scan(&id).await?,
        Cmd::Remove(id, key) => c.remove(&id, key).await?,
        Cmd::Leave(id) => c.leave(&id).await?,
        Cmd::Clear(id) => {
            c.clear(&id).await?;
            send_chat(c, &id, 0, Vec::new(), unacked, tell).await;
        }
        Cmd::Rename(id, to) => {
            c.rename(&id, &to).await?;
            send_chat(c, &id, usize::MAX, Vec::new(), unacked, tell).await;
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
        Store(s) => format!("Couldn't save: {s}"),
    }
}
