//! The window's own rules, on mocked time: controls ignore clicks until they have settled, and
//! the dangerous button of a confirmation waits longer than the safe one.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use hushphone::{Actions, AppWindow, Clock, Overlay};
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
    let st = ui.global::<hushphone::AppState>();
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
