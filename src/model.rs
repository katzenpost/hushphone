//! What the client says, in the shapes the UI draws: pure functions, tested without a window.

use std::collections::HashMap;
use std::time::Duration;

use chrono::{DateTime, Local, TimeZone};
use pigeonhole_protocols::protocols::katzenqt1::client::{ChatView, LineView};
use pigeonhole_protocols::protocols::katzenqt1::engine::{Notice, NoticeKind, Status, plain};

use crate::settings::Settings;
use crate::text;
use crate::voice;
use crate::{ChatItem, Delivery, LineItem, LineKind, MemberItem};

/// Lines from one sender within this long of each other share a name and avatar.
const RUN: i64 = 5 * 60 * 1000;

/// Facts about received files the model needs but the client does not keep: voice clip lengths
/// and sniffed types, by the attachment's file.
#[derive(Default)]
pub struct Files {
    pub durations: HashMap<String, Duration>,
    pub types: HashMap<String, String>,
}

pub fn time_of(ms: i64, now: DateTime<Local>) -> String {
    let Some(t) = Local.timestamp_millis_opt(ms).single() else { return String::new() };
    let days = (now.date_naive() - t.date_naive()).num_days();
    match days {
        0 => t.format("%H:%M").to_string(),
        1..=6 => t.format("%a").to_string(),
        _ => t.format("%d %b").to_string(),
    }
}

/// The name we show for a member: our own name for them if we gave one, else theirs, marked.
pub struct Who {
    pub shown: String,
    pub unverified: bool,
    pub fingerprint: String,
}

/// Names for every member of a group, with fingerprints added where two look alike.
pub fn names(view: &ChatView, settings: &Settings, me_name: &str) -> HashMap<Vec<u8>, Who> {
    let mut out: HashMap<Vec<u8>, Who> = HashMap::new();
    let mut skeletons: HashMap<String, Vec<Vec<u8>>> = HashMap::new();
    let mut add = |key: &[u8], claimed: &str| {
        let (shown, unverified) = match settings.petname(key) {
            Some(p) => (p.to_string(), false),
            None => (text::name(claimed), true),
        };
        skeletons.entry(text::skeleton_of(&shown)).or_default().push(key.to_vec());
        out.insert(key.to_vec(), Who { shown, unverified, fingerprint: String::new() });
    };
    if !view.me.is_empty() {
        add(&view.me, me_name);
    }
    for m in &view.members {
        add(&m.key, &m.name);
    }
    for keys in skeletons.values().filter(|k| k.len() > 1) {
        for k in keys {
            if let Some(w) = out.get_mut(k) {
                w.fingerprint = text::fingerprint(k);
            }
        }
    }
    if let Some(w) = out.get_mut(&view.me) {
        w.unverified = false;
    }
    out
}

/// A notice in the app's words, with any member-supplied names in it cleaned.
pub fn notice_text(n: &Notice) -> String {
    let n = Notice {
        kind: n.kind,
        args: n.args.iter().map(|a| text::identifier(a, 60)).collect(),
    };
    match n.kind {
        NoticeKind::VoucherMinted => "your voucher code is ready: give it to a member of the group".into(),
        NoticeKind::Added => "added a member: they now read everything said in this group".into(),
        _ => plain(&n),
    }
}

/// The latest failure in a group still joining, for its joining page; empty if there is none.
pub fn joining_note(lines: &[LineView]) -> String {
    lines
        .iter()
        .rev()
        .filter_map(|l| l.line.notice.as_ref())
        .find(|n| matches!(n.kind, NoticeKind::ActionFailed | NoticeKind::ReplyUnreadable))
        .map(notice_text)
        .unwrap_or_default()
}

pub fn line_item(
    l: &LineView,
    prev: Option<&LineView>,
    who: &HashMap<Vec<u8>, Who>,
    files: &Files,
    expanded: bool,
    now: DateTime<Local>,
) -> LineItem {
    let mut item = LineItem {
        id: l.line.id as i32,
        mine: l.mine,
        time: time_of(l.line.time, now).into(),
        ..LineItem::default()
    };
    if let Some(n) = &l.line.notice {
        item.kind = LineKind::Notice;
        item.text = notice_text(n).into();
        return item;
    }
    let w = who.get(&l.line.sender);
    item.from = w.map_or_else(|| text::name(&l.from), |w| w.shown.clone()).into();
    item.initial = text::initial(&item.from).into();
    item.unverified = w.is_none_or(|w| w.unverified) && !l.mine;
    item.fingerprint = w.map(|w| w.fingerprint.clone()).unwrap_or_default().into();
    item.tint = text::tint(&l.line.sender);
    item.first = prev.is_none_or(|p| p.line.notice.is_some() || p.line.sender != l.line.sender || l.line.time - p.line.time > RUN);
    item.delivery = match (l.mine, l.line.status, l.acked) {
        (false, _, _) => Delivery::None,
        (true, _, true) => Delivery::Read,
        (true, Status::Pending, _) => Delivery::Queued,
        (true, Status::Sent, _) => Delivery::Written,
    };
    item.read_by = "a member".into();
    match &l.line.attachment {
        Some(a) => {
            let name = text::identifier(&a.name, 200);
            if voice::is_voice(&a.filetype, &a.name) {
                item.kind = LineKind::Voice;
                let d = files.durations.get(&a.file).copied().unwrap_or_default();
                item.duration = voice::show_duration(d).into();
            } else {
                item.kind = LineKind::File;
            }
            item.file_size = text::file_size(a.size).into();
            item.file_type = files.types.get(&a.file).cloned().unwrap_or_else(|| "file".into()).into();
            item.file_name = name.into();
        }
        None => {
            item.kind = LineKind::Text;
            let (body, long) = text::body(&l.line.text);
            item.collapsed = long && !expanded;
            item.text = if item.collapsed { text::collapsed(&body) } else { body }.into();
        }
    }
    item
}

