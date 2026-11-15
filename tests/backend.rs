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
        Peer::open_at(w, name, state_dir, keys, 4)
    }

    fn open_at(w: &World, name: &str, state_dir: PathBuf, keys: Keys, wire_version: u64) -> Peer {
        let (tx, rx) = mpsc::channel();
        let params = Params { source: Source::Memory(w.clone()), state_dir, name: name.into(), keys, wire_version };
        let cmd = backend::spawn(params, move |u| {
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
                Update::Files(a) => self.attachments = Some(a),
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

/// Cancelling a join deletes the group and its voucher, for good.
#[test]
fn cancel_joining() {
    let w = World::default();
    let key = Secret::new(&[4; 32]);
    let state = dir("dana");
    let mut dana = Peer::open(&w, "dana", state.clone(), Keys::Given(key.clone()));
    dana.cmd.send(Cmd::Create("other".into(), 1)).unwrap();
    dana.wait("the other group", |p| p.made.is_some());
    let other = dana.made.take().unwrap();
    dana.cmd.send(Cmd::Join("friends".into(), 2)).unwrap();
    dana.wait("a voucher code", |p| p.made.as_ref().is_some_and(|id| p.chats.get(id).is_some_and(|c| !c.0.code.is_empty())));
    let id = dana.made.clone().unwrap();
    dana.cmd.send(Cmd::Cancel(id.clone())).unwrap();
    dana.wait("the group to go", |p| !p.chats.contains_key(&id));
    assert!(dana.chats.contains_key(&other), "the cancel took another group");
    drop(dana);
    released(&state, &key);
    // The saved groups are told all at once: once one is, the cancelled join would be too.
    let mut dana = Peer::open(&w, "dana", state, Keys::Given(key));
    dana.wait("the saved groups", |p| p.chats.contains_key(&other));
    assert!(!dana.chats.contains_key(&id), "the cancelled join came back");
}

/// Cancelling an add stops it and leaves the group; a cancel with nothing under way says so.
#[test]
fn cancel_adding() {
    use pigeonhole_protocols::protocols::katzenqt1::engine::{NoticeKind, voucher_code};
    let w = World::default();
    let mut erin = Peer::new(&w, "erin-add");
    erin.cmd.send(Cmd::Create("friends".into(), 1)).unwrap();
    erin.wait("the group", |p| p.made.as_ref().is_some_and(|id| p.chats.get(id).is_some_and(|c| c.0.joined)));
    let g = erin.made.clone().unwrap();
    let noticed = |p: &Peer, k: NoticeKind| p.chat(&g).1.iter().any(|l| l.line.notice.as_ref().is_some_and(|n| n.kind == k));
    // A voucher no one minted is read for until cancelled.
    erin.cmd.send(Cmd::Invite(g.clone(), voucher_code(&[5; 32]))).unwrap();
    erin.wait("the add to start", |p| p.chat(&g).0.adding);
    erin.cmd.send(Cmd::Cancel(g.clone())).unwrap();
    erin.wait("the add to stop", |p| !p.chat(&g).0.adding && noticed(p, NoticeKind::Cancelled));
    assert!(erin.chat(&g).0.joined);
    erin.cmd.send(Cmd::Cancel(g.clone())).unwrap();
    erin.wait("nothing to cancel", |p| noticed(p, NoticeKind::NothingToCancel));
    assert!(erin.chats.contains_key(&g));
}

/// The wire version in the params reaches the client: version 3 mints the map form voucher
/// payload, which begins with the map byte, and version 4 the compact one.
#[test]
fn wire_version_sets_the_voucher_form() {
    for v in [3, 4] {
        let w = World::default();
        let name = format!("erin{v}");
        let mut p = Peer::open_at(&w, &name, dir(&name), Keys::None, v);
        p.cmd.send(Cmd::Join("friends".into(), 1)).unwrap();
        p.wait("a voucher code", |p| p.made.as_ref().is_some_and(|id| p.chats.get(id).is_some_and(|c| !c.0.code.is_empty())));
        let compact = format!("{name}\0").into_bytes();
        let map = [&[0xa0][..], &compact].concat();
        let boxes = w.plaintexts();
        assert_eq!(boxes.iter().any(|b| b.starts_with(&map)), v == 3, "version {v}: the map form");
        assert_eq!(boxes.iter().any(|b| b.starts_with(&compact)), v == 4, "version {v}: the compact form");
    }
}

/// A ping reaches the other member as a notice naming who pinged, which the app plays once, and
/// is no line for the sender.
#[test]
fn ping_reaches_the_group() {
    use hushphone::ping::Pings;
    use pigeonhole_protocols::protocols::katzenqt1::engine::NoticeKind;
    let w = World::default();
    let mut alice = Peer::new(&w, "alice-ping");
    let mut bob = Peer::new(&w, "bob-ping");
    alice.cmd.send(Cmd::Create("friends".into(), 1)).unwrap();
    alice.wait("the group", |p| p.made.as_ref().is_some_and(|id| p.chats.get(id).is_some_and(|c| c.0.joined)));
    let g = alice.made.clone().unwrap();
    bob.cmd.send(Cmd::Join("friends".into(), 1)).unwrap();
    bob.wait("a voucher code", |p| p.made.as_ref().is_some_and(|id| p.chats.get(id).is_some_and(|c| !c.0.code.is_empty())));
    let bg = bob.made.clone().unwrap();
    alice.cmd.send(Cmd::Invite(g.clone(), bob.chat(&bg).0.code.clone())).unwrap();
    bob.wait("bob to join", |p| p.chat(&bg).0.joined);

    let mut pings = Pings::default();
    assert!(pings.arrived(&bg, &bob.chat(&bg).1).is_empty());
    alice.cmd.send(Cmd::Ping(g.clone())).unwrap();
    alice.cmd.send(Cmd::Send(g.clone(), "after".into())).unwrap();
    bob.wait("alice's ping", |p| p.chat(&bg).1.iter().any(|l| l.line.text == "after"));
    let lines = &bob.chat(&bg).1;
    let got: Vec<String> = pings.arrived(&bg, lines).into_iter().map(model::notice_text).collect();
    assert_eq!(got, ["~alice-ping pinged"]);
    assert!(pings.arrived(&bg, lines).is_empty(), "played once");
    let alice_lines = &alice.chat(&g).1;
    assert!(alice_lines.iter().all(|l| l.line.notice.as_ref().is_none_or(|n| n.kind != NoticeKind::Pinged)), "no line for the sender");
}

/// Saved groups show at once, with their lines, though the daemon can't be reached.
#[test]
fn saved_groups_show_without_the_daemon() {
    let w = World::default();
    let key = Secret::new(&[9; 32]);
    let state = dir("offline");
    let mut fay = Peer::open(&w, "fay", state.clone(), Keys::Given(key.clone()));
    fay.cmd.send(Cmd::Create("friends".into(), 1)).unwrap();
    fay.wait("the group", |p| p.made.as_ref().is_some_and(|id| p.chats.get(id).is_some_and(|c| c.0.joined)));
    let g = fay.made.clone().unwrap();
    fay.cmd.send(Cmd::Send(g.clone(), "saved".into())).unwrap();
    fay.wait("the line", |p| p.chat(&g).1.iter().any(|l| l.line.text == "saved"));
    drop(fay);
    released(&state, &key);

    let (tx, rx) = mpsc::channel();
    let nowhere = state.join("no-daemon.sock").to_string_lossy().into_owned();
    let params = Params { source: Source::Daemon(nowhere), state_dir: state, name: "fay".into(), keys: Keys::Given(key), wire_version: 4 };
    let _cmd = backend::spawn(params, move |u| {
        let _ = tx.send(u);
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let u = rx.recv_timeout(deadline.saturating_duration_since(Instant::now())).expect("the saved group, without the daemon");
        if let Update::Chat { view, lines, .. } = u
            && view.id == g
        {
            assert_eq!(view.name, "friends");
            assert!(lines.iter().any(|l| l.line.text == "saved"));
            break;
        }
    }
}

/// A poll made by one member reaches the others, who vote and change their votes; its maker
/// closes it; every member counts the same, as the app shows it.
#[test]
fn a_poll_reaches_every_member() {
    use pigeonhole_protocols::protocols::katzenqt1::tally::{self, Polls};
    let w = World::default();
    let names = ["alice-poll", "bob-poll", "carol-poll"];
    let mut peers: Vec<Peer> = names.iter().map(|n| Peer::new(&w, n)).collect();
    peers[0].cmd.send(Cmd::Create("friends".into(), 1)).unwrap();
    peers[0].wait("the group", |p| p.made.as_ref().is_some_and(|id| p.chats.get(id).is_some_and(|c| c.0.joined)));
    let mut ids = vec![peers[0].made.clone().unwrap()];
    for i in 1..3 {
        peers[i].cmd.send(Cmd::Join("friends".into(), 1)).unwrap();
        peers[i].wait("a voucher code", |p| p.made.as_ref().is_some_and(|id| p.chats.get(id).is_some_and(|c| !c.0.code.is_empty())));
        let id = peers[i].made.clone().unwrap();
        let code = peers[i].chat(&id).0.code.clone();
        peers[0].cmd.send(Cmd::Invite(ids[0].clone(), code)).unwrap();
        peers[i].wait("the join", |p| p.chat(&id).0.joined);
        ids.push(id);
    }
    for (p, id) in peers.iter_mut().zip(&ids) {
        p.wait("every member", |p| p.chat(id).0.members.len() == 2);
    }

    // What each member's app shows of the poll: options' (text, yes, mark), and whether closed.
    let shown = |p: &Peer, id: &str| {
        let (view, lines) = p.chat(id).clone();
        let who = model::names(&view, &Contacts::default(), "me");
        let items = model::line_items(&lines, &who, &Files::default(), &Default::default(), chrono::Local::now());
        items.iter().find(|i| i.kind == LineKind::Poll).map(|i| {
            use slint::Model;
            (i.poll_options.iter().map(|o| (o.text.to_string(), o.yes, o.mark.to_string())).collect::<Vec<_>>(), i.poll_voters, i.poll_closed)
        })
    };
    // The line a member's app shows the poll at, and its polls, as the bridge reads them.
    let polls = |p: &Peer, id: &str| {
        let (view, lines) = p.chat(id);
        let polls = Polls::from_lines(lines, &view.me);
        let at = lines.iter().map(|l| l.line.id).find(|&l| polls.at(l).is_some());
        (polls, at)
    };

    let opts = vec!["Mon".to_string(), "Tue".to_string()];
    let me = peers[0].chat(&ids[0]).0.me.clone();
    peers[0].cmd.send(Cmd::Tally(ids[0].clone(), tally::create(&[7; 16], "Lunch?", tally::Mode::Approval, &opts, &me))).unwrap();
    for (p, id) in peers.iter_mut().zip(&ids) {
        p.wait("the poll", |p| shown(p, id).is_some());
    }

    // Bob says yes to Tue; carol yes to Mon, then changes it to no.
    let vote = |p: &mut Peer, id: &str, index: usize| {
        let (polls, at) = polls(p, id);
        let t = polls.cycle(at.unwrap(), index).unwrap();
        p.cmd.send(Cmd::Tally(id.to_string(), t)).unwrap();
    };
    vote(&mut peers[1], &ids[1], 1);
    peers[1].wait("bob's own vote", |p| shown(p, &ids[1]).unwrap().0[1] == ("Tue".into(), 1, "yes".into()));
    vote(&mut peers[2], &ids[2], 0);
    peers[2].wait("carol's own vote", |p| shown(p, &ids[2]).unwrap().0[0] == ("Mon".into(), 1, "yes".into()));
    vote(&mut peers[2], &ids[2], 0);
    peers[2].wait("carol's change", |p| shown(p, &ids[2]).unwrap().0[0] == ("Mon".into(), 0, "no".into()));
    for (p, id) in peers.iter_mut().zip(&ids) {
        p.wait("both votes", |p| shown(p, id).is_some_and(|(o, voters, _)| voters == 2 && o[0].1 == 0 && o[1].1 == 1));
    }

    // Alice closes it.
    let (polls0, at0) = polls(&peers[0], &ids[0]);
    peers[0].cmd.send(Cmd::Tally(ids[0].clone(), polls0.close(at0.unwrap()).unwrap())).unwrap();
    for (p, id) in peers.iter_mut().zip(&ids) {
        p.wait("the close", |p| shown(p, id).is_some_and(|(_, _, closed)| closed));
    }
}
