//! A member removed from the Members dialog goes from it at once, before the removal is done.

use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use hushphone::backend::{self, Cmd, Keys, Params, Source, Update};
use hushphone::{Actions, AppState, AppWindow, Connection, Overlay, Setup};
use i_slint_backend_testing::{AccessibleRole, ElementHandle};
use pigeonhole_protocols::transport::sim::World;
use slint::platform::PointerEventButton;
use slint::{ComponentHandle, LogicalSize, Model};

async fn sleep(ms: u64) {
    let (tx, rx) = tokio::sync::oneshot::channel();
    slint::Timer::single_shot(Duration::from_millis(ms), move || {
        let _ = tx.send(());
    });
    let _ = rx.await;
}

async fn until(what: &str, mut ok: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while !ok() {
        assert!(Instant::now() < deadline, "waited too long for {what}");
        sleep(50).await;
    }
}

fn find(ui: &AppWindow, label: &str) -> Option<ElementHandle> {
    ElementHandle::find_by_accessible_label(ui, label).find(|e| e.accessible_role() == Some(AccessibleRole::Button))
}

async fn settled(ui: &AppWindow, label: &str) -> ElementHandle {
    until(label, || find(ui, label).is_some()).await;
    sleep(700).await;
    find(ui, label).unwrap()
}

#[test]
fn a_removed_member_goes_from_the_dialog_at_once() {
    i_slint_backend_testing::init_integration_test_with_system_time();
    let data = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("ui-members-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&data);
    let world = World::default();
    let ui = AppWindow::new().unwrap();
    ui.window().set_size(LogicalSize::new(1100.0, 720.0));
    hushphone::bridge::start(
        &ui,
        Setup {
            cache_dir: data.join("cache"),
            files_dir: data.join("files"),
            data_dir: data.join("alice"),
            source: Source::Memory(world.clone()),
            default_daemon: String::new(),
            name: Some("alice".into()),
            wire_version: None,
            old_defaults: Vec::new(),
            keys: Keys::None,
            camera: None,
        },
    );
    // Bob joins through the backend alone.
    let (tx, bob_rx) = mpsc::channel();
    let params =
        Params { source: Source::Memory(world), state_dir: data.join("bob"), name: "bob".into(), keys: Keys::None, wire_version: 4 };
    let bob = backend::spawn(params, move |u| {
        let _ = tx.send(u);
    });
    ui.show().unwrap();
    let weak = ui.as_weak();
    slint::spawn_local(async move {
        let ui = weak.upgrade().unwrap();
        let st = ui.global::<AppState>();
        until("the link", || st.get_connection() == Connection::Online).await;
        ui.global::<Actions>().invoke_create_group("friends".into());
        until("the group", || st.get_current_name() == "friends" && st.get_current_joined()).await;

        bob.send(Cmd::Join("friends".into(), 1)).unwrap();
        let mut code = String::new();
        until("bob's voucher", || {
            while let Ok(u) = bob_rx.try_recv() {
                if let Update::Chat { view, .. } = u
                    && !view.code.is_empty()
                {
                    code = view.code;
                }
            }
            !code.is_empty()
        })
        .await;
        ui.global::<Actions>().invoke_invite(code.into());
        until("bob to be a member", || st.get_members().iter().any(|m| m.name == "bob")).await;

        ui.set_overlay(Overlay::Members);
        settled(&ui, "Stop reading bob").await.single_click(PointerEventButton::Left).await;
        let confirm = settled(&ui, "Stop reading").await;
        // No await between the confirmation and the check: the removal cannot have been done.
        confirm.invoke_accessible_default_action();
        assert!(!st.get_members().iter().any(|m| m.name == "bob"), "bob is still listed");
        assert_eq!(ui.get_overlay(), Overlay::Members);
        let _ = slint::quit_event_loop();
    })
    .unwrap();
    slint::run_event_loop().unwrap();
}
