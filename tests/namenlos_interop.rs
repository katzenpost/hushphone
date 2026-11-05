//! The real app against a running kpclientd, driven through the same Actions the buttons fire,
//! in one group with another client scripted from outside through files in $HUSH_INTEROP.
//! It does nothing unless HUSH_INTEROP is set. HUSH_ROLE is host (default) or guest, HUSH_GROUP
//! the group's name, HUSH_DAEMON the daemon's address, HUSH_EXPECT the text the other side sends.
//!   host:  create the group; wait for $HUSH_INTEROP/voucher; add that member; send; wait.
//!   guest: join; write the voucher to $HUSH_INTEROP/hush.voucher; wait to be added; send; wait.
//! Every few seconds the group's lines and state go to $HUSH_INTEROP/hush.lines; status lines go
//! to stderr. katzenrns's headless shell on a Reticulum bridge is one such other client.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use hushphone::backend::Source;
use hushphone::{Actions, AppState, AppWindow, Connection, Delivery, LineKind, Setup};
use slint::{ComponentHandle, LogicalSize, Model};

async fn sleep(ms: u64) {
    let (tx, rx) = tokio::sync::oneshot::channel();
    slint::Timer::single_shot(Duration::from_millis(ms), move || {
        let _ = tx.send(());
    });
    let _ = rx.await;
}

async fn until(what: &str, secs: u64, mut ok: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while !ok() {
        assert!(Instant::now() < deadline, "waited too long for {what}");
        sleep(500).await;
    }
}

fn dump(dir: &Path, ui: &AppWindow) -> String {
    let st = ui.global::<AppState>();
    let mut out = String::new();
    for l in st.get_lines().iter() {
        let d = match l.delivery {
            Delivery::None => "none",
            Delivery::Queued => "queued",
            Delivery::Written => "written",
            Delivery::Read => "read",
        };
        let k = match l.kind {
            LineKind::Text => "text",
            LineKind::Notice => "notice",
            LineKind::File => "file",
            LineKind::Voice => "voice",
        };
        out.push_str(&format!(
            "LINE kind={k} mine={} from={} delivery={d} text={}\n",
            l.mine, l.from, l.text
        ));
    }
    for c in st.get_chats().iter() {
        out.push_str(&format!(
            "CHAT {} joining={} members={} preview={}\n",
            c.name, c.joining, c.members, c.preview
        ));
    }
    out.push_str(&format!(
        "MEMBERS {} JOINED {} ADDING {} NOTICE {} CONN {:?} {}\n",
        st.get_current_members(),
        st.get_current_joined(),
        st.get_adding(),
        st.get_notice(),
        st.get_connection(),
        chrono::Local::now().format("%T")
    ));
    std::fs::write(dir.join("hush.lines"), &out).unwrap();
    out
}

fn status(s: &str) {
    eprintln!("STATUS {} {s}", chrono::Local::now().format("%T"));
}

fn has_line(ui: &AppWindow, from_other: bool, text: &str) -> bool {
    ui.global::<AppState>()
        .get_lines()
        .iter()
        .any(|l| l.kind == LineKind::Text && l.mine != from_other && l.text == text)
}

