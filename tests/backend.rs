//! Two hushphone clients over one in-memory Pigeonhole: a group made, joined by voucher, and a
//! text and a voice clip sent each way, all through the app's own backend and model.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use hushphone::backend::{self, Cmd, Params, Source, Update};
use hushphone::model::{self, Files};
use hushphone::settings::Settings;
use hushphone::voice;
use hushphone::{Delivery, LineKind};
use pigeonhole_katzenqt1_client::{ChatView, LineView};
use pigeonhole_transport::sim::World;
use tokio::sync::mpsc::UnboundedSender;

struct Peer {
    cmd: UnboundedSender<Cmd>,
    rx: mpsc::Receiver<Update>,
    chats: std::collections::HashMap<String, (ChatView, Vec<LineView>)>,
    paths: std::collections::HashMap<String, PathBuf>,
    made: Option<String>,
}

fn dir(name: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("backend-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

impl Peer {
    fn new(w: &World, name: &str) -> Peer {
        let (tx, rx) = mpsc::channel();
        let cmd = backend::spawn(
            Params { source: Source::Memory(w.clone()), state_dir: dir(name), name: name.into() },
            move |u| {
                let _ = tx.send(u);
            },
        );
        Peer { cmd, rx, chats: Default::default(), paths: Default::default(), made: None }
    }

    fn pump(&mut self) {
        while let Ok(u) = self.rx.recv_timeout(Duration::from_millis(20)) {
            match u {
                Update::Chat { view, from, lines, dropped, paths } => {
                    self.paths.extend(paths);
                    let e = self.chats.entry(view.id.clone()).or_insert_with(|| (view.clone(), Vec::new()));
                    e.0 = view;
                    if from != usize::MAX {
                        e.1.retain(|l| l.line.id < from && !dropped.contains(&l.line.id));
                        e.1.extend(lines);
                    }
                }
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
    let mut bob = Peer::new(&w, "bob");

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
    alice.cmd.send(Cmd::SendFile(g.clone(), name.clone(), clip.clone())).unwrap();
    bob.wait("alice's voice clip", |p| {
        p.chat(&bg).1.iter().any(|l| {
            l.line.attachment.as_ref().is_some_and(|a| a.name == name && p.paths.get(&a.file).is_some_and(|f| f.exists()))
        })
    });

    // What bob's UI would show.
    let (view, lines) = bob.chat(&bg).clone();
    let who = model::names(&view, &Settings::default(), "bob");
    let mut files = Files::default();
    for a in lines.iter().filter_map(|l| l.line.attachment.as_ref()) {
        let bytes = std::fs::read(&bob.paths[&a.file]).unwrap();
        assert_eq!(bytes, clip, "the clip arrives whole");
        files.durations.insert(a.file.clone(), voice::duration(&bytes).unwrap());
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
    let items = model::line_items(&lines, &model::names(&view, &Settings::default(), "alice"), &Files::default(), &Default::default(), chrono::Local::now());
    assert!(items.iter().any(|i| i.mine && i.delivery == Delivery::Read));
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
