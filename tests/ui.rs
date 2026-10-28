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
