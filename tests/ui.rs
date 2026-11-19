//! The window's own rules, on mocked time: controls ignore clicks until they have settled, and
//! the dangerous button of a confirmation waits longer than the safe one.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use hushphone::{Actions, AppState, AppWindow, ChatItem, Clock, Overlay};
use i_slint_backend_testing::{ElementHandle, mock_elapsed_time};
use slint::platform::PointerEventButton;
use slint::{ComponentHandle, LogicalSize, ModelRc, VecModel};

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
    let new_group = button(&ui, "Create or join a group");
    // A click the moment the button appears goes nowhere: it may have been meant for whatever
    // was under the pointer a moment before.
    new_group.mock_single_click(PointerEventButton::Left);
    new_group.invoke_accessible_default_action();
    assert_eq!(ui.get_overlay(), Overlay::None);
    mock_elapsed_time(Duration::from_millis(600));
    new_group.mock_single_click(PointerEventButton::Left);
    assert_eq!(ui.get_overlay(), Overlay::Start);

    // The dialog's buttons settle too, from when the dialog opened.
    let close = button(&ui, "Close");
    close.mock_single_click(PointerEventButton::Left);
    assert_eq!(ui.get_overlay(), Overlay::Start);
    mock_elapsed_time(Duration::from_millis(600));
    close.mock_single_click(PointerEventButton::Left);
    assert_eq!(ui.get_overlay(), Overlay::None);
}

