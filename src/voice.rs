//! Push to talk over rustic_audio: clips are Ogg Opus, 48 kHz mono, sent as an ordinary file named
//! by the SHA-256 of its bytes, as frank_chat and katzenqt send them.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rustic_audio::{VoiceClip, VoiceNoteEngine};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

/// Shorter clips are taken for a slip of the finger and dropped.
pub const MIN_CLIP: Duration = Duration::from_millis(300);
/// Recording stops on its own after this long.
pub const MAX_CLIP: Duration = Duration::from_secs(600);

/// The name a clip is sent under.
pub fn clip_name(bytes: &[u8]) -> String {
    format!("{}.opus", hex::encode(Sha256::digest(bytes)))
}

/// Whether a received file is a voice clip we can play.
pub fn is_voice(filetype: &str, name: &str) -> bool {
    filetype == "audio/opus" || name.ends_with(".opus")
}

/// The length of an Ogg Opus stream: the last page's granule position, less the pre-skip, at
/// 48 kHz.
pub fn duration(bytes: &[u8]) -> Option<Duration> {
    let head = bytes.windows(8).position(|w| w == b"OpusHead")?;
    let pre_skip = u16::from_le_bytes(bytes.get(head + 10..head + 12)?.try_into().ok()?) as u64;
    let last = bytes.windows(4).rposition(|w| w == b"OggS")?;
    let granule = u64::from_le_bytes(bytes.get(last + 6..last + 14)?.try_into().ok()?);
    if granule == u64::MAX {
        return None;
    }
    let samples = granule.saturating_sub(pre_skip);
    Some(Duration::from_micros(samples.checked_mul(1_000_000)? / 48_000))
}

pub fn show_duration(d: Duration) -> String {
    let s = d.as_secs_f64().round() as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

/// A clip ready to send, in guarded memory: it is never written to a file.
pub struct Clip {
    pub name: String,
    pub audio: VoiceClip,
}

/// What is playing: which line, since when, for how long.
#[derive(Clone, Debug, PartialEq)]
pub struct Playing {
    pub chat: String,
    pub line: usize,
    pub started: Instant,
    pub length: Duration,
}

pub struct Voice {
    cache: PathBuf,
    engine: Arc<Mutex<Option<VoiceNoteEngine>>>,
    recording_since: Option<Instant>,
    pub playing: Option<Playing>,
}

impl Voice {
    pub fn new(cache: PathBuf) -> Voice {
        Voice {
            cache,
            engine: Arc::new(Mutex::new(None)),
            recording_since: None,
            playing: None,
        }
    }

    fn with_engine<T>(&self, f: impl FnOnce(&mut VoiceNoteEngine) -> Result<T, String>) -> Result<T, String> {
        let mut e = self.engine.lock().map_err(|_| "audio engine poisoned".to_string())?;
        if e.is_none() {
            *e = Some(VoiceNoteEngine::new(&self.cache)?);
        }
        f(e.as_mut().unwrap())
    }

    pub fn recording(&self) -> Option<Duration> {
        self.recording_since.map(|t| t.elapsed())
    }

    /// Open the microphone. Only ever called for the user pressing the talk button.
    pub fn start(&mut self, _: &crate::bridge::UserIntent) -> Result<(), String> {
        if self.recording_since.is_some() {
            return Ok(());
        }
        self.stop_playing();
        self.with_engine(|e| e.start_capture_in_memory())?;
        self.recording_since = Some(Instant::now());
        Ok(())
    }

    /// Close the microphone and encode the clip, off the caller's thread; `done` gets the clip, or
    /// `None` when it was too short to keep.
    pub fn stop(&mut self, done: impl FnOnce(Result<Option<Clip>, String>) + Send + 'static) {
        let Some(since) = self.recording_since.take() else { return };
        let engine = self.engine.clone();
        std::thread::spawn(move || {
            let r = (|| {
                let audio = {
                    let mut e = engine.lock().map_err(|_| "audio engine poisoned".to_string())?;
                    e.as_mut().ok_or("no audio engine")?.stop_capture_in_memory()?
                };
                if since.elapsed() < MIN_CLIP || audio.duration_seconds() < MIN_CLIP.as_secs_f64() {
                    return Ok(None);
                }
                Ok(Some(Clip { name: clip_name(audio.bytes()), audio }))
            })();
            done(r);
        });
    }

    pub fn cancel(&mut self) {
        if self.recording_since.take().is_some() {
            let _ = self.with_engine(|e| e.cancel_capture().map(|_| ()));
        }
    }

    /// Play a received clip from memory: a clip kept sealed is never written out to be played.
    pub fn play(&mut self, chat: &str, line: usize, mut bytes: Zeroizing<Vec<u8>>) -> Result<(), String> {
        self.stop_playing();
        let length = duration(&bytes).unwrap_or_default();
        let bytes = std::mem::take(&mut *bytes);
        self.with_engine(move |e| e.play_received_bytes(bytes))?;
        self.playing = Some(Playing {
            chat: chat.into(),
            line,
            started: Instant::now(),
            length,
        });
        Ok(())
    }

    pub fn stop_playing(&mut self) {
        if self.playing.take().is_some() {
            let _ = self.with_engine(|e| e.stop_playback());
        }
    }

    /// How far the clip has played, from 0 to 1; clears `playing` once it has finished.
    pub fn progress(&mut self) -> Option<f32> {
        let p = self.playing.as_ref()?;
        let done = !self.with_engine(|e| Ok(e.is_playing())).unwrap_or(false);
        if done && p.started.elapsed() > Duration::from_millis(300) {
            self.playing = None;
            return None;
        }
        let frac = if p.length.is_zero() { 0.0 } else { p.started.elapsed().as_secs_f32() / p.length.as_secs_f32() };
        Some(frac.min(1.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal Ogg Opus stream: a head page and one audio page at `granule`.
    fn ogg(pre_skip: u16, granule: u64) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend(b"OggS\0\x02");
        b.extend(0u64.to_le_bytes());
        b.extend([0u8; 13]);
        b.extend(b"OpusHead\x01\x01");
        b.extend(pre_skip.to_le_bytes());
        b.extend(48000u32.to_le_bytes());
        b.extend(b"OggS\0\x04");
        b.extend(granule.to_le_bytes());
        b.extend([0u8; 13]);
        b
    }

    #[test]
    fn duration_from_granule() {
        assert_eq!(duration(&ogg(312, 312 + 48_000 * 3)), Some(Duration::from_secs(3)));
        assert_eq!(duration(b"not ogg"), None);
        assert_eq!(duration(&ogg(0, u64::MAX - 1)), None, "a granule that overflows is not a length");
    }

    #[test]
    fn frank_chat_clip() {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/tone.opus");
        let Ok(b) = std::fs::read(p) else { return };
        let d = duration(&b).unwrap();
        assert!(d > Duration::from_millis(300) && d < Duration::from_secs(60), "{d:?}");
    }

    #[test]
    fn clips_are_named_by_hash() {
        let n = clip_name(b"abc");
        assert_eq!(n, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad.opus");
        assert!(is_voice("audio/opus", "x"));
        assert!(is_voice("", &n));
        assert!(!is_voice("image/png", "x.png"));
    }

    #[test]
    fn durations_show_as_minutes() {
        assert_eq!(show_duration(Duration::from_secs(4)), "0:04");
        assert_eq!(show_duration(Duration::from_secs(75)), "1:15");
    }
}