pub fn line_items(
    lines: &[LineView],
    who: &HashMap<Vec<u8>, Who>,
    files: &Files,
    expanded: &std::collections::HashSet<usize>,
    now: DateTime<Local>,
) -> Vec<LineItem> {
    lines
        .iter()
        .enumerate()
        .map(|(i, l)| line_item(l, i.checked_sub(1).map(|p| &lines[p]), who, files, expanded.contains(&l.line.id), now))
        .collect()
}

pub fn preview(l: &LineView, who: &HashMap<Vec<u8>, Who>) -> String {
    let from = if l.mine {
        "you".to_string()
    } else {
        who.get(&l.line.sender).map_or_else(|| text::name(&l.from), |w| w.shown.clone())
    };
    let what = match &l.line.attachment {
        Some(a) if voice::is_voice(&a.filetype, &a.name) => "voice message".to_string(),
        Some(a) => text::identifier(&a.name, 80),
        None => text::identifier(&l.line.text, 120),
    };
    format!("{from}: {what}")
}

pub fn chat_item(view: &ChatView, last: Option<&LineView>, unread: usize, who: &HashMap<Vec<u8>, Who>, now: DateTime<Local>) -> ChatItem {
    let name = text::name(&view.name);
    ChatItem {
        id: view.id.clone().into(),
        initial: text::initial(&name).into(),
        name: name.into(),
        preview: last.map(|l| preview(l, who)).unwrap_or_default().into(),
        time: last.map(|l| time_of(l.line.time, now)).unwrap_or_default().into(),
        unread: unread as i32,
        joining: !view.joined,
        members: view.members.len() as i32,
        tint: text::tint(view.id.as_bytes()),
    }
}

