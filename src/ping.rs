//! Ping: a katzenqt1 message that asks the group's members to play their ping sound, with the
//! T-Deck Radio app's sounds.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use pigeonhole_protocols::protocols::katzenqt1::client::LineView;
use pigeonhole_protocols::protocols::katzenqt1::engine::{ChatId, Notice, NoticeKind};

/// What the sender hears.
pub const SENT: &[u8] = include_bytes!("../sounds/ping-sent.wav");
/// What its readers hear.
pub const RECEIVED: &[u8] = include_bytes!("../sounds/ping-received.wav");
/// At most one ping this often.
pub const EVERY: Duration = Duration::from_secs(3);

#[derive(Default)]
pub struct Pings {
    last_sent: Option<Instant>,
    /// The last line of each group already looked at; set the first time its lines are seen, so
    /// the pings already there are never played.
    seen: HashMap<ChatId, usize>,
}

impl Pings {
    /// Whether a ping may go at `now`; if so, it counts as sent.
    pub fn may_send(&mut self, now: Instant) -> bool {
        if self.last_sent.is_some_and(|t| now.duration_since(t) < EVERY) {
            return false;
        }
        self.last_sent = Some(now);
        true
    }

    /// The pings that arrived in `chat` since last time.
    pub fn arrived<'a>(&mut self, chat: &str, lines: &'a [LineView]) -> Vec<&'a Notice> {
        let last = lines.last().map_or(0, |l| l.line.id);
        let Some(from) = self.seen.insert(chat.to_string(), last) else { return Vec::new() };
        lines
            .iter()
            .filter(|l| l.line.id > from)
            .filter_map(|l| l.line.notice.as_ref())
            .filter(|n| n.kind == NoticeKind::Pinged)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pigeonhole_protocols::protocols::katzenqt1::engine::Line;

    fn line(id: usize, ping: bool) -> LineView {
        let notice = ping.then(|| Notice { kind: NoticeKind::Pinged, args: vec![format!("~m{id}")] });
        LineView { line: Line { id, notice, ..Line::default() }, mine: false, acked: false, from: "?".into() }
    }

    #[test]
    fn one_ping_every_few_seconds() {
        let mut p = Pings::default();
        let t = Instant::now();
        assert!(p.may_send(t));
        assert!(!p.may_send(t + Duration::from_secs(1)));
        assert!(!p.may_send(t + EVERY - Duration::from_millis(1)));
        assert!(p.may_send(t + EVERY));
        assert!(!p.may_send(t + EVERY + Duration::from_secs(2)), "the refused ones do not count");
    }

    #[test]
    fn only_new_pings_play_once() {
        let mut p = Pings::default();
        let mut lines = vec![line(1, true), line(2, false)];
        assert!(p.arrived("g", &lines).is_empty(), "the pings there at start");
        lines.extend([line(3, true), line(4, false), line(5, true)]);
        let got: Vec<_> = p.arrived("g", &lines).iter().map(|n| n.args[0].clone()).collect();
        assert_eq!(got, ["~m3", "~m5"]);
        assert!(p.arrived("g", &lines).is_empty(), "each once");
        assert!(p.arrived("h", &lines).is_empty(), "groups apart");
    }

    #[test]
    fn sounds_are_wavs() {
        for w in [SENT, RECEIVED] {
            assert_eq!(&w[..4], b"RIFF");
            assert_eq!(&w[8..12], b"WAVE");
        }
    }
}
