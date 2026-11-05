//! Radio mode: in a group with it on, received voice clips play by themselves, one at a time, in
//! the order they arrive.

use std::collections::{HashMap, VecDeque};

use pigeonhole_protocols::protocols::katzenqt1::client::LineView;
use pigeonhole_protocols::protocols::katzenqt1::engine::{Attachment, ChatId};

use crate::voice;

#[derive(Default)]
pub struct Radio {
    /// The groups with radio on, and the last line in each already dealt with; `None` until the
    /// group's lines are known, so what was there before is never played.
    on: HashMap<ChatId, Option<usize>>,
    queue: VecDeque<(ChatId, usize)>,
}

impl Radio {
    pub fn is_on(&self, chat: &str) -> bool {
        self.on.contains_key(chat)
    }

    /// Turn radio on from after line `last`, or off, dropping what was queued for the group.
    pub fn set(&mut self, chat: &str, on: bool, last: Option<usize>) {
        if on {
            self.on.insert(chat.to_string(), last);
        } else {
            self.on.remove(chat);
            self.queue.retain(|(c, _)| c != chat);
        }
    }

    /// Queue the clips received in `chat` since last time; a clip whose file has not arrived
    /// holds back those after it.
    pub fn arrived(&mut self, chat: &str, lines: &[LineView], ready: impl Fn(&Attachment) -> bool) {
        let Some(after) = self.on.get_mut(chat) else { return };
        let Some(from) = *after else {
            *after = Some(lines.last().map_or(0, |l| l.line.id));
            return;
        };
        for l in lines.iter().filter(|l| l.line.id > from) {
            match &l.line.attachment {
                Some(a) if !l.mine && voice::is_voice(&a.filetype, &a.name) => {
                    if !ready(a) {
                        return;
                    }
                    self.queue.push_back((chat.to_string(), l.line.id));
                }
                _ => {}
            }
            *after = Some(l.line.id);
        }
    }

    pub fn next(&mut self) -> Option<(ChatId, usize)> {
        self.queue.pop_front()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pigeonhole_protocols::protocols::katzenqt1::engine::Line;

    fn line(id: usize, mine: bool, voice: bool) -> LineView {
        let attachment = voice.then(|| Attachment { name: format!("{id}.opus"), filetype: "audio/opus".into(), size: 1, file: format!("f{id}") });
        LineView { line: Line { id, attachment, ..Line::default() }, mine, acked: false, from: "?".into() }
    }

    fn all(r: &mut Radio) -> Vec<(ChatId, usize)> {
        std::iter::from_fn(|| r.next()).collect()
    }

    #[test]
    fn plays_only_what_arrives_after_turning_on() {
        let mut r = Radio::default();
        let mut lines = vec![line(1, false, true)];
        r.arrived("g", &lines, |_| true);
        assert!(all(&mut r).is_empty(), "off: nothing plays");
        r.set("g", true, Some(1));
        lines.extend([line(2, false, true), line(3, true, true), line(4, false, false), line(5, false, true)]);
        r.arrived("g", &lines, |_| true);
        assert_eq!(all(&mut r), vec![("g".into(), 2), ("g".into(), 5)], "in order, never our own");
        r.arrived("g", &lines, |_| true);
        assert!(all(&mut r).is_empty(), "each clip once");
    }

    #[test]
    fn a_clip_not_yet_here_holds_back_the_ones_after_it() {
        let mut r = Radio::default();
        r.set("g", true, Some(0));
        let lines = vec![line(1, false, true), line(2, false, true)];
        r.arrived("g", &lines, |a| a.file != "f1");
        assert!(all(&mut r).is_empty());
        r.arrived("g", &lines, |_| true);
        assert_eq!(all(&mut r), vec![("g".into(), 1), ("g".into(), 2)]);
    }

    #[test]
    fn groups_queue_together_in_arrival_order() {
        let mut r = Radio::default();
        r.set("a", true, Some(0));
        r.set("b", true, Some(0));
        r.arrived("a", &[line(1, false, true)], |_| true);
        r.arrived("b", &[line(1, false, true)], |_| true);
        r.arrived("a", &[line(1, false, true), line(2, false, true)], |_| true);
        assert_eq!(r.next(), Some(("a".into(), 1)));
        r.set("a", false, None);
        assert_eq!(all(&mut r), vec![("b".into(), 1)], "turning off drops the group's queue");
    }

    #[test]
    fn history_known_at_start_is_not_played() {
        let mut r = Radio::default();
        r.set("g", true, None);
        let mut lines = vec![line(1, false, true), line(2, false, true)];
        r.arrived("g", &lines, |_| true);
        assert!(all(&mut r).is_empty());
        lines.push(line(3, false, true));
        r.arrived("g", &lines, |_| true);
        assert_eq!(all(&mut r), vec![("g".into(), 3)]);
    }
}