/// One way in: the start dialog leads to the existing name entry for a new group.
#[test]
fn start_dialog_leads_to_a_new_group() {
    let ui = window();
    ui.set_overlay(Overlay::Start);
    let start = button(&ui, "Start a new group");
    mock_elapsed_time(Duration::from_millis(600));
    start.mock_single_click(PointerEventButton::Left);
    assert_eq!(ui.get_overlay(), Overlay::NewGroup);
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
    ui.global::<Actions>().on_cancel_voucher(move || c.set(c.get() + 1));
    ui.global::<Actions>().on_leave(|| panic!("a join is cancelled, not left"));
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

/// A member being added can be cancelled from the group's header, which shows only while adding.
#[test]
fn cancel_adding_from_the_header() {
    let ui = window();
    let cancelled = Rc::new(Cell::new(0));
    let c = cancelled.clone();
    ui.global::<Actions>().on_cancel_voucher(move || c.set(c.get() + 1));
    let st = ui.global::<AppState>();
    st.set_current("g".into());
    st.set_current_name("friends".into());
    st.set_current_joined(true);
    ui.set_show_conversation(true);
    mock_elapsed_time(Duration::from_millis(100));
    assert_eq!(ElementHandle::find_by_accessible_label(&ui, "Cancel adding a member").count(), 0);
    st.set_adding(true);
    let cancel = button(&ui, "Cancel adding a member");
    mock_elapsed_time(Duration::from_millis(600));
    cancel.mock_single_click(PointerEventButton::Left);
    assert_eq!(cancelled.get(), 1);
}

fn shown(ui: &AppWindow, label: &str, role: i_slint_backend_testing::AccessibleRole) -> usize {
    ElementHandle::find_by_accessible_label(ui, label).filter(|e| e.accessible_role() == Some(role)).count()
}

/// Each empty state says what to do next: no groups offers one way in, groups but none open asks
/// for one, and a group of one says how to add someone. On a phone the group pane carries them.
#[test]
fn empty_states_guide_the_next_step() {
    use i_slint_backend_testing::AccessibleRole::{Button, Text};
    let ui = window();
    let st = ui.global::<AppState>();
    assert_eq!(shown(&ui, "Create or join a group", Button), 1);
    assert_eq!(shown(&ui, "Select a group to start.", Text), 0);
    ui.window().set_size(LogicalSize::new(400.0, 720.0));
    assert_eq!(shown(&ui, "Create or join a group", Button), 1, "on a phone, in the group pane");
    ui.window().set_size(LogicalSize::new(1100.0, 720.0));

    let chat = ChatItem { id: "g".into(), name: "friends".into(), ..Default::default() };
    st.set_chats(ModelRc::new(VecModel::from(vec![chat])));
    assert_eq!(shown(&ui, "Select a group to start.", Text), 1);
    assert_eq!(shown(&ui, "Create or join a group", Button), 0);
    ui.window().set_size(LogicalSize::new(400.0, 720.0));
    assert_eq!(shown(&ui, "Select a group to start.", Text), 1, "on a phone, in the group pane");
    ui.window().set_size(LogicalSize::new(1100.0, 720.0));

    let alone = "No one else is here yet. Ask someone for their code, then add it with the add-member icon above.";
    st.set_current("g".into());
    st.set_current_name("friends".into());
    st.set_current_joined(true);
    assert_eq!(shown(&ui, alone, Text), 1);
    st.set_current_members(1);
    assert_eq!(shown(&ui, alone, Text), 0);
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
    let new_group = button(&ui, "Create or join a group");
    mock_elapsed_time(Duration::from_millis(600));
    // The window changes size, and the button moves under a pointer that was aimed elsewhere.
    ui.window().set_size(LogicalSize::new(900.0, 720.0));
    let moved = button(&ui, "Create or join a group");
    moved.mock_single_click(PointerEventButton::Left);
    assert_eq!(ui.get_overlay(), Overlay::None, "a click on a control that just moved is ignored");
    mock_elapsed_time(Duration::from_millis(600));
    new_group.mock_single_click(PointerEventButton::Left);
    assert_eq!(ui.get_overlay(), Overlay::Start);
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

/// A picture made smaller is shown with both sizes; Send sends it smaller, Send original sends the
/// file untouched, and Cancel drops it.
#[test]
fn a_picture_is_sent_smaller_unless_the_original_is_asked_for() {
    let ui = window();
    let chosen = Rc::new(std::cell::RefCell::new(Vec::new()));
    let (c, d) = (chosen.clone(), chosen.clone());
    ui.global::<Actions>().on_send_picture(move |original| c.borrow_mut().push(if original { "original" } else { "smaller" }));
    ui.global::<Actions>().on_drop_picture(move || d.borrow_mut().push("dropped"));
    let st = ui.global::<AppState>();
    st.set_current_name("friends".into());
    st.set_picture_name("Screenshot.png".into());
    st.set_picture_sizes("286 KB → 54 KB".into());
    for (label, what) in [("Send", "smaller"), ("Send original", "original"), ("Cancel", "dropped")] {
        ui.set_overlay(Overlay::Picture);
        assert_eq!(ElementHandle::find_by_accessible_label(&ui, "286 KB → 54 KB").count(), 1);
        let b = button(&ui, label);
        mock_elapsed_time(Duration::from_millis(600));
        b.mock_single_click(PointerEventButton::Left);
        assert_eq!(chosen.borrow().last(), Some(&what), "{label}");
        assert_eq!(ui.get_overlay(), Overlay::None);
    }
}

/// Every pending voucher is listed in one place, whatever group is open, and can be cancelled from
/// there: the cancel selects that group and asks for confirmation; nothing goes before it is given.
#[test]
fn pending_vouchers_are_listed_and_cancelled_from_the_menu() {
    let ui = window();
    let selected = Rc::new(std::cell::RefCell::new(String::new()));
    let s = selected.clone();
    ui.global::<Actions>().on_select_chat(move |id| *s.borrow_mut() = id.to_string());
    let cancelled = Rc::new(Cell::new(0));
    let c = cancelled.clone();
    ui.global::<Actions>().on_cancel_voucher(move || c.set(c.get() + 1));
    let pending = |id: &str, name: &str, minting: bool| ChatItem {
        id: id.into(),
        name: name.into(),
        initial: name[..1].to_uppercase().into(),
        joining: true,
        minting,
        ..Default::default()
    };
    let st = ui.global::<AppState>();
    st.set_pending(ModelRc::new(VecModel::from(vec![pending("g", "friends", false), pending("h", "work", true)])));
    ui.set_overlay(Overlay::Pending);
    mock_elapsed_time(Duration::from_millis(100));
    let _ = button(&ui, "Cancel joining work");
    let cancel = button(&ui, "Cancel joining friends");
    mock_elapsed_time(Duration::from_millis(600));
    cancel.mock_single_click(PointerEventButton::Left);
    assert_eq!(*selected.borrow(), "g");
    assert_eq!(ui.get_overlay(), Overlay::ConfirmLeave);
    assert_eq!(cancelled.get(), 0, "nothing is deleted before it is confirmed");
}

#[test]
fn text_names_the_transport_in_use() {
    let ui = window();
    let st = ui.global::<AppState>();
    assert_eq!(st.get_network(), "the mixnet");
    st.set_mesh(true);
    assert_eq!(st.get_network(), "the Reticulum mesh");
}

#[test]
fn radio_mode_trades_the_chat_for_a_radio_and_back() {
    let ui = window();
    let st = ui.global::<AppState>();
    st.set_current("c".into());
    st.set_current_name("friends".into());
    st.set_current_joined(true);
    let toggled = Rc::new(Cell::new(0));
    let t = toggled.clone();
    ui.global::<Actions>().on_toggle_radio(move || t.set(t.get() + 1));
    let composer = |ui: &AppWindow| {
        let _ = button(ui, "More for friends");
        ElementHandle::find_by_accessible_label(ui, "Attach a file for friends").count()
    };
    assert_eq!(composer(&ui), 1);
    mock_elapsed_time(Duration::from_millis(600));
    let switch = |ui: &AppWindow, label: &str| {
        let found: Vec<_> = ElementHandle::find_by_accessible_label(ui, label)
            .filter(|e| e.accessible_role() == Some(i_slint_backend_testing::AccessibleRole::Switch))
            .collect();
        assert_eq!(found.len(), 1, "one switch labelled {label:?}");
        found.into_iter().next().unwrap()
    };
    let off = switch(&ui, "Text and audio");
    assert_eq!(off.accessible_checked(), Some(false));
    off.invoke_accessible_default_action();
    assert_eq!(toggled.get(), 1);
    st.set_current_radio(true);
    assert_eq!(switch(&ui, "Audio only").accessible_checked(), Some(true));
    st.set_radio_receiving(true);
    st.set_radio_talker("carol".into());
    assert_eq!(composer(&ui), 0);
    assert_eq!(ElementHandle::find_by_accessible_label(&ui, "Channel friends: carol talking").count(), 1);
    let _ = button(&ui, "Hold to record a voice message");
    st.set_current_radio(false);
    assert_eq!(composer(&ui), 1);
}

/// With a voucher still pending, another is made only once the user confirms: a reply to the
/// first may still be on its way.
#[test]
fn a_second_voucher_is_confirmed_first() {
    let ui = window();
    ui.set_overlay(Overlay::Start);
    let join = button(&ui, "Join someone's group");
    mock_elapsed_time(Duration::from_millis(600));
    join.mock_single_click(PointerEventButton::Left);
    assert_eq!(ui.get_overlay(), Overlay::Join, "no voucher pending: straight to the dialog");
    ui.set_overlay(Overlay::Start);
    let pending = ChatItem { id: "g".into(), joining: true, ..Default::default() };
    ui.global::<AppState>().set_pending(ModelRc::new(VecModel::from(vec![pending])));
    let join = button(&ui, "Join someone's group");
    mock_elapsed_time(Duration::from_millis(600));
    join.mock_single_click(PointerEventButton::Left);
    assert_eq!(ui.get_overlay(), Overlay::ConfirmJoin);
    let again = button(&ui, "Make another");
    mock_elapsed_time(Duration::from_millis(800));
    again.mock_single_click(PointerEventButton::Left);
    assert_eq!(ui.get_overlay(), Overlay::Join);
}

#[test]
fn a_finger_that_stops_a_fling_and_rests_can_still_drag_the_list() {
    use i_slint_core::input::TouchPhase::{self, Ended, Moved, Started};
    use slint::platform::{InternalEvent, WindowEvent};
    use slint::private_unstable_api::re_exports::LogicalPoint;
    let ui = window();
    ui.window().set_size(LogicalSize::new(400.0, 800.0));
    let chats: Vec<ChatItem> = (0..40)
        .map(|i| ChatItem {
            id: format!("c{i}").into(),
            name: format!("Chat {i}").into(),
            ..Default::default()
        })
        .collect();
    ui.global::<AppState>()
        .set_chats(ModelRc::new(VecModel::from(chats)));
    let frames = |n: u32| {
        for _ in 0..n {
            mock_elapsed_time(Duration::from_millis(16));
            slint::platform::update_timers_and_animations();
        }
    };
    let row_y = |name: &str| {
        let row = ElementHandle::find_by_accessible_label(&ui, name).next();
        row.map(|e| e.absolute_position().y)
    };
    // Touch as the Android backend reports it.
    let touch = |y: f32, phase: TouchPhase| {
        let position = LogicalPoint::new(200.0, y);
        ui.window()
            .dispatch_event(WindowEvent::internal(InternalEvent::Touch {
                id: 0,
                position,
                phase,
            }));
    };
    let _ = row_y("Chat 0");
    frames(40);

    // A quick swipe up sets the list flinging.
    let mut y = 700.0;
    touch(y, Started);
    for _ in 0..8 {
        frames(1);
        y -= 40.0;
        touch(y, Moved);
    }
    touch(y, Ended);
    frames(5);
    // A finger stops it, rests a while, then drags it down: the list follows the finger.
    touch(300.0, Started);
    frames(1);
    let caught = row_y("Chat 12").expect("the fling brought chat 12 into view");
    frames(40);
    assert_eq!(
        row_y("Chat 12"),
        Some(caught),
        "the finger stopped the fling"
    );
    // Its first move past the slop lands on another row, as a quick finger's can.
    let mut y = 300.0;
    for step in [80.0].into_iter().chain([10.0; 12]) {
        frames(1);
        y += step;
        touch(y, Moved);
    }
    assert_eq!(
        row_y("Chat 12"),
        Some(caught + 200.0),
        "the list followed the finger"
    );
    touch(y, Ended);
    frames(60);
    assert!(!ui.get_show_conversation(), "a drag opens no chat");

    // A tap on a row that has come to rest still opens it.
    let tap_at = row_y("Chat 12").unwrap() + 32.0;
    touch(tap_at, Started);
    touch(tap_at, Ended);
    assert!(ui.get_show_conversation(), "a tap opens the chat");
}

#[test]
fn a_finger_drags_a_long_chat_up_from_the_bottom() {
    use hushphone::{LineItem, LineKind};
    use i_slint_core::input::TouchPhase::{self, Ended, Moved, Started};
    use slint::platform::{InternalEvent, WindowEvent};
    use slint::private_unstable_api::re_exports::LogicalPoint;
    let ui = window();
    ui.window().set_size(LogicalSize::new(400.0, 800.0));
    let st = ui.global::<AppState>();
    st.set_current("c".into());
    st.set_current_name("friends".into());
    st.set_current_joined(true);
    ui.set_show_conversation(true);
    // Short recent lines and long older ones, so the list's estimate of its height grows as older
    // lines come into view.
    let lines: Vec<LineItem> = (0..60)
        .map(|i| LineItem {
            id: i,
            kind: LineKind::Text,
            from: "~bob".into(),
            first: true,
            text: if i == 59 {
                "the last one".into()
            } else if i < 50 {
                format!("{i} {}", "a long line that wraps over a few rows ".repeat(4)).into()
            } else {
                format!("{i}").into()
            },
            ..Default::default()
        })
        .collect();
    st.set_lines(ModelRc::new(VecModel::from(lines)));
    let frames = |n: u32| {
        for _ in 0..n {
            mock_elapsed_time(Duration::from_millis(16));
            slint::platform::update_timers_and_animations();
        }
    };
    let last_y = || {
        ElementHandle::find_by_accessible_label(&ui, "~bob: the last one")
            .next()
            .map(|e| e.absolute_position().y)
    };
    let touch = |y: f32, phase: TouchPhase| {
        ui.window()
            .dispatch_event(WindowEvent::internal(InternalEvent::Touch {
                id: 0,
                position: LogicalPoint::new(200.0, y),
                phase,
            }));
    };
    let _ = last_y();
    frames(40);
    let start = last_y().expect("the chat opens at its last line");

    // A slow drag down by 150px, to read older lines.
    let mut y = 300.0;
    touch(y, Started);
    for _ in 0..50 {
        frames(1);
        y += 3.0;
        touch(y, Moved);
        let _ = last_y();
    }
    let moved = last_y().map_or(f32::INFINITY, |end| end - start);
    assert!(moved > 140.0, "the list followed the finger, moved {moved}px");
    touch(y, Ended);
}

/// A pasted message taller than the composer gets a scrollbar on its right, and every line of it
/// can be scrolled into view, by wheel or by finger; a send brings the box back to one line.
#[test]
fn a_pasted_message_taller_than_the_composer_scrolls() {
    use slint::platform::WindowEvent;
    let ui = window();
    let sent = Rc::new(std::cell::RefCell::new(String::new()));
    let s = sent.clone();
    ui.global::<Actions>().on_send_text(move |t| *s.borrow_mut() = t.into());
    let st = ui.global::<AppState>();
    st.set_current("c".into());
    st.set_current_name("friends".into());
    st.set_current_joined(true);
    let input = ElementHandle::find_by_accessible_label(&ui, "Message to friends").next().unwrap();
    let field = || ElementHandle::find_by_element_id(&ui, "Composer::field").next().unwrap();
    let thumbs = || ElementHandle::find_by_element_id(&ui, "Composer::thumb").collect::<Vec<_>>();
    let bottom = |e: &ElementHandle| e.absolute_position().y + e.size().height;

    input.set_accessible_value("one");
    let one = field().size().height;
    assert!(thumbs().is_empty(), "no scrollbar while it all fits");

    let pasted = (1..=12).map(|n| format!("line {n}")).collect::<Vec<_>>().join("\n");
    input.set_accessible_value(pasted.as_str());
    let f = field();
    assert!(f.size().height > one, "the box grows");
    let thumb = thumbs();
    assert_eq!(thumb.len(), 1, "a pasted message taller than the box has a scrollbar");
    let thumb = &thumb[0];
    assert!(thumb.size().height < f.size().height, "the bar shows only part is in view");
    assert!(thumb.absolute_position().x > f.absolute_position().x + f.size().width / 2.0, "on the right");

    // The wheel scrolls to the last line: the bar reaches the bottom of the box.
    let p = f.absolute_position();
    let mid = slint::LogicalPosition::new(p.x + f.size().width / 2.0, p.y + f.size().height / 2.0);
    ui.window().dispatch_event(WindowEvent::PointerMoved { position: mid });
    ui.window().dispatch_event(WindowEvent::PointerScrolled { position: mid, delta_x: 0.0, delta_y: -1000.0 });
    assert!((bottom(thumb) - bottom(&f)).abs() < 1.0, "scrolled to the end");

    // A finger dragging down brings the first line back.
    let low = slint::LogicalPosition::new(mid.x, mid.y + 30.0);
    ui.window().dispatch_event(WindowEvent::PointerPressed { position: mid, button: PointerEventButton::Left });
    for step in 1..=10 {
        let at = slint::LogicalPosition::new(mid.x, mid.y + 3.0 * step as f32);
        ui.window().dispatch_event(WindowEvent::PointerMoved { position: at });
    }
    ui.window().dispatch_event(WindowEvent::PointerReleased { position: low, button: PointerEventButton::Left });
    mock_elapsed_time(Duration::from_millis(1000));
    assert!(bottom(thumb) < bottom(&f) - 1.0, "a finger drag scrolls back up");

    mock_elapsed_time(Duration::from_millis(600));
    button(&ui, "Send to friends").mock_single_click(PointerEventButton::Left);
    assert_eq!(*sent.borrow(), pasted);
    input.set_accessible_value("x");
    assert_eq!(field().size().height, one, "a send brings it back to one line");
    assert!(thumbs().is_empty());
}

/// Emergency wipe: holding the button 2 s wipes, letting go sooner does not, and it is in reach
/// of a gloved thumb.
#[test]
fn a_two_second_hold_wipes_and_an_early_release_does_not() {
    use slint::platform::WindowEvent;
    let ui = window();
    let wiped = Rc::new(Cell::new(0));
    let w = wiped.clone();
    ui.global::<Actions>().on_wipe(move || w.set(w.get() + 1));
    ui.set_overlay(Overlay::Wipe);
    let hold = button(&ui, "Emergency wipe: hold for 2 seconds to destroy everything");
    // 20 mm on Android, where a logical pixel is a dp, 1/160 inch: 126px.
    assert!(hold.size().height >= 126.0, "{:?}", hold.size());
    let (p, size) = (hold.absolute_position(), hold.size());
    let at = slint::LogicalPosition::new(p.x + size.width / 2.0, p.y + size.height / 2.0);
    let press = || ui.window().dispatch_event(WindowEvent::PointerPressed { position: at, button: PointerEventButton::Left });
    let release = || ui.window().dispatch_event(WindowEvent::PointerReleased { position: at, button: PointerEventButton::Left });
    mock_elapsed_time(Duration::from_millis(600));

    press();
    mock_elapsed_time(Duration::from_millis(1900));
    release();
    mock_elapsed_time(Duration::from_millis(500));
    assert_eq!(wiped.get(), 0, "let go at 1.9 s: nothing happens");

    press();
    mock_elapsed_time(Duration::from_millis(1000));
    mock_elapsed_time(Duration::from_millis(1100));
    assert_eq!(wiped.get(), 1, "held 2 s: wiped");
    release();
    mock_elapsed_time(Duration::from_millis(3000));
    assert_eq!(wiped.get(), 1, "once");
}

/// After a wipe the report is shown, and it cannot be dismissed but by reading on.
#[test]
fn the_wipe_report_leads_to_the_first_start() {
    let ui = window();
    let seen = Rc::new(Cell::new(0));
    let s = seen.clone();
    ui.global::<Actions>().on_wipe_seen(move || s.set(s.get() + 1));
    ui.global::<AppState>().set_wipe_report("• The key: deleted.".into());
    ui.set_overlay(Overlay::Wiped);
    let go = button(&ui, "Start again");
    mock_elapsed_time(Duration::from_millis(600));
    go.mock_single_click(PointerEventButton::Left);
    assert_eq!(seen.get(), 1);
}
