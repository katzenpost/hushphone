//! Voucher codes by sound, as the T-Deck sends and hears them, through the speaker and the
//! microphone at whatever rate they run: ggwave's Audible Fast, the voucher's 32 raw bytes, as
//! pigeonhole_reticulous's docs/sound-vouchers.md sets out.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};

use ggwave::{Decoder, Encoder, Parameters, Protocol};
use sha3::Shake256;
use sha3::digest::{ExtendableOutput, Update, XofReader};

use crate::bridge::UserIntent;

/// A voucher's length: what goes over the air, raw.
pub const PAYLOAD: usize = 32;
/// How long Listen waits, as on the T-Deck.
pub const LISTEN: Duration = Duration::from_secs(15);
/// The T-Deck's level (`ggwave_encode`'s 0-100): peaks at a quarter of full scale, quiet, for a
/// device 10-30 cm away.
const VOLUME: u8 = 25;

/// The 4 digits both screens show for a voucher, so that the two people can tell it arrived
/// whole: SHAKE256 of the voucher, its first 4 bytes big-endian, mod 10000.
pub fn check(voucher: &[u8]) -> String {
    let mut h = Shake256::default();
    h.update(voucher);
    let mut d = [0u8; 4];
    h.finalize_xof().read(&mut d);
    format!("{:04}", u32::from_be_bytes(d) % 10000)
}

/// ggwave at its defaults, 48 kHz with 1024-sample frames, heard or played at `fs`.
fn params(fs: u32) -> Parameters {
    Parameters { sample_rate_inp: fs as f32, sample_rate_out: fs as f32, ..Parameters::default() }
}

/// The voucher as the speaker plays it at `fs`: 0.2 s of quiet while it comes up, the 2.73 s
/// message, 0.3 s after.
fn clip(voucher: &[u8; PAYLOAD], fs: u32) -> Result<Vec<f32>, String> {
    let tones = Encoder::new(params(fs)).and_then(|e| e.encode(voucher, Protocol::AudibleFast, VOLUME));
    let mut clip = vec![0f32; fs as usize / 5];
    clip.extend(tones.map_err(|e| format!("the speaker runs at {fs} Hz: {e}"))?);
    clip.extend(vec![0f32; fs as usize * 3 / 10]);
    Ok(clip)
}

/// Hears vouchers in the microphone's samples at `fs`: Audible Fast only, 32 bytes only.
struct Ear(Decoder);

impl Ear {
    fn new(fs: u32) -> Result<Ear, String> {
        Decoder::with_protocols(params(fs), &[Protocol::AudibleFast])
            .map(Ear)
            .map_err(|e| format!("the microphone runs at {fs} Hz: {e}"))
    }

    fn push(&mut self, x: &[f32]) -> Option<[u8; PAYLOAD]> {
        self.0.push(x)?.try_into().ok()
    }
}

/// Play the voucher. Returns once it has played.
pub fn send(_: &UserIntent, voucher: &[u8; PAYLOAD]) -> Result<(), String> {
    out(|fs| clip(voucher, fs))
}

/// Play a 16-bit mono PCM WAV, such as a ping sound. Returns once it has played.
pub fn chime(wav: &[u8]) -> Result<(), String> {
    let (rate, pcm) = wav_pcm(wav).ok_or("not a 16-bit mono WAV")?;
    out(|fs| Ok(resample(&pcm, rate, fs)))
}

/// The rate and samples of a 16-bit mono PCM WAV.
fn wav_pcm(b: &[u8]) -> Option<(u32, Vec<f32>)> {
    if b.get(..4)? != b"RIFF" || b.get(8..12)? != b"WAVE" {
        return None;
    }
    let (mut at, mut rate) = (12, None);
    while at + 8 <= b.len() {
        let len = u32::from_le_bytes(b[at + 4..at + 8].try_into().ok()?) as usize;
        let body = b.get(at + 8..(at + 8).checked_add(len)?.min(b.len()))?;
        match &b[at..at + 4] {
            b"fmt " => {
                let u16_at = |i: usize| Some(u16::from_le_bytes(body.get(i..i + 2)?.try_into().ok()?));
                if u16_at(0)? != 1 || u16_at(2)? != 1 || u16_at(14)? != 16 {
                    return None;
                }
                rate = Some(u32::from_le_bytes(body.get(4..8)?.try_into().ok()?));
            }
            b"data" => return Some((rate?, body.as_chunks::<2>().0.iter().map(|&s| i16::from_le_bytes(s) as f32 / 32768.0).collect())),
            _ => {}
        }
        at += 8 + len + len % 2;
    }
    None
}

/// `x` at rate `from`, linearly interpolated to rate `to`.
fn resample(x: &[f32], from: u32, to: u32) -> Vec<f32> {
    let n = (x.len() as u64 * to as u64 / from.max(1) as u64) as usize;
    (0..n)
        .map(|i| {
            let p = i as f64 * from as f64 / to as f64;
            let (k, f) = (p as usize, (p.fract()) as f32);
            let a = x.get(k).copied().unwrap_or(0.0);
            a + (x.get(k + 1).copied().unwrap_or(a) - a) * f
        })
        .collect()
}

