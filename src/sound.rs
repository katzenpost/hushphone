//! Voucher codes by sound, as the T-Deck sends and hears them (see `fskv`), through the speaker
//! and the microphone at whatever rate they run.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};

use crate::bridge::UserIntent;
use crate::fskv;

/// How long Listen waits, as on the T-Deck.
pub const LISTEN: Duration = Duration::from_secs(15);
/// The T-Deck's level, -20 dBFS RMS: quiet, for a device 10-30 cm away.
const AMP: f32 = 9268.0;

/// Play the voucher: 0.2 s of quiet while the speaker comes up, the 1.2 s frame, 0.3 s after.
/// Returns once it has played.
pub fn send(_: &UserIntent, voucher: &[u8; fskv::PAYLOAD]) -> Result<(), String> {
    out(|fs| {
        if fs % 25 != 0 {
            return Err(format!("the speaker runs at {fs} Hz"));
        }
        let mut clip = vec![0f32; fs as usize / 5];
        clip.extend(fskv::modulate(voucher, AMP, fs).iter().map(|&s| s as f32 / 32768.0));
        clip.extend(vec![0f32; fs as usize * 3 / 10]);
        Ok(clip)
    })
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
pub fn listen(_: &UserIntent, stop: &AtomicBool) -> Result<Option<[u8; fskv::PAYLOAD]>, String> {
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
    let (mut rs, mut rx, mut pcm, mut peak) = (fskv::Resampler::new(fs), fskv::Rx::default(), Vec::new(), 0i32);
    let until = Instant::now() + LISTEN;
    while Instant::now() < until {
        if stop.load(Ordering::Relaxed) {
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(20));
        let got = std::mem::take(&mut *heard.lock().map_err(|_| "microphone poisoned")?);
        pcm.clear();
        rs.push(&got, &mut pcm);
        peak = pcm.iter().fold(peak, |p, &s| p.max((s as i32).abs()));
        if let Some(v) = rx.push(&pcm) {
            return Ok(Some(v));
        }
    }
    Err(format!(
        "heard no voucher in {} s (mic peak {:.0} dBFS)",
        LISTEN.as_secs(),
        20.0 * (peak.max(1) as f64 / 32768.0).log10()
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
