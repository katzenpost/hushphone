//! The window's own rules, on mocked time: controls ignore clicks until they have settled, and
//! the dangerous button of a confirmation waits longer than the safe one.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use hushphone::{Actions, AppState, AppWindow, Clock, Overlay};
use i_slint_backend_testing::{ElementHandle, mock_elapsed_time};
use slint::platform::PointerEventButton;
use slint::{ComponentHandle, LogicalSize};

fn window() -> AppWindow {
    i_slint_backend_testing::init_no_event_loop();
    let ui = AppWindow::new().unwrap();
    ui.global::<Clock>().on_now(|| i_slint_backend_testing::get_mocked_time() as i64);
    ui.window().set_size(LogicalSize::new(1100.0, 720.0));
    ui.show().unwrap();
    ui
}

fn button(ui: &AppWindow, label: &str) -> ElementHandle {
    let found: Vec<_> = ElementHandle::find_by_accessible_label(ui, label)
        .filter(|e| e.accessible_role() == Some(i_slint_backend_testing::AccessibleRole::Button))
        .collect();
    assert_eq!(found.len(), 1, "one element labelled {label:?}");
    let b = found.into_iter().next().unwrap();
    // Lay it out now, as drawing it would: the testing backend only lays out on demand.
    let _ = b.absolute_position();
    slint::platform::update_timers_and_animations();
    b
}

#[test]
fn controls_settle_before_they_act() {
    let ui = window();
    let new_group = button(&ui, "New group");
    // A click the moment the button appears goes nowhere: it may have been meant for whatever
    // was under the pointer a moment before.
    new_group.mock_single_click(PointerEventButton::Left);
    new_group.invoke_accessible_default_action();
    assert_eq!(ui.get_overlay(), Overlay::None);
    mock_elapsed_time(Duration::from_millis(600));
    new_group.mock_single_click(PointerEventButton::Left);
    assert_eq!(ui.get_overlay(), Overlay::NewGroup);

    // The dialog's buttons settle too, from when the dialog opened.
    let cancel = button(&ui, "Cancel");
    cancel.mock_single_click(PointerEventButton::Left);
    assert_eq!(ui.get_overlay(), Overlay::NewGroup);
    mock_elapsed_time(Duration::from_millis(600));
    cancel.mock_single_click(PointerEventButton::Left);
    assert_eq!(ui.get_overlay(), Overlay::None);
}

#[test]
fn leaving_waits_longer_than_cancelling() {
    let ui = window();
    let left = Rc::new(Cell::new(0));
    let l = left.clone();
    ui.global::<Actions>().on_leave(move || l.set(l.get() + 1));
    ui.global::<AppState>().set_current_joined(true);
    ui.set_overlay(Overlay::ConfirmLeave);
    let leave = button(&ui, "Leave group");
    mock_elapsed_time(Duration::from_millis(600));
    leave.mock_single_click(PointerEventButton::Left);
    assert_eq!(left.get(), 0, "the dangerous choice is not ready at 600 ms");
    assert_eq!(ui.get_overlay(), Overlay::ConfirmLeave);
    mock_elapsed_time(Duration::from_millis(200));
    leave.mock_single_click(PointerEventButton::Left);
    assert_eq!(left.get(), 1);
    assert_eq!(ui.get_overlay(), Overlay::None);
}

/// A group still joining can be given up from its voucher card: the voucher goes once confirmed.
#[test]
fn cancel_joining_from_the_voucher_card() {
    let ui = window();
    let cancelled = Rc::new(Cell::new(0));
    let c = cancelled.clone();
    ui.global::<Actions>().on_leave(move || c.set(c.get() + 1));
    let st = ui.global::<AppState>();
    st.set_current("g".into());
    st.set_current_name("friends".into());
    st.set_code("AAAA BBBB".into());
    mock_elapsed_time(Duration::from_millis(100));
    let card = button(&ui, "Cancel joining");
    mock_elapsed_time(Duration::from_millis(600));
    card.mock_single_click(PointerEventButton::Left);
    assert_eq!(ui.get_overlay(), Overlay::ConfirmLeave);
    assert_eq!(cancelled.get(), 0, "nothing is deleted before it is confirmed");
    let confirm: Vec<_> = ElementHandle::find_by_accessible_label(&ui, "Cancel joining")
        .filter(|e| e.accessible_role() == Some(i_slint_backend_testing::AccessibleRole::Button))
        .collect();
    let confirm = confirm.last().unwrap();
    let _ = confirm.absolute_position();
    mock_elapsed_time(Duration::from_millis(800));
    confirm.mock_single_click(PointerEventButton::Left);
    assert_eq!(cancelled.get(), 1);
}

#[test]
fn every_button_has_a_label() {
    let ui = window();
    ui.set_overlay(Overlay::Settings);
    mock_elapsed_time(Duration::from_millis(100));
    let unlabelled: Vec<_> = ElementHandle::find_by_element_type_name(&ui, "IconButton")
        .chain(ElementHandle::find_by_element_type_name(&ui, "Button"))
        .filter(|e| e.accessible_label().is_none_or(|l| l.is_empty()))
        .collect();
    assert!(unlabelled.is_empty(), "{} buttons without a label", unlabelled.len());
}

#[test]
fn a_control_that_moves_must_settle_again() {
    let ui = window();
    let new_group = button(&ui, "New group");
    mock_elapsed_time(Duration::from_millis(600));
    // The window changes size, and the button moves under a pointer that was aimed elsewhere.
    ui.window().set_size(LogicalSize::new(900.0, 720.0));
    let moved = button(&ui, "New group");
    moved.mock_single_click(PointerEventButton::Left);
    assert_eq!(ui.get_overlay(), Overlay::None, "a click on a control that just moved is ignored");
    mock_elapsed_time(Duration::from_millis(600));
    new_group.mock_single_click(PointerEventButton::Left);
    assert_eq!(ui.get_overlay(), Overlay::NewGroup);
}