pub fn member_items(view: &ChatView, who: &HashMap<Vec<u8>, Who>) -> Vec<MemberItem> {
    let mut out: Vec<MemberItem> = view
        .members
        .iter()
        .map(|m| {
            let w = who.get(&m.key);
            let claimed = text::name(&m.name);
            let petname = w.filter(|w| !w.unverified).map(|w| w.shown.clone()).unwrap_or_default();
            MemberItem {
                key: hex::encode(&m.key).into(),
                fingerprint: text::fingerprint(&m.key).into(),
                initial: text::initial(if petname.is_empty() { &claimed } else { &petname }).into(),
                name: claimed.into(),
                petname: petname.into(),
                me: false,
                tint: text::tint(&m.key),
            }
        })
        .collect();
    out.sort_by(|a, b| {
        let ka = if a.petname.is_empty() { &a.name } else { &a.petname };
        let kb = if b.petname.is_empty() { &b.name } else { &b.petname };
        ka.to_lowercase().cmp(&kb.to_lowercase())
    });
    if !view.me.is_empty() {
        let me = who.get(&view.me).map(|w| w.shown.clone()).unwrap_or_else(|| "you".into());
        out.push(MemberItem {
            key: hex::encode(&view.me).into(),
            fingerprint: text::fingerprint(&view.me).into(),
            initial: text::initial(&me).into(),
            name: me.into(),
            petname: Default::default(),
            me: true,
            tint: text::tint(&view.me),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use pigeonhole_protocols::protocols::katzenqt1::client::MemberView;
    use pigeonhole_protocols::protocols::katzenqt1::engine::{Attachment, Line};

    fn view() -> ChatView {
        ChatView {
            id: "c1".into(),
            name: "friends".into(),
            joined: true,
            me: vec![0; 32],
            code: String::new(),
            minting: false,
            adding: false,
            members: vec![
                MemberView { key: vec![1; 32], name: "alice".into() },
                MemberView { key: vec![2; 32], name: "аlice".into() },
                MemberView { key: vec![3; 32], name: "bob\u{202E}evil".into() },
            ],
            unsent: 0,
        }
    }

    fn line(id: usize, sender: u8, text: &str, time: i64) -> LineView {
        LineView {
            line: Line { id, sender: vec![sender; 32], text: text.into(), time, ..Line::default() },
            mine: sender == 0,
            acked: false,
            from: "?".into(),
        }
    }

    fn now() -> DateTime<Local> {
        Local.timestamp_millis_opt(1_800_000_000_000).unwrap()
    }

    #[test]
    fn joining_shows_only_failures() {
        let mut l = vec![line(0, 0, "", 0), line(1, 0, "", 0)];
        l[0].line.notice = Some(Notice { kind: NoticeKind::ActionFailed, args: vec!["read timed out".into()] });
        l[1].line.notice = Some(Notice { kind: NoticeKind::VoucherMinted, args: vec![] });
        assert!(joining_note(&l).contains("read timed out"));
        assert_eq!(joining_note(&l[1..]), "");
    }

    #[test]
    fn lookalike_names_get_fingerprints() {
        let who = names(&view(), &Settings::default(), "me");
        assert!(!who[&vec![1; 32]].fingerprint.is_empty());
        assert!(!who[&vec![2; 32]].fingerprint.is_empty());
        assert!(who[&vec![3; 32]].fingerprint.is_empty());
        assert_eq!(who[&vec![3; 32]].shown, "bobevil");
        assert!(who[&vec![1; 32]].unverified);
        assert!(!who[&vec![0; 32]].unverified);
    }

    #[test]
    fn petnames_win_and_are_verified() {
        let mut s = Settings::default();
        s.set_petname(&hex::encode([2u8; 32]), "Alice from work");
        let who = names(&view(), &s, "me");
        let w = &who[&vec![2; 32]];
        assert_eq!(w.shown, "Alice from work");
        assert!(!w.unverified);
        assert!(who[&vec![1; 32]].fingerprint.is_empty());
        let members = member_items(&view(), &who);
        assert_eq!(members.len(), 4);
        assert!(members.last().unwrap().me);
        assert!(members.iter().any(|m| m.petname == "Alice from work"));
    }

    #[test]
    fn runs_and_delivery() {
        let v = view();
        let who = names(&v, &Settings::default(), "me");
        let t = now().timestamp_millis();
        let mut mine = line(3, 0, "hi", t + 2000);
        mine.line.status = Status::Pending;
        let lines = vec![line(1, 1, "a", t), line(2, 1, "b", t + 1000), mine.clone(), line(4, 1, "c", t + RUN + 5000)];
        let items = line_items(&lines, &who, &Files::default(), &Default::default(), now());
        assert!(items[0].first && !items[1].first && items[3].first);
        assert!(items[0].unverified);
        assert_eq!(items[2].delivery, Delivery::Queued);
        mine.line.status = Status::Sent;
        assert_eq!(line_item(&mine, None, &who, &Files::default(), false, now()).delivery, Delivery::Written);
        mine.acked = true;
        assert_eq!(line_item(&mine, None, &who, &Files::default(), false, now()).delivery, Delivery::Read);
    }

    #[test]
    fn long_bodies_collapse_until_expanded() {
        let who = HashMap::new();
        let l = line(1, 1, &"x\n".repeat(40), 0);
        let c = line_item(&l, None, &who, &Files::default(), false, now());
        assert!(c.collapsed);
        assert_eq!(c.text.lines().count(), text::LINES_MAX + 1);
        let e = line_item(&l, None, &who, &Files::default(), true, now());
        assert!(!e.collapsed);
    }

    #[test]
    fn attachments() {
        let who = HashMap::new();
        let mut l = line(1, 1, "", 0);
        l.line.attachment = Some(Attachment { name: "ab.opus".into(), filetype: "audio/opus".into(), size: 2000, file: "f1".into() });
        let mut files = Files::default();
        files.durations.insert("f1".into(), Duration::from_secs(12));
        let i = line_item(&l, None, &who, &files, false, now());
        assert_eq!(i.kind, LineKind::Voice);
        assert_eq!(i.duration, "0:12");
        l.line.attachment = Some(Attachment { name: "invoice\u{202E}fdp.exe".into(), filetype: "".into(), size: 5, file: "f2".into() });
        let i = line_item(&l, None, &who, &files, false, now());
        assert_eq!(i.kind, LineKind::File);
        assert_eq!(i.file_name, "invoicefdp.exe");
        assert_eq!(preview(&l, &who), "?: invoicefdp.exe");
    }

    #[test]
    fn notices_clean_their_arguments() {
        let n = Notice { kind: NoticeKind::MemberAdded, args: vec!["bob\u{202E}x".into(), "carol".into()] };
        assert_eq!(notice_text(&n), "bobx added ~carol");
    }
}
