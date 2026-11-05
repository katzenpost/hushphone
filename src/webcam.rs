//! Linux: a webcam's frames, through V4L2's memory-mapped streaming, for reading a QR code off
//! another screen. Only the frames' brightness is kept, and only while scanning.

use std::fs::File;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::Duration;

const fn ioc(dir: u64, nr: u64, size: usize) -> u64 {
    (dir << 30) | ((size as u64) << 16) | ((b'V' as u64) << 8) | nr
}
const QUERYCAP: u64 = ioc(2, 0, size_of::<Capability>());
const S_FMT: u64 = ioc(3, 5, size_of::<Format>());
const REQBUFS: u64 = ioc(3, 8, size_of::<RequestBuffers>());
const QUERYBUF: u64 = ioc(3, 9, size_of::<Buffer>());
const QBUF: u64 = ioc(3, 15, size_of::<Buffer>());
const DQBUF: u64 = ioc(3, 17, size_of::<Buffer>());
const STREAMON: u64 = ioc(1, 18, size_of::<u32>());
const STREAMOFF: u64 = ioc(1, 19, size_of::<u32>());

const CAPTURE: u32 = 1;
const DEVICE_CAPS: u32 = 0x8000_0000;
const STREAMING: u32 = 0x0400_0000;
const MMAP: u32 = 1;

const fn fourcc(s: &[u8; 4]) -> u32 {
    u32::from_le_bytes(*s)
}
const YUYV: u32 = fourcc(b"YUYV");
/// Formats whose first plane is the brightness, one byte a pixel.
const LUMA_FIRST: [u32; 4] = [fourcc(b"GREY"), fourcc(b"YU12"), fourcc(b"NV12"), fourcc(b"NV21")];

