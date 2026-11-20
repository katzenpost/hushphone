//! OSS, FreeBSD's sound system: `/dev/dsp`, which is the device `hw.snd.default_unit` names.

use std::ffi::{c_int, c_ulong};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;

use upstream::{BackendSpecificError, StreamConfig};

pub const NAME: &str = "OSS /dev/dsp";

// sys/soundcard.h: _IOWR('P', n, int), but GETBLKSIZE is _IOR.
const SNDCTL_DSP_SPEED: c_ulong = 0xc004_5002;
const SNDCTL_DSP_GETBLKSIZE: c_ulong = 0x4004_5004;
const SNDCTL_DSP_SETFMT: c_ulong = 0xc004_5005;
const SNDCTL_DSP_CHANNELS: c_ulong = 0xc004_5006;
const AFMT_S16_NE: c_int = if cfg!(target_endian = "little") { 0x10 } else { 0x20 };

unsafe extern "C" {
    fn ioctl(fd: c_int, request: c_ulong, ...) -> c_int;
}

/// `/dev/dsp`, open to play or record; closed, after playing what it holds, when dropped.
pub struct Hdl(File);

impl Hdl {
    /// OSS starts on the first read or write.
    pub fn start(&self) -> bool {
        true
    }

    /// Blocks until `b` is full.
    pub fn read(&self, b: &mut [u8]) -> bool {
        (&self.0).read_exact(b).is_ok()
    }

    /// Blocks until all of `b` is taken.
    pub fn write(&self, b: &[u8]) -> bool {
        (&self.0).write_all(b).is_ok()
    }
}

/// `/dev/dsp` opened to play or record native-endian 16-bit samples, at `want`'s rate and
/// channels, or else at 48 kHz in stereo, which the kernel converts to and from the card's; with
/// the rate, channels and frames per block it agreed to.
pub fn open(play: bool, want: Option<&StreamConfig>) -> Result<(Hdl, u32, u16, usize), BackendSpecificError> {
    let err = |what: String| BackendSpecificError { description: format!("OSS: {what}") };
    let f = OpenOptions::new().read(!play).write(play).open("/dev/dsp").map_err(|e| err(format!("/dev/dsp: {e}")))?;
    let set = |request, mut v: c_int| match unsafe { ioctl(f.as_raw_fd(), request, &mut v) } {
        0 => Ok(v),
        _ => Err(err(format!("ioctl {request:#x}: {}", std::io::Error::last_os_error()))),
    };
    if set(SNDCTL_DSP_SETFMT, AFMT_S16_NE)? != AFMT_S16_NE {
        return Err(err("the device takes no 16-bit samples".into()));
    }
    let chans = set(SNDCTL_DSP_CHANNELS, want.map_or(2, |c| c.channels.into()))?;
    let rate = set(SNDCTL_DSP_SPEED, want.map_or(48000, |c| c.sample_rate as c_int))?;
    if want.is_some_and(|c| (c.sample_rate as c_int, c_int::from(c.channels)) != (rate, chans)) {
        return Err(err(format!("the device runs at {rate} Hz with {chans} channels")));
    }
    let block = set(SNDCTL_DSP_GETBLKSIZE, 0)? as usize / (2 * chans as usize);
    Ok((Hdl(f), rate as u32, chans as u16, block.max(1)))
}
