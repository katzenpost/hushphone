//! Two hushphone clients over one in-memory Pigeonhole: a group made, joined by voucher, and a
//! text and a voice clip sent each way, all through the app's own backend and model.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use hushphone::backend::{self, Attachments, Cmd, FileInfo, Keys, Params, Source, Update};
use hushphone::contacts::Contacts;
use hushphone::model::{self, Files};
use hushphone::voice;
use hushphone::{Delivery, LineKind};
use pigeonhole_protocols::protocols::katzenqt1::client::{ChatView, LineView};
use pigeonhole_protocols::secret::Secret;
use pigeonhole_protocols::transport::sim::World;
use tokio::sync::mpsc::UnboundedSender;
use zeroize::Zeroizing;

struct Peer {
    cmd: UnboundedSender<Cmd>,
    rx: mpsc::Receiver<Update>,
    chats: std::collections::HashMap<String, (ChatView, Vec<LineView>)>,
    paths: std::collections::HashMap<String, PathBuf>,
    infos: std::collections::HashMap<String, FileInfo>,
    attachments: Option<Attachments>,
    locked: Option<String>,
    made: Option<String>,
}

fn dir(name: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("backend-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

/// Wait for the client that had the state in `dir`, sealed with `key`, to let it go.
fn released(dir: &Path, key: &Secret) {
    use pigeonhole_protocols::protocols::katzenqt1::client::store::{Options, Store, StoreError};
    let deadline = Instant::now() + Duration::from_secs(10);
    while let Err(StoreError::Other(e)) = Store::open(dir, Options { no_sync: true, key: Some(key.clone()) }) {
        assert!(Instant::now() < deadline, "the state stays open: {e}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

impl Peer {
    fn new(w: &World, name: &str) -> Peer {
        Peer::open(w, name, dir(name), Keys::None)
    }

    fn open(w: &World, name: &str, state_dir: PathBuf, keys: Keys) -> Peer {
        let (tx, rx) = mpsc::channel();
        let cmd = backend::spawn(Params { source: Source::Memory(w.clone()), state_dir, name: name.into(), keys }, move |u| {
            let _ = tx.send(u);
        });
        Peer {
            cmd,
            rx,
            chats: Default::default(),
            paths: Default::default(),
            infos: Default::default(),
            attachments: None,
            locked: None,
            made: None,
        }
    }

    fn pump(&mut self) {
        while let Ok(u) = self.rx.recv_timeout(Duration::from_millis(20)) {
            match u {
                Update::Chat { view, from, lines, dropped, paths, files } => {
                    self.paths.extend(paths);
                    self.infos.extend(files);
                    let e = self.chats.entry(view.id.clone()).or_insert_with(|| (view.clone(), Vec::new()));
                    e.0 = view;
                    if from != usize::MAX {
                        e.1.retain(|l| l.line.id < from && !dropped.contains(&l.line.id));
                        e.1.extend(lines);
                    }
                }
                Update::Files(a, unsealed) => {
                    assert_eq!(unsealed, None);
                    self.attachments = Some(a);
                }
                Update::Locked(why) => self.locked = Some(why),
                Update::Made(id, _) => self.made = Some(id),
                Update::Removed(id) => {
                    self.chats.remove(&id);
                }
                Update::Failed(e) | Update::Stopped(e) => panic!("{e}"),
                _ => {}
            }
        }
    }

    fn wait(&mut self, what: &str, mut ok: impl FnMut(&Peer) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            self.pump();
            if ok(self) {
                return;
            }
            assert!(Instant::now() < deadline, "waited a minute for {what}");
        }
    }

    fn chat(&self, id: &str) -> &(ChatView, Vec<LineView>) {
        &self.chats[id]
    }
}

#[test]
fn two_members_talk_and_send_voice() {
    let w = World::default();
    let mut alice = Peer::new(&w, "alice");
    // Bob's state is sealed.
    let bob_dir = dir("bob");
    let mut bob = Peer::open(&w, "bob", bob_dir.clone(), Keys::Given(Secret::new(&[7; 32])));

    alice.cmd.send(Cmd::Create("friends".into(), 1)).unwrap();
    alice.wait("the group", |p| p.made.as_ref().is_some_and(|id| p.chats.get(id).is_some_and(|c| c.0.joined)));
    let g = alice.made.clone().unwrap();

    bob.cmd.send(Cmd::Join("alice's friends".into(), 1)).unwrap();
    bob.wait("a voucher code", |p| p.made.as_ref().is_some_and(|id| p.chats.get(id).is_some_and(|c| !c.0.code.is_empty())));
    let bg = bob.made.clone().unwrap();
    let code = bob.chat(&bg).0.code.clone();

    alice.cmd.send(Cmd::Invite(g.clone(), code)).unwrap();
    bob.wait("bob to join", |p| p.chat(&bg).0.joined);
    alice.wait("bob to be a member", |p| p.chat(&g).0.members.len() == 1);

    bob.cmd.send(Cmd::Send(bg.clone(), "hi alice".into())).unwrap();
    alice.wait("bob's message", |p| p.chat(&g).1.iter().any(|l| l.line.text == "hi alice"));

    // A voice clip goes as a file named by its hash, as frank_chat sends it.
    let clip = std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/tone.opus")).unwrap();
    let name = voice::clip_name(&clip);
    alice.cmd.send(Cmd::SendFile(g.clone(), name.clone(), Zeroizing::new(clip.clone()))).unwrap();
    bob.wait("alice's voice clip", |p| {
        p.chat(&bg).1.iter().any(|l| {
            l.line.attachment.as_ref().is_some_and(|a| a.name == name && p.paths.get(&a.file).is_some_and(|f| f.exists()))
        })
    });

    // What bob's UI would show.
    let (view, lines) = bob.chat(&bg).clone();
    let who = model::names(&view, &Contacts::default(), "bob");
    let mut files = Files::default();
    for a in lines.iter().filter_map(|l| l.line.attachment.as_ref()) {
        let bytes = bob.attachments.as_ref().unwrap().read(&a.file).unwrap();
        assert_eq!(*bytes, clip, "the clip arrives whole");
        assert_ne!(std::fs::read(&bob.paths[&a.file]).unwrap(), clip, "the clip is kept sealed");
        // The client's thread already read the file and said what it is.
        let info = bob.infos.get(&a.file).expect("what the file holds");
        assert_eq!(info.kind, "Ogg audio");
        assert_eq!(info.duration, voice::duration(&bytes));
        files.durations.insert(a.file.clone(), info.duration.unwrap());
    }
    let items = model::line_items(&lines, &who, &files, &Default::default(), chrono::Local::now());
    let v = items.iter().find(|i| i.kind == LineKind::Voice).expect("a voice line");
    assert!(!v.mine && v.unverified);
    assert_eq!(v.from, "alice");
    assert_ne!(v.duration, "0:00");
    let mine = items.iter().find(|i| i.text == "hi alice").unwrap();
    assert!(mine.mine);

    // Alice's message is acknowledged once bob reads it and says something.
    bob.cmd.send(Cmd::Send(bg.clone(), "got it".into())).unwrap();
    alice.wait("bob's ack", |p| {
        p.chat(&g).1.iter().any(|l| l.mine && l.line.attachment.is_some() && l.acked)
    });
    let (view, lines) = alice.chat(&g).clone();
    let items = model::line_items(&lines, &model::names(&view, &Contacts::default(), "alice"), &Files::default(), &Default::default(), chrono::Local::now());
    assert!(items.iter().any(|i| i.mine && i.delivery == Delivery::Read));

    // Bob's state opens only with its key.
    drop(bob);
    released(&bob_dir, &Secret::new(&[7; 32]));
    let mut keyless = Peer::open(&w, "bob", bob_dir.clone(), Keys::None);
    keyless.wait("bob's state to stay shut", |p| p.locked.is_some());
    assert!(keyless.chats.is_empty());
    drop(keyless);
    released(&bob_dir, &Secret::new(&[7; 32]));
    let mut bob = Peer::open(&w, "bob", bob_dir, Keys::Given(Secret::new(&[7; 32])));
    bob.wait("bob's groups again", |p| p.chats.get(&bg).is_some_and(|c| c.1.iter().any(|l| l.line.text == "got it")));
}

/// Cancelling a join deletes the group and its voucher.
#[test]
fn cancel_joining() {
    let w = World::default();
    let mut dana = Peer::new(&w, "dana");
    dana.cmd.send(Cmd::Join("friends".into(), 1)).unwrap();
    dana.wait("a voucher code", |p| p.made.as_ref().is_some_and(|id| p.chats.get(id).is_some_and(|c| !c.0.code.is_empty())));
    let id = dana.made.clone().unwrap();
    dana.cmd.send(Cmd::Leave(id.clone())).unwrap();
    dana.wait("the group to go", |p| !p.chats.contains_key(&id));
}