// The kernel's structures (linux/videodev2.h), for a 64-bit target.
#[repr(C)]
struct Capability {
    driver: [u8; 16],
    card: [u8; 32],
    bus_info: [u8; 32],
    version: u32,
    capabilities: u32,
    device_caps: u32,
    reserved: [u32; 3],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct PixFormat {
    width: u32,
    height: u32,
    pixelformat: u32,
    field: u32,
    bytesperline: u32,
    sizeimage: u32,
    rest: [u32; 6],
}

#[repr(C)]
union FormatUnion {
    pix: PixFormat,
    raw: [u64; 25],
}

#[repr(C)]
struct Format {
    kind: u32,
    fmt: FormatUnion,
}

#[repr(C)]
struct RequestBuffers {
    count: u32,
    kind: u32,
    memory: u32,
    capabilities: u32,
    flags: u8,
    reserved: [u8; 3],
}

#[repr(C)]
struct Buffer {
    index: u32,
    kind: u32,
    bytesused: u32,
    flags: u32,
    field: u32,
    timestamp: libc::timeval,
    timecode: [u32; 4],
    sequence: u32,
    memory: u32,
    offset: u64,
    length: u32,
    reserved2: u32,
    request_fd: u32,
}

const _: () = assert!(size_of::<Capability>() == 104 && size_of::<Format>() == 208);
const _: () = assert!(size_of::<RequestBuffers>() == 20 && size_of::<Buffer>() == 88);

fn zeroed<T>() -> T {
    // SAFETY: only for the plain structures above, of integers and arrays, for which zero is valid.
    unsafe { std::mem::zeroed() }
}

fn ioctl<T>(f: &File, request: u64, arg: &mut T) -> std::io::Result<()> {
    loop {
        // SAFETY: `arg` is the structure `request` names, of the size encoded in it.
        if unsafe { libc::ioctl(f.as_raw_fd(), request as _, arg as *mut T) } == 0 {
            return Ok(());
        }
        let e = std::io::Error::last_os_error();
        if e.kind() != std::io::ErrorKind::Interrupted {
            return Err(e);
        }
    }
}

fn capture_device(f: &File) -> bool {
    let mut cap: Capability = zeroed();
    if ioctl(f, QUERYCAP, &mut cap).is_err() {
        return false;
    }
    let caps = if cap.capabilities & DEVICE_CAPS != 0 { cap.device_caps } else { cap.capabilities };
    caps & CAPTURE != 0 && caps & STREAMING != 0
}

/// The first webcam there is: `/dev/video0`, `/dev/video1`… that captures video.
pub fn find() -> Option<PathBuf> {
    (0..64)
        .map(|n| PathBuf::from(format!("/dev/video{n}")))
        .find(|p| File::options().read(true).write(true).open(p).is_ok_and(|f| capture_device(&f)))
}

/// A webcam streaming frames into buffers it shares with us.
pub struct Camera {
    file: File,
    maps: Vec<(*mut libc::c_void, usize)>,
    pub width: usize,
    pub height: usize,
    stride: usize,
    format: u32,
}

impl Camera {
    pub fn open(path: &Path) -> Result<Camera, String> {
        let file = File::options().read(true).write(true).open(path).map_err(|e| format!("can't open {}: {e}", path.display()))?;
        if !capture_device(&file) {
            return Err(format!("{} is not a webcam", path.display()));
        }
        let mut fmt: Format = zeroed();
        fmt.kind = CAPTURE;
        fmt.fmt.pix = PixFormat { width: 1280, height: 720, pixelformat: YUYV, field: 0, bytesperline: 0, sizeimage: 0, rest: [0; 6] };
        ioctl(&file, S_FMT, &mut fmt).map_err(|e| format!("{} refuses a format: {e}", path.display()))?;
        // SAFETY: the driver filled in the pix member for a capture format.
        let pix = unsafe { fmt.fmt.pix };
        if pix.pixelformat != YUYV && !LUMA_FIRST.contains(&pix.pixelformat) {
            let name = String::from_utf8_lossy(&pix.pixelformat.to_le_bytes()).into_owned();
            return Err(format!("{} sends {name} frames, which hushphone can't read", path.display()));
        }
        let mut cam = Camera {
            file,
            maps: Vec::new(),
            width: pix.width as usize,
            height: pix.height as usize,
            stride: pix.bytesperline as usize,
            format: pix.pixelformat,
        };
        let mut req = RequestBuffers { count: 4, kind: CAPTURE, memory: MMAP, capabilities: 0, flags: 0, reserved: [0; 3] };
        ioctl(&cam.file, REQBUFS, &mut req).map_err(|e| e.to_string())?;
        for index in 0..req.count {
            let mut b = cam.buffer(index);
            ioctl(&cam.file, QUERYBUF, &mut b).map_err(|e| e.to_string())?;
            // SAFETY: maps the buffer the driver just described, at its offset and length.
            let p = unsafe {
                libc::mmap(std::ptr::null_mut(), b.length as usize, libc::PROT_READ, libc::MAP_SHARED, cam.file.as_raw_fd(), b.offset as libc::off_t)
            };
            if p == libc::MAP_FAILED {
                return Err(std::io::Error::last_os_error().to_string());
            }
            cam.maps.push((p, b.length as usize));
            ioctl(&cam.file, QBUF, &mut b).map_err(|e| e.to_string())?;
        }
        let mut kind = CAPTURE;
        ioctl(&cam.file, STREAMON, &mut kind).map_err(|e| e.to_string())?;
        Ok(cam)
    }

    fn buffer(&self, index: u32) -> Buffer {
        let mut b: Buffer = zeroed();
        b.index = index;
        b.kind = CAPTURE;
        b.memory = MMAP;
        b
    }

