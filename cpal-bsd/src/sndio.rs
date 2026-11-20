//! sndio, OpenBSD's sound system: its `default` device (`AUDIODEVICE`, else sndiod).

use std::ffi::{c_char, c_int, c_uint, c_void};
use std::mem::MaybeUninit;

use upstream::{BackendSpecificError, StreamConfig};

pub const NAME: &str = "sndio default device";

/// sndio.h's `struct sio_par`.
#[repr(C)]
struct Par {
    bits: c_uint,
    bps: c_uint,
    sig: c_uint,
    le: c_uint,
    msb: c_uint,
    rchan: c_uint,
    pchan: c_uint,
    rate: c_uint,
    bufsz: c_uint,
    xrun: c_uint,
    round: c_uint,
    appbufsz: c_uint,
    pad: [c_int; 3],
    magic: c_uint,
}

#[link(name = "sndio")]
unsafe extern "C" {
    fn sio_open(name: *const c_char, mode: c_uint, nbio: c_int) -> *mut c_void;
    fn sio_close(hdl: *mut c_void);
    fn sio_initpar(par: *mut Par);
    fn sio_setpar(hdl: *mut c_void, par: *mut Par) -> c_int;
    fn sio_getpar(hdl: *mut c_void, par: *mut Par) -> c_int;
    fn sio_start(hdl: *mut c_void) -> c_int;
    fn sio_read(hdl: *mut c_void, buf: *mut c_void, nbytes: usize) -> usize;
    fn sio_write(hdl: *mut c_void, buf: *const c_void, nbytes: usize) -> usize;
}

const PLAY: c_uint = 1;
const REC: c_uint = 2;
const LE: c_uint = cfg!(target_endian = "little") as c_uint;

/// An open sndio stream, closed (after playing what it holds) when dropped.
pub struct Hdl(*mut c_void);

// SAFETY: only one thread at a time uses a handle: it moves to its stream's thread.
unsafe impl Send for Hdl {}

impl Drop for Hdl {
    fn drop(&mut self) {
        unsafe { sio_close(self.0) }
    }
}

impl Hdl {
    pub fn start(&self) -> bool {
        unsafe { sio_start(self.0) != 0 }
    }

    /// Blocks until `b` is full.
    pub fn read(&self, b: &mut [u8]) -> bool {
        unsafe { sio_read(self.0, b.as_mut_ptr().cast(), b.len()) == b.len() }
    }

    /// Blocks until all of `b` is taken.
    pub fn write(&self, b: &[u8]) -> bool {
        unsafe { sio_write(self.0, b.as_ptr().cast(), b.len()) == b.len() }
    }
}

/// The default device opened to play or record native-endian 16-bit samples, at `want`'s rate
/// and channels, or else at sndio's own; with the rate, channels and frames per block it agreed
/// to.
pub fn open(play: bool, want: Option<&StreamConfig>) -> Result<(Hdl, u32, u16, usize), BackendSpecificError> {
    let err = |what: String| BackendSpecificError { description: format!("sndio: {what}") };
    let hdl = unsafe { sio_open(c"default".as_ptr(), if play { PLAY } else { REC }, 0) };
    if hdl.is_null() {
        return Err(err("cannot open the default device".into()));
    }
    let hdl = Hdl(hdl);
    let mut par = unsafe {
        let mut p = MaybeUninit::<Par>::uninit();
        sio_initpar(p.as_mut_ptr());
        p.assume_init()
    };
    (par.bits, par.sig, par.le) = (16, 1, LE);
    if let Some(c) = want {
        par.rate = c.sample_rate;
        *if play { &mut par.pchan } else { &mut par.rchan } = c.channels.into();
    }
    if unsafe { sio_setpar(hdl.0, &mut par) == 0 || sio_getpar(hdl.0, &mut par) == 0 } {
        return Err(err("cannot set the device's parameters".into()));
    }
    if (par.bits, par.bps, par.sig, par.le) != (16, 2, 1, LE) {
        return Err(err("the device takes no 16-bit samples".into()));
    }
    let chans = if play { par.pchan } else { par.rchan };
    if want.is_some_and(|c| (c.sample_rate, u32::from(c.channels)) != (par.rate, chans)) {
        return Err(err(format!("the device runs at {} Hz with {chans} channels", par.rate)));
    }
    Ok((hdl, par.rate, chans as u16, par.round as usize))
}