/// Play what `make` makes for the speaker's rate on the default speaker, and wait until it has.
fn out(make: impl FnOnce(u32) -> Result<Vec<f32>, String>) -> Result<(), String> {
    let dev = cpal::default_host().default_output_device().ok_or("no speaker")?;
    let cfg = dev.default_output_config().map_err(|e| e.to_string())?;
    let (fs, format) = (cfg.sample_rate(), cfg.sample_format());
    let clip = make(fs)?;
    let (clip, at) = (Arc::new(clip), Arc::new(AtomicUsize::new(0)));
    let cfg = cfg.config();
    let stream = match format {
        SampleFormat::F32 => play::<f32>(&dev, &cfg, clip.clone(), at.clone()),
        SampleFormat::I16 => play::<i16>(&dev, &cfg, clip.clone(), at.clone()),
        SampleFormat::U16 => play::<u16>(&dev, &cfg, clip.clone(), at.clone()),
        SampleFormat::I32 => play::<i32>(&dev, &cfg, clip.clone(), at.clone()),
        f => return Err(format!("the speaker takes {f} samples")),
    }?;
    stream.play().map_err(|e| e.to_string())?;
    let until = Instant::now() + Duration::from_secs_f64(clip.len() as f64 / fs as f64) + Duration::from_secs(2);
    while at.load(Ordering::Relaxed) < clip.len() {
        if Instant::now() > until {
            return Err("the speaker stopped".into());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Ok(())
}

fn play<T: SizedSample + FromSample<f32>>(
    dev: &cpal::Device,
    cfg: &cpal::StreamConfig,
    clip: Arc<Vec<f32>>,
    at: Arc<AtomicUsize>,
) -> Result<cpal::Stream, String> {
    let ch = cfg.channels.max(1) as usize;
    dev.build_output_stream(
        cfg,
        move |data: &mut [T], _| {
            for frame in data.chunks_mut(ch) {
                let v = clip.get(at.fetch_add(1, Ordering::Relaxed)).copied().unwrap_or(0.0);
                frame.fill(T::from_sample(v));
            }
        },
        |e| eprintln!("hushphone: speaker: {e}"),
        None,
    )
    .map_err(|e| e.to_string())
}

/// Listen for a voucher for up to `LISTEN`, or until `stop` is set: `None` then. Nothing heard is
/// an error that says how loud the microphone was.
pub fn listen(_: &UserIntent, stop: &AtomicBool) -> Result<Option<[u8; PAYLOAD]>, String> {
    let dev = cpal::default_host().default_input_device().ok_or("no microphone")?;
    let cfg = dev.default_input_config().map_err(|e| e.to_string())?;
    let (fs, format) = (cfg.sample_rate(), cfg.sample_format());
    let heard = Arc::new(Mutex::new(Vec::<f32>::new()));
    let cfg = cfg.config();
    let stream = match format {
        SampleFormat::F32 => hear::<f32>(&dev, &cfg, heard.clone()),
        SampleFormat::I16 => hear::<i16>(&dev, &cfg, heard.clone()),
        SampleFormat::U16 => hear::<u16>(&dev, &cfg, heard.clone()),
        SampleFormat::I32 => hear::<i32>(&dev, &cfg, heard.clone()),
        f => return Err(format!("the microphone gives {f} samples")),
    }?;
    stream.play().map_err(|e| e.to_string())?;
    let (mut ear, mut peak) = (Ear::new(fs)?, 0f32);
    let until = Instant::now() + LISTEN;
    while Instant::now() < until {
        if stop.load(Ordering::Relaxed) {
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(20));
        let got = std::mem::take(&mut *heard.lock().map_err(|_| "microphone poisoned")?);
        peak = got.iter().fold(peak, |p, &s| p.max(s.abs()));
        if let Some(v) = ear.push(&got) {
            return Ok(Some(v));
        }
    }
    Err(format!(
        "heard no voucher in {} s (mic peak {:.0} dBFS)",
        LISTEN.as_secs(),
        20.0 * peak.max(1.0 / 32768.0).log10()
    ))
}

fn hear<T: SizedSample>(dev: &cpal::Device, cfg: &cpal::StreamConfig, heard: Arc<Mutex<Vec<f32>>>) -> Result<cpal::Stream, String>
where
    f32: FromSample<T>,
{
    let ch = cfg.channels.max(1) as usize;
    // A listener that falls 2 s behind drops what comes after, rather than grow without bound.
    let most = cfg.sample_rate as usize * 2;
    dev.build_input_stream(
        cfg,
        move |data: &[T], _| {
            if let Ok(mut h) = heard.lock()
                && h.len() < most
            {
                h.extend(data.chunks(ch).map(|f| f[0].to_sample::<f32>()));
            }
        },
        |e| eprintln!("hushphone: microphone: {e}"),
        None,
    )
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pigeonhole_protocols::protocols::katzenqt1::engine::{parse_voucher_code, voucher_code};
    use rand::{Rng, SeedableRng};

    /// The voucher of pigeonhole_reticulous's voucher vector, which the T-Deck's fixtures carry.
    const VECTOR: &str = "f2ca3f0fe0fdaa414c9e55ebcd95495de017271f1c06a56c8a1bbf856cae6d6f";

    fn vector() -> [u8; 32] {
        hex::decode(VECTOR).unwrap().try_into().unwrap()
    }

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/ggwave").join(name)).unwrap()
    }

    /// What Listen hears from `x`, in the 20 ms blocks it takes from the microphone.
    fn hear(x: &[f32], fs: u32) -> Option<[u8; 32]> {
        let mut ear = Ear::new(fs).unwrap();
        x.chunks(fs as usize / 50).find_map(|c| ear.push(c))
    }

    /// Gaussian noise, `sd` the standard deviation.
    fn noise(r: &mut impl Rng, sd: f32, n: usize) -> Vec<f32> {
        (0..n).map(|_| sd * (-2.0 * r.r#gen::<f32>().max(1e-9).ln()).sqrt() * (std::f32::consts::TAU * r.r#gen::<f32>()).cos()).collect()
    }

    #[test]
    fn check_code_matches_the_t_deck() {
        // docs/sound-vouchers.md: the vector's voucher checks as 1939.
        assert_eq!(check(&vector()), "1939");
        assert_eq!(voucher_code(&vector()), "8so/D+D9qkFMnlXrzZVJXeAXJx8cBqVsihu/hWyubW8=");
        assert_eq!(parse_voucher_code(&voucher_code(&vector())).as_deref(), Some(&vector()[..]));
    }

    #[test]
    fn round_trip_through_noise() {
        let mut r = rand::rngs::StdRng::seed_from_u64(1);
        for fs in [48000, 44100, 16000] {
            let v: [u8; 32] = r.r#gen();
            let mut x = clip(&v, fs).unwrap();
            assert!((x.len() as f64 / fs as f64 - 3.23).abs() < 0.01, "{fs} Hz: {} samples", x.len());
            let rms = (x.iter().map(|s| s * s).sum::<f32>() / x.len() as f32).sqrt();
            // 0 dB SNR across the whole band; arriving 0.7 s into what the microphone hears.
            let mut heard = noise(&mut r, rms, fs as usize * 7 / 10);
            heard.append(&mut x);
            let n = noise(&mut r, rms, heard.len());
            heard.iter_mut().zip(n).for_each(|(s, n)| *s += n);
            assert_eq!(hear(&heard, fs), Some(v), "{fs} Hz");
        }
    }

    #[test]
    fn nothing_in_noise_and_only_32_bytes() {
        let mut r = rand::rngs::StdRng::seed_from_u64(2);
        assert_eq!(hear(&noise(&mut r, 0.05, 48000 * 6), 48000), None);
        let short = Encoder::new(params(48000)).unwrap().encode(&[7; 31], Protocol::AudibleFast, VOLUME).unwrap();
        assert_eq!(hear(&short, 48000), None);
        let other = Encoder::new(params(48000)).unwrap().encode(&vector(), Protocol::AudibleNormal, VOLUME).unwrap();
        assert_eq!(hear(&other, 48000), None, "Audible Fast only");
    }

    #[test]
    fn hears_c_ggwave() {
        // ggwave-rs's tests/vectors/p1-var-32-48000.i16: C ggwave 060aec7's Audible Fast, 32 bytes
        // of variable length, at 48 kHz, volume 50.
        let x: Vec<f32> = fixture("p1-var-32-48000.i16").as_chunks::<2>().0.iter().map(|&b| i16::from_le_bytes(b) as f32 / 32768.0).collect();
        let want = hex::decode("7a1f30d5ee83a4791237c8ed865b7c112acfe0855e731429c2e7b85d760b2cc1").unwrap();
        assert_eq!(hear(&x, 48000).map(|v| v.to_vec()), Some(want));
        // pigeonhole_reticulous's esp-idf/test/host/vectors/ggwave: the vector's voucher as the
        // T-Deck plays it at 16 kHz, and through its simulated room at 10 and 0 dB SNR.
        for f in ["tx.wav", "room-10dB.wav", "room-0dB.wav"] {
            let (fs, x) = wav_pcm(&fixture(f)).unwrap();
            assert_eq!(hear(&x, fs), Some(vector()), "{f}");
        }
    }

    #[test]
    fn ping_sounds_read_and_resample() {
        for w in [crate::ping::SENT, crate::ping::RECEIVED] {
            let (rate, pcm) = wav_pcm(w).unwrap();
            assert_eq!(rate, 16000);
            assert!(pcm.len() > 1000 && pcm.iter().any(|&s| s.abs() > 0.05));
            assert_eq!(resample(&pcm, 16000, 48000).len(), pcm.len() * 3);
        }
        assert_eq!(resample(&[0.0, 1.0], 1, 2), [0.0, 0.5, 1.0, 1.0]);
        assert!(wav_pcm(b"RIFF\0\0\0\0WAVE").is_none());
        assert!(wav_pcm(&crate::ping::SENT[..40]).is_none(), "cut short");
    }
}