#[test]
fn releasing_the_talk_button_sends_after_the_composer_changes() {
    use slint::platform::WindowEvent;
    let ui = window();
    let started = Rc::new(Cell::new(0));
    let stopped = Rc::new(Cell::new(0));
    let (s, t) = (started.clone(), stopped.clone());
    let st = ui.global::<AppState>();
    st.set_current("c".into());
    st.set_current_name("friends".into());
    st.set_current_joined(true);
    let weak = ui.as_weak();
    ui.global::<Actions>().on_ptt_start(move || {
        s.set(s.get() + 1);
        // As the host does: the composer turns into the recording bar.
        weak.upgrade().unwrap().global::<hushphone::AppState>().set_recording(true);
    });
    ui.global::<Actions>().on_ptt_stop(move || t.set(t.get() + 1));
    let talk = button(&ui, "Hold to record a voice message");
    mock_elapsed_time(Duration::from_millis(600));
    let p = talk.absolute_position();
    let at = slint::LogicalPosition::new(p.x + talk.size().width / 2.0, p.y + talk.size().height / 2.0);
    ui.window().dispatch_event(WindowEvent::PointerMoved { position: at });
    ui.window().dispatch_event(WindowEvent::PointerPressed { position: at, button: PointerEventButton::Left });
    assert_eq!(started.get(), 1);
    mock_elapsed_time(Duration::from_millis(2000));
    ui.window().dispatch_event(WindowEvent::PointerReleased { position: at, button: PointerEventButton::Left });
    assert_eq!(stopped.get(), 1, "the release reaches the button that saw the press");
}

/// Rows are updated in place, so a button can come to stand for another message without
/// moving: it must settle again before it acts on the new one.
#[test]
fn a_button_that_comes_to_stand_for_another_message_settles_again() {
    use hushphone::LineItem;
    use slint::{Model, ModelRc, VecModel};
    let ui = window();
    let saved = Rc::new(Cell::new(-1));
    let s = saved.clone();
    ui.global::<Actions>().on_save_file(move |id| s.set(id));
    let st = ui.global::<AppState>();
    st.set_current("c".into());
    st.set_current_name("friends".into());
    st.set_current_joined(true);
    ui.set_show_conversation(true);
    let file = |id: i32, name: &str| LineItem {
        id,
        kind: hushphone::LineKind::File,
        file_name: name.into(),
        file_size: "1 KB".into(),
        file_type: "PDF document".into(),
        from: "~bob".into(),
        first: true,
        ..Default::default()
    };
    let lines = Rc::new(VecModel::from(vec![file(1, "a.pdf")]));
    st.set_lines(ModelRc::from(lines.clone()));
    mock_elapsed_time(Duration::from_millis(100));
    let save = button(&ui, "Save a.pdf");
    mock_elapsed_time(Duration::from_millis(600));
    // The row now shows another message, at the same place.
    lines.set_row_data(0, file(2, "b.pdf"));
    let save_b = button(&ui, "Save b.pdf");
    assert_eq!(save.absolute_position(), save_b.absolute_position(), "the button did not move");
    save_b.mock_single_click(PointerEventButton::Left);
    assert_eq!(saved.get(), -1, "a click right after the change goes nowhere");
    mock_elapsed_time(Duration::from_millis(600));
    save_b.mock_single_click(PointerEventButton::Left);
    assert_eq!(saved.get(), 2);
}

/// The attach dialog lists what the host found in the send directory, and sends the one picked.
#[test]
fn attaching_picks_from_the_send_directory() {
    use hushphone::AttachItem;
    use slint::{ModelRc, VecModel};
    let ui = window();
    let listed = Rc::new(Cell::new(0));
    let sent = Rc::new(std::cell::RefCell::new(String::new()));
    let (l, s) = (listed.clone(), sent.clone());
    ui.global::<Actions>().on_open_attach(move || l.set(l.get() + 1));
    ui.global::<Actions>().on_attach_file(move |name| *s.borrow_mut() = name.to_string());
    let st = ui.global::<AppState>();
    st.set_current("c".into());
    st.set_current_name("friends".into());
    st.set_current_joined(true);
    st.set_files_dir("/home/me/hushphone".into());
    ui.set_show_conversation(true);
    mock_elapsed_time(Duration::from_millis(100));
    let attach = button(&ui, "Attach a file for friends");
    mock_elapsed_time(Duration::from_millis(600));
    attach.mock_single_click(PointerEventButton::Left);
    assert_eq!(listed.get(), 1, "the host is asked to list the directory");
    // As the host does once it has read the directory.
    st.set_attachable(ModelRc::from(Rc::new(VecModel::from(vec![
        AttachItem { name: "report.pdf".into(), size: "12 KB".into(), too_large: false },
        AttachItem { name: "huge.iso".into(), size: "700.0 MB".into(), too_large: true },
    ]))));
    ui.set_overlay(Overlay::Attach);
    let sends: Vec<_> = ElementHandle::find_by_accessible_label(&ui, "Send")
        .filter(|e| e.accessible_role() == Some(i_slint_backend_testing::AccessibleRole::Button))
        .collect();
    assert_eq!(sends.len(), 2);
    for s in &sends {
        let _ = s.absolute_position();
    }
    assert_eq!(sends[1].accessible_enabled(), Some(false), "a file too large cannot be sent");
    mock_elapsed_time(Duration::from_millis(600));
    sends[0].mock_single_click(PointerEventButton::Left);
    assert_eq!(*sent.borrow(), "report.pdf");
    assert_eq!(ui.get_overlay(), Overlay::None);
}
