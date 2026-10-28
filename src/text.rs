//! How text from other members is shown: names and file names lose the controls that would
//! reorder them, names are clamped, and identities are compared by confusable skeleton.

use sha2::{Digest, Sha256};
use unicode_normalization::UnicodeNormalization;
use unicode_security::confusable_detection::skeleton;
use unicode_segmentation::UnicodeSegmentation;

/// The longest display name katzenqt accepts.
pub const NAME_MAX: usize = 30;
/// Bodies longer than this are cut, with a marker.
pub const BODY_MAX: usize = 64 * 1024;
/// Bodies with more lines than this start collapsed.
pub const LINES_MAX: usize = 20;

fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200E}' | '\u{200F}' | '\u{061C}')
}

fn is_hidden(c: char) -> bool {
    matches!(c, '\u{200B}'..='\u{200D}' | '\u{2060}'..='\u{2064}' | '\u{FEFF}' | '\u{180E}' | '\u{E0000}'..='\u{E007F}')
}

/// A name or file name for one line of display: no bidi controls, invisible characters or line
/// breaks, and at most `max` characters.
pub fn identifier(s: &str, max: usize) -> String {
    let mut out: String = s
        .chars()
        .filter(|&c| !is_bidi_control(c) && !is_hidden(c))
        .map(|c| if c.is_control() || matches!(c, '\u{2028}' | '\u{2029}' | '\u{85}') { '\u{FFFD}' } else { c })
        .collect::<String>()
        .trim()
        .to_string();
    if out.chars().count() > max {
        out = out.chars().take(max - 1).collect::<String>() + "…";
    }
    out
}

pub fn name(s: &str) -> String {
    let n = identifier(s, NAME_MAX);
    if n.is_empty() { "?".into() } else { n }
}

/// A message body: control characters other than newline and tab are shown as U+FFFD, and a
/// body over [`BODY_MAX`] bytes is cut. Returns the text and whether it should start collapsed.
pub fn body(s: &str) -> (String, bool) {
    let mut out: String = s
        .chars()
        .map(|c| if c.is_control() && c != '\n' && c != '\t' { '\u{FFFD}' } else { c })
        .collect();
    if out.len() > BODY_MAX {
        let mut cut = BODY_MAX;
        while !out.is_char_boundary(cut) {
            cut -= 1;
        }
        out.truncate(cut);
        out.push_str("\n[… cut: the message was longer than 64 KiB]");
    }
    let long = out.lines().count() > LINES_MAX;
    (breakable(&out), long)
}

/// A run of this many characters without a space gets a place to break, so that one long word
/// cannot make its bubble wider than the screen.
const RUN_MAX: usize = 24;

fn breakable(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut run = 0;
    for g in s.graphemes(true) {
        if g.chars().all(char::is_whitespace) {
            run = 0;
        } else {
            run += 1;
            if run > RUN_MAX {
                out.push('\u{200B}');
                run = 1;
            }
        }
        out.push_str(g);
    }
    out
}

/// The first `LINES_MAX` lines of a long body.
pub fn collapsed(s: &str) -> String {
    let mut out: Vec<&str> = s.lines().take(LINES_MAX).collect();
    out.push("…");
    out.join("\n")
}

/// The first grapheme of a name, upper-cased, for an avatar.
pub fn initial(s: &str) -> String {
    s.graphemes(true)
        .find(|g| !g.trim().is_empty())
        .map(|g| g.to_uppercase())
        .unwrap_or_else(|| "?".into())
}

/// A short, stable fingerprint of a member's key.
pub fn fingerprint(key: &[u8]) -> String {
    hex::encode(&Sha256::digest(key)[..4])
}

/// A tint index picked by the key, so that a member cannot choose their own colour.
pub fn tint(key: &[u8]) -> i32 {
    (Sha256::digest(key)[4] % 8) as i32
}

/// The UTS #39 skeleton of a name after NFKC and case folding: two names with the same skeleton
/// look alike.
pub fn skeleton_of(s: &str) -> String {
    let folded: String = s.nfkc().collect::<String>().to_lowercase();
    skeleton(&folded).collect()
}

/// Escape what a notification server might read as markup.
pub fn notification(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

pub fn file_size(n: usize) -> String {
    match n {
        n if n < 1024 => format!("{n} B"),
        n if n < 1024 * 1024 => format!("{} KB", n / 1024),
        n => format!("{:.1} MB", n as f64 / (1024.0 * 1024.0)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rlo_cannot_reorder_a_file_name() {
        assert_eq!(identifier("invoice\u{202E}fdp.exe", 100), "invoicefdp.exe");
    }

    #[test]
    fn names_are_clamped_and_one_line() {
        let n = name(&"x".repeat(50));
        assert_eq!(n.chars().count(), NAME_MAX);
        assert!(n.ends_with('…'));
        assert_eq!(name("bob\u{2028}admin"), "bob\u{FFFD}admin");
        assert_eq!(name("\u{200B}"), "?");
    }

    #[test]
    fn bodies_keep_newlines_and_mark_controls() {
        assert_eq!(body("a\nb\u{1b}[31m").0, "a\nb\u{FFFD}[31m");
        assert!(!body("short").1);
        assert!(body(&"l\n".repeat(30)).1);
        let (big, _) = body(&"é ".repeat(BODY_MAX));
        assert!(big.len() < BODY_MAX + 100);
        assert!(big.ends_with("64 KiB]"));
    }

    #[test]
    fn long_words_get_places_to_break() {
        let (b, _) = body(&"x".repeat(60));
        assert_eq!(b.matches('\u{200B}').count(), 2);
        assert_eq!(body("short words stay as they are").0, "short words stay as they are");
        let (f, _) = body(&"👩‍👩‍👧".repeat(30));
        assert!(f.split('\u{200B}').all(|p| p.graphemes(true).count() <= RUN_MAX), "breaks fall between graphemes");
    }

    #[test]
    fn initials_are_one_grapheme() {
        assert_eq!(initial("bob"), "B");
        assert_eq!(initial("  كريم"), "ك");
        assert_eq!(initial("👩‍👩‍👧 family"), "👩‍👩‍👧");
        assert_eq!(initial(""), "?");
    }

    #[test]
    fn confusable_names_share_a_skeleton() {
        assert_eq!(skeleton_of("alice"), skeleton_of("аlice"));
        assert_eq!(skeleton_of("Bob"), skeleton_of("bob"));
        assert_ne!(skeleton_of("bob"), skeleton_of("rob"));
    }

    #[test]
    fn notifications_escape_markup() {
        assert_eq!(notification("<b>&"), "&lt;b&gt;&amp;");
    }
}