#[test]
fn namenlos_interop() {
    let Ok(dir) = std::env::var("HUSH_INTEROP") else {
        eprintln!("set HUSH_INTEROP to a directory and HUSH_ROLE to host or guest");
        return;
    };
    let dir = PathBuf::from(dir);
    let role = std::env::var("HUSH_ROLE").unwrap_or_else(|_| "host".into());
    let daemon = std::env::var("HUSH_DAEMON").unwrap_or_else(|_| "@katzenpost".into());
    let other_text = std::env::var("HUSH_EXPECT").unwrap_or_else(|_| "from the mesh".into());

    i_slint_backend_testing::init_integration_test_with_system_time();
    let data = dir.join("hush-data");
    let _ = std::fs::remove_dir_all(&data);
    let ui = AppWindow::new().unwrap();
    ui.window().set_size(LogicalSize::new(1100.0, 720.0));
    hushphone::bridge::start(
        &ui,
        Setup {
            cache_dir: data.join("cache"),
            files_dir: data.join("files"),
            data_dir: data.clone(),
            source: Source::Daemon(daemon),
            default_daemon: String::new(),
            name: Some("hush".into()),
            wire_version: None,
            old_defaults: Vec::new(),
            keys: hushphone::backend::Keys::None,
            camera: None,
        },
    );
    ui.show().unwrap();
    let group = std::env::var("HUSH_GROUP").unwrap_or_else(|_| "g".into());
    let weak = ui.as_weak();
    let dumper = weak.clone();
    let dump_dir = dir.clone();
    slint::spawn_local(async move {
        loop {
            sleep(5000).await;
            if let Some(ui) = dumper.upgrade() {
                dump(&dump_dir, &ui);
            }
        }
    })
    .unwrap();
    slint::spawn_local(async move {
        let ui = weak.upgrade().unwrap();
        let st = ui.global::<AppState>();
        let act = ui.global::<Actions>();
        until("the daemon", 300, || {
            st.get_connection() == Connection::Online
        })
        .await;
        status(&format!("online as {}", st.get_my_name()));

        if role == "host" {
            act.invoke_create_group(group.as_str().into());
            until("the group", 120, || {
                st.get_current_name() == group.as_str() && st.get_current_joined()
            })
            .await;
            status(&format!("created {group}"));
            let voucher = dir.join("voucher");
            until("the other side's voucher file", 900, || voucher.exists()).await;
            let code = std::fs::read_to_string(&voucher)
                .unwrap()
                .trim()
                .to_string();
            status(&format!(
                "adding member from voucher ({} chars)",
                code.len()
            ));
            act.invoke_invite(code.into());
            until("the member to be added", 1200, || {
                st.get_current_members() >= 1 && !st.get_adding()
            })
            .await;
            status(&format!("members {}", st.get_current_members()));
        } else {
            act.invoke_join_group(group.as_str().into());
            until("the voucher", 600, || !st.get_code().is_empty()).await;
            std::fs::write(dir.join("hush.voucher"), st.get_code().as_str()).unwrap();
            status("voucher written");
            until("to be added", 1800, || st.get_current_joined()).await;
            status(&format!("joined {group}"));
        }

        act.invoke_send_text("hello from hushphone".into());
        until("my line to be written", 600, || {
            st.get_lines().iter().any(|l| {
                l.mine
                    && l.text == "hello from hushphone"
                    && l.delivery != Delivery::Queued
                    && l.delivery != Delivery::None
            })
        })
        .await;
        status("sent");
        dump(&dir, &ui);

        let deadline = Instant::now() + Duration::from_secs(1800);
        while !has_line(&ui, true, &other_text) {
            assert!(
                Instant::now() < deadline,
                "no line {other_text:?} from the other side"
            );
            sleep(2000).await;
            dump(&dir, &ui);
        }
        print!("{}", dump(&dir, &ui));
        status("got the other side's line");
        // A second message carries the read acknowledgement for theirs.
        act.invoke_send_text("hushphone read you".into());
        until("the second line to be written", 600, || {
            st.get_lines().iter().any(|l| {
                l.mine
                    && l.text == "hushphone read you"
                    && l.delivery != Delivery::Queued
                    && l.delivery != Delivery::None
            })
        })
        .await;
        let deadline = Instant::now() + Duration::from_secs(900);
        while !st
            .get_lines()
            .iter()
            .any(|l| l.mine && l.text == "hello from hushphone" && l.delivery == Delivery::Read)
        {
            if Instant::now() > deadline {
                status("no read receipt within the wait");
                break;
            }
            sleep(2000).await;
            dump(&dir, &ui);
        }
        print!("{}", dump(&dir, &ui));
        status("done");
        let _ = slint::quit_event_loop();
    })
    .unwrap();
    slint::run_event_loop().unwrap();
}
