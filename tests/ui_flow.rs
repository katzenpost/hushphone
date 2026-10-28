//! The whole app on a real event loop over an in-memory Pigeonhole: make a group through the
//! window and say something in it, as a user would.

use std::path::Path;
use std::time::{Duration, Instant};

use hushphone::backend::Source;
use hushphone::{AppState, AppWindow, Connection, Delivery, LineKind, Setup};
use i_slint_backend_testing::{AccessibleRole, ElementHandle};
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
    let deadline = Instant::now() + Duration::from_secs(30);
    while !ok() {
        assert!(Instant::now() < deadline, "waited too long for {what}");
        sleep(50).await;
    }
}

fn find(ui: &AppWindow, label: &str, role: AccessibleRole) -> Option<ElementHandle> {
    ElementHandle::find_by_accessible_label(ui, label).find(|e| e.accessible_role() == Some(role))
}

/// Find a control, and give it the time it needs to settle before it accepts input.
async fn settled(ui: &AppWindow, label: &str, role: AccessibleRole) -> ElementHandle {
    until(label, || find(ui, label, role).is_some()).await;
    sleep(700).await;
    find(ui, label, role).unwrap()
}

#[test]
fn make_a_group_and_talk() {
    i_slint_backend_testing::init_integration_test_with_system_time();
    let data = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("ui-flow-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&data);
    let ui = AppWindow::new().unwrap();
    ui.window().set_size(LogicalSize::new(1100.0, 720.0));
    hushphone::bridge::start(
        &ui,
        Setup {
            cache_dir: data.join("cache"),
            data_dir: data,
            source: Source::Memory(Default::default()),
            default_daemon: String::new(),
            name: Some("alice".into()),
        },
    );
    ui.show().unwrap();
    let weak = ui.as_weak();
    slint::spawn_local(async move {
        let ui = weak.upgrade().unwrap();
        let st = ui.global::<AppState>();
        until("the link", || st.get_connection() == Connection::Online).await;
        assert_eq!(st.get_my_name(), "alice");

        settled(&ui, "New group", AccessibleRole::Button).await.single_click(PointerEventButton::Left).await;
        let field = ElementHandle::find_by_element_type_name(&ui, "LineEdit").next().expect("a name field");
        let input = field
            .query_descendants()
            .match_accessible_role(AccessibleRole::TextInput)
            .find_first()
            .unwrap_or(field);
        input.set_accessible_value("friends");
        settled(&ui, "Create", AccessibleRole::Button).await.single_click(PointerEventButton::Left).await;
        until("the group to open", || st.get_current_name() == "friends" && st.get_current_joined()).await;

        let message = settled(&ui, "Message to friends", AccessibleRole::TextInput).await;
        message.set_accessible_value("hello over the mixnet");
        settled(&ui, "Send to friends", AccessibleRole::Button).await.single_click(PointerEventButton::Left).await;
        until("the message to be written", || {
            st.get_lines().iter().any(|l| l.kind == LineKind::Text && l.mine && l.text == "hello over the mixnet" && l.delivery == Delivery::Written)
        })
        .await;
        assert!(st.get_chats().iter().any(|c| c.preview == "you: hello over the mixnet"));
        let _ = slint::quit_event_loop();
    })
    .unwrap();
    slint::run_event_loop().unwrap();
}