    /// The next frame's brightness, one byte a pixel, row after row; none if no frame came in
    /// `wait`.
    pub fn frame(&mut self, wait: Duration) -> Result<Option<Vec<u8>>, String> {
        let mut pfd = libc::pollfd { fd: self.file.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        // SAFETY: one pollfd, ours.
        match unsafe { libc::poll(&mut pfd, 1, wait.as_millis() as libc::c_int) } {
            0 => return Ok(None),
            n if n < 0 => return Err(std::io::Error::last_os_error().to_string()),
            _ => {}
        }
        let mut b = self.buffer(0);
        ioctl(&self.file, DQBUF, &mut b).map_err(|e| e.to_string())?;
        let (p, len) = *self.maps.get(b.index as usize).ok_or("the webcam named a buffer it never had")?;
        // SAFETY: the mapping of this buffer, of `len` bytes; the driver does not write it until it
        // is queued again, below.
        let data = unsafe { std::slice::from_raw_parts(p as *const u8, len.min(b.bytesused as usize)) };
        let luma = luma(self.format, data, self.width, self.height, self.stride);
        ioctl(&self.file, QBUF, &mut b).map_err(|e| e.to_string())?;
        Ok(luma)
    }
}

impl Drop for Camera {
    fn drop(&mut self) {
        let mut kind = CAPTURE;
        let _ = ioctl(&self.file, STREAMOFF, &mut kind);
        for &(p, len) in &self.maps {
            // SAFETY: a mapping made in `open`, unmapped once.
            unsafe { libc::munmap(p, len) };
        }
    }
}

/// The brightness of a `w` by `h` frame in `format`, rows `stride` bytes apart; none if the
/// frame is cut short.
fn luma(format: u32, data: &[u8], w: usize, h: usize, stride: usize) -> Option<Vec<u8>> {
    let step = if format == YUYV { 2 } else { 1 };
    let stride = stride.max(w * step);
    if h == 0 || data.len() < stride * (h - 1) + w * step {
        return None;
    }
    Some((0..h).flat_map(|y| data[y * stride..][..w * step].iter().step_by(step).copied()).collect())
}

/// A `w` by `h` greyscale frame as a preview at most `max` pixels wide.
pub fn preview(luma: &[u8], w: usize, h: usize, max: usize) -> slint::SharedPixelBuffer<slint::Rgb8Pixel> {
    let k = w.div_ceil(max).max(1);
    let (pw, ph) = (w / k, h / k);
    let mut buf = slint::SharedPixelBuffer::<slint::Rgb8Pixel>::new(pw as u32, ph as u32);
    for (i, p) in buf.make_mut_slice().iter_mut().enumerate() {
        let v = luma[(i / pw) * k * w + (i % pw) * k];
        *p = slint::Rgb8Pixel::new(v, v, v);
    }
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brightness_from_yuyv_and_planar_frames() {
        // Two rows of two pixels, with padding at the end of each row.
        let yuyv = [10, 128, 20, 128, 0, 0, 30, 128, 40, 128, 0, 0];
        assert_eq!(luma(YUYV, &yuyv, 2, 2, 6), Some(vec![10, 20, 30, 40]));
        let grey = [1, 2, 9, 3, 4, 9];
        assert_eq!(luma(fourcc(b"GREY"), &grey, 2, 2, 3), Some(vec![1, 2, 3, 4]));
        assert_eq!(luma(YUYV, &yuyv[..9], 2, 2, 6), None);
        assert_eq!(luma(YUYV, &yuyv, 2, 0, 6), None);
    }

    #[test]
    fn a_recorded_frame_holds_the_t_deck_code() {
        // One YUYV frame from /dev/video0 (v4l2loopback fed the T-Deck's join screen), as the
        // brightness hushphone keeps of it, 640 by 480.
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/webcam-frame.png");
        let img = slint::Image::load_from_path(&path).unwrap().to_rgba8().unwrap();
        let (w, h) = (img.width() as usize, img.height() as usize);
        let grey: Vec<u8> = img.as_slice().iter().map(|p| p.r).collect();
        assert_eq!(crate::qr::decode(w, h, |x, y| grey[y * w + x]).as_deref(), Some("cvV8Q1uGC31DYm4pNGXfCg8ihJA1Pj7sYkyK/eBIP28="));
        let p = preview(&grey, w, h, 320);
        assert_eq!((p.width(), p.height()), (320, 240));
    }
}
