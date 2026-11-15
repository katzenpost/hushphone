//! The whole app on a real event loop over an in-memory Pigeonhole: make a group through the
//! window and say something in it, as a user would.

use std::path::Path;
use std::time::{Duration, Instant};

use hushphone::backend::Source;
use hushphone::{AppState, AppWindow, Connection, Delivery, LineKind, Overlay, Setup};
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

/// Type into the one widget of type `widget` on screen.
fn typed(ui: &AppWindow, widget: &str, value: &str) {
    let field = ElementHandle::find_by_element_type_name(ui, widget).next().unwrap_or_else(|| panic!("no {widget}"));
    let input = field.query_descendants().match_accessible_role(AccessibleRole::TextInput).find_first().unwrap_or(field);
    input.set_accessible_value(value);
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
    let files = data.join("files");
    hushphone::bridge::start(
        &ui,
        Setup {
            cache_dir: data.join("cache"),
            files_dir: files.clone(),
            data_dir: data.clone(),
            source: Source::Memory(Default::default()),
            default_daemon: String::new(),
            name: Some("alice".into()),
            wire_version: Some(3),
            old_defaults: Vec::new(),
            keys: hushphone::backend::Keys::None,
            camera: None,
        },
    );
    // --wire-version is saved for later starts, and the backend reads it from the settings.
    let at_rest = hushphone::backend::Attachments::new(data.join("state"), None);
    assert_eq!(hushphone::settings::Settings::load(&at_rest).unwrap().unwrap().wire_version, Some(3));
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

        // A file is sent by putting it in the send directory and picking it there.
        let send = files.join("send");
        std::fs::create_dir_all(&send).unwrap();
        std::fs::write(send.join("notes.txt"), b"the minutes").unwrap();
        settled(&ui, "Attach a file for friends", AccessibleRole::Button).await.single_click(PointerEventButton::Left).await;
        settled(&ui, "Send", AccessibleRole::Button).await.single_click(PointerEventButton::Left).await;
        until("the file to be written", || {
            st.get_lines().iter().any(|l| l.kind == LineKind::File && l.mine && l.file_name == "notes.txt" && l.delivery == Delivery::Written)
        })
        .await;

        // A poll, made from the group's menu; a vote for an option, and taking it back.
        settled(&ui, "More for friends", AccessibleRole::Button).await.single_click(PointerEventButton::Left).await;
        // An entry shown only in a joined group, which once closed the menu and with it its own
        // handler, before it opened the dialog.
        settled(&ui, "New poll", AccessibleRole::Button).await.single_click(PointerEventButton::Left).await;
        until("the poll dialog", || ui.get_overlay() == Overlay::Poll && find(&ui, "Send poll", AccessibleRole::Button).is_some()).await;
        typed(&ui, "LineEdit", "Lunch: which day?");
        typed(&ui, "TextEdit", "Monday\nTuesday\n");
        settled(&ui, "Send poll", AccessibleRole::Button).await.single_click(PointerEventButton::Left).await;
        let poll = || st.get_lines().iter().find(|l| l.kind == LineKind::Poll);
        until("the poll", || poll().is_some_and(|p| p.delivery == Delivery::Written)).await;
        let p = poll().unwrap();
        assert_eq!(p.text, "Lunch: which day?");
        let counts = |p: &hushphone::LineItem| p.poll_options.iter().map(|o| (o.text.to_string(), o.yes, o.mark.to_string())).collect::<Vec<_>>();
        assert_eq!(counts(&p), vec![("Monday".into(), 0, "".into()), ("Tuesday".into(), 0, "".into())]);
        assert!(!p.poll_availability && p.poll_can_close);
        // A tap marks yes, the next no, the next nothing.
        settled(&ui, "Tuesday, 0 yes, not marked", AccessibleRole::Button).await.single_click(PointerEventButton::Left).await;
        until("the vote", || poll().is_some_and(|p| p.poll_voters == 1 && counts(&p)[1] == ("Tuesday".into(), 1, "yes".into()))).await;
        settled(&ui, "Tuesday, 1 yes, you said yes", AccessibleRole::Button).await.single_click(PointerEventButton::Left).await;
        until("the no", || poll().is_some_and(|p| counts(&p)[1] == ("Tuesday".into(), 0, "no".into()))).await;
        settled(&ui, "Tuesday, 0 yes, you said no", AccessibleRole::Button).await.single_click(PointerEventButton::Left).await;
        until("the vote taken back", || poll().is_some_and(|p| counts(&p)[1] == ("Tuesday".into(), 0, "".into()))).await;
        // Who voted what; then its maker closes it, and it says what was chosen.
        settled(&ui, "Show who voted", AccessibleRole::Button).await.single_click(PointerEventButton::Left).await;
        until("the ballots", || poll().is_some_and(|p| p.poll_ballots == "you: nothing marked")).await;
        settled(&ui, "Close poll", AccessibleRole::Button).await.single_click(PointerEventButton::Left).await;
        until("the close", || poll().is_some_and(|p| p.poll_closed && p.poll_outcome == "No option had a yes." && !p.poll_can_close)).await;
        // Votes are not lines of their own.
        assert_eq!(st.get_lines().iter().filter(|l| l.kind == LineKind::Text && l.text.contains("vote")).count(), 0);
        let _ = slint::quit_event_loop();
    })
    .unwrap();
    slint::run_event_loop().unwrap();
}
