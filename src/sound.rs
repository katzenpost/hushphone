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
    let dev = cpal::default_host().default_output_device().ok_or("no speaker")?;
    let cfg = dev.default_output_config().map_err(|e| e.to_string())?;
    let (fs, format) = (cfg.sample_rate(), cfg.sample_format());
    if fs % 25 != 0 {
        return Err(format!("the speaker runs at {fs} Hz"));
    }
    let mut clip = vec![0f32; fs as usize / 5];
    clip.extend(fskv::modulate(voucher, AMP, fs).iter().map(|&s| s as f32 / 32768.0));
    clip.extend(vec![0f32; fs as usize * 3 / 10]);
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
