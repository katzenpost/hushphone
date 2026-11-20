//! A cpal host over a sound system whose streams block (`io`: sndio or OSS): one device, the
//! system's default, at the rate and channels the system agrees to, each stream moving one block
//! at a time on a thread of its own. Samples are 16-bit to the system and f32 to cpal, since
//! rustic_audio plays only f32.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use upstream::traits::{DeviceTrait, HostTrait, StreamTrait};
use upstream::{
    BackendSpecificError, BuildStreamError, Data, DefaultStreamConfigError, DeviceDescription, DeviceDescriptionBuilder,
    DeviceId, DeviceIdError, DeviceNameError, DevicesError, HostId, InputCallbackInfo, InputStreamTimestamp,
    OutputCallbackInfo, OutputStreamTimestamp, PauseStreamError, PlayStreamError, Sample, SampleFormat, StreamConfig,
    StreamError, StreamInstant, SupportedBufferSize, SupportedStreamConfig, SupportedStreamConfigRange,
    SupportedStreamConfigsError,
};

use crate::io::{Hdl, NAME, open};

fn default_config(play: bool) -> Result<SupportedStreamConfig, BackendSpecificError> {
    let (_, rate, chans, _) = open(play, None)?;
    Ok(SupportedStreamConfig::new(chans, rate, SupportedBufferSize::Unknown, SampleFormat::F32))
}

fn configs(play: bool) -> Result<std::vec::IntoIter<SupportedStreamConfigRange>, BackendSpecificError> {
    let c = default_config(play)?;
    let range = SupportedStreamConfigRange::new(c.channels(), c.sample_rate(), c.sample_rate(), SupportedBufferSize::Unknown, SampleFormat::F32);
    Ok(vec![range].into_iter())
}

pub struct Host;

impl HostTrait for Host {
    type Devices = std::iter::Once<Device>;
    type Device = Device;

    fn is_available() -> bool {
        true
    }

    fn devices(&self) -> Result<Self::Devices, DevicesError> {
        Ok(std::iter::once(Device))
    }

    fn default_input_device(&self) -> Option<Device> {
        Some(Device)
    }

    fn default_output_device(&self) -> Option<Device> {
        Some(Device)
    }
}

#[derive(Clone)]
pub struct Device;

impl DeviceTrait for Device {
    type SupportedInputConfigs = std::vec::IntoIter<SupportedStreamConfigRange>;
    type SupportedOutputConfigs = std::vec::IntoIter<SupportedStreamConfigRange>;
    type Stream = Stream;

    fn description(&self) -> Result<DeviceDescription, DeviceNameError> {
        Ok(DeviceDescriptionBuilder::new(NAME).build())
    }

    fn id(&self) -> Result<DeviceId, DeviceIdError> {
        Ok(DeviceId(HostId::Custom, "default".into()))
    }

    fn supported_input_configs(&self) -> Result<Self::SupportedInputConfigs, SupportedStreamConfigsError> {
        Ok(configs(false)?)
    }

    fn supported_output_configs(&self) -> Result<Self::SupportedOutputConfigs, SupportedStreamConfigsError> {
        Ok(configs(true)?)
    }

    fn default_input_config(&self) -> Result<SupportedStreamConfig, DefaultStreamConfigError> {
        Ok(default_config(false)?)
    }

    fn default_output_config(&self) -> Result<SupportedStreamConfig, DefaultStreamConfigError> {
        Ok(default_config(true)?)
    }

    // The config's buffer size is not asked for: the system's own is used.
    fn build_input_stream_raw<D, E>(
        &self,
        config: &StreamConfig,
        format: SampleFormat,
        mut data_callback: D,
        error_callback: E,
        _: Option<Duration>,
    ) -> Result<Stream, BuildStreamError>
    where
        D: FnMut(&Data, &InputCallbackInfo) + Send + 'static,
        E: FnMut(StreamError) + Send + 'static,
    {
        if format != SampleFormat::F32 {
            return Err(BuildStreamError::StreamConfigNotSupported);
        }
        let (hdl, _, chans, block) = open(false, Some(config))?;
        let n = block * chans as usize;
        let (mut s, mut f) = (vec![0u8; 2 * n], vec![0f32; n]);
        Ok(Stream::run(hdl, error_callback, move |hdl, now| {
            if !hdl.read(&mut s) {
                return false;
            }
            f.iter_mut().zip(s.as_chunks::<2>().0).for_each(|(f, &s)| *f = i16::from_ne_bytes(s).to_sample());
            let data = unsafe { Data::from_parts(f.as_mut_ptr().cast(), n, SampleFormat::F32) };
            data_callback(&data, &InputCallbackInfo::new(InputStreamTimestamp { callback: now, capture: now }));
            true
        }))
    }

    fn build_output_stream_raw<D, E>(
        &self,
        config: &StreamConfig,
        format: SampleFormat,
        mut data_callback: D,
        error_callback: E,
        _: Option<Duration>,
    ) -> Result<Stream, BuildStreamError>
    where
        D: FnMut(&mut Data, &OutputCallbackInfo) + Send + 'static,
        E: FnMut(StreamError) + Send + 'static,
    {
        if format != SampleFormat::F32 {
            return Err(BuildStreamError::StreamConfigNotSupported);
        }
        let (hdl, _, chans, block) = open(true, Some(config))?;
        let n = block * chans as usize;
        let (mut f, mut s) = (vec![0f32; n], vec![0u8; 2 * n]);
        Ok(Stream::run(hdl, error_callback, move |hdl, now| {
            let mut data = unsafe { Data::from_parts(f.as_mut_ptr().cast(), n, SampleFormat::F32) };
            data_callback(&mut data, &OutputCallbackInfo::new(OutputStreamTimestamp { callback: now, playback: now }));
            s.as_chunks_mut::<2>().0.iter_mut().zip(&f).for_each(|(s, f)| *s = f.to_sample::<i16>().to_ne_bytes());
            hdl.write(&s)
        }))
    }
}

pub struct Stream {
    playing: Arc<AtomicBool>,
    exit: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Stream {
    /// Moves one block between `hdl` and the app with `block` for as long as the stream plays,
    /// on a thread of its own, until the stream is dropped or `block` fails.
    fn run(
        hdl: Hdl,
        mut error_callback: impl FnMut(StreamError) + Send + 'static,
        mut block: impl FnMut(&Hdl, StreamInstant) -> bool + Send + 'static,
    ) -> Stream {
        let (playing, exit) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
        let (p, x) = (playing.clone(), exit.clone());
        let thread = std::thread::spawn(move || {
            let (t0, mut started) = (Instant::now(), false);
            while !x.load(Ordering::Relaxed) {
                // Paused, the system waits for samples to come or go again.
                if !p.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                }
                let t = t0.elapsed();
                let ok = (started || hdl.start()) && block(&hdl, StreamInstant::new(t.as_secs() as i64, t.subsec_nanos()));
                if !ok {
                    error_callback(StreamError::DeviceNotAvailable);
                    return;
                }
                started = true;
            }
        });
        Stream { playing, exit, thread: Some(thread) }
    }
}

impl StreamTrait for Stream {
    fn play(&self) -> Result<(), PlayStreamError> {
        self.playing.store(true, Ordering::Relaxed);
        Ok(())
    }

    fn pause(&self) -> Result<(), PauseStreamError> {
        self.playing.store(false, Ordering::Relaxed);
        Ok(())
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        self.exit.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
