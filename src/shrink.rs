//! Images the user sends, made small for the network: a 286 KB screenshot is about 190 boxes on
//! the mixnet and 1100 on the mesh. A picture is turned the way its EXIF says, scaled down,
//! and encoded again as a JPEG, which carries none of the original's metadata: no GPS position,
//! camera model or time.

use std::io::Cursor;

use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngDecoder;
use image::codecs::webp::WebPDecoder;
use image::imageops::FilterType;
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageResult, Limits};
use zeroize::Zeroizing;

use crate::effects::Picked;

/// Smaller files are sent as they are.
pub const SMALL: usize = 20 * 1024;
/// The longest side of a picture sent.
pub const SIDE: u32 = 1600;
pub const QUALITY: u8 = 80;

/// The picture as a smaller JPEG without metadata; `None` for a file that is not a still
/// picture this can read, one already small, or one that would not get smaller.
pub fn shrink(p: &Picked) -> Option<Picked> {
    if p.bytes.len() < SMALL {
        return None;
    }
    let img = decode(&p.bytes).ok()?;
    let img = if img.width().max(img.height()) > SIDE { img.resize(SIDE, SIDE, FilterType::Lanczos3) } else { img };
    // JPEG has no transparency: what is see-through shows white, as on a light screen.
    let mut rgba = img.into_rgba8();
    for px in rgba.pixels_mut() {
        let a = px[3] as u32;
        for c in &mut px.0[..3] {
            *c = ((*c as u32 * a + 255 * (255 - a) + 127) / 255) as u8;
        }
    }
    let rgb = DynamicImage::ImageRgba8(rgba).into_rgb8();
    let mut out = Zeroizing::new(Vec::new());
    JpegEncoder::new_with_quality(&mut *out, QUALITY).encode_image(&rgb).ok()?;
    if out.len() >= p.bytes.len() {
        return None;
    }
    let stem = std::path::Path::new(&p.name).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    Some(Picked { name: format!("{}.jpg", if stem.is_empty() { "image" } else { &stem }), bytes: out })
}

/// A still picture, turned upright; animations are refused, as one frame would be all that's sent.
fn decode(bytes: &[u8]) -> ImageResult<DynamicImage> {
    let c = Cursor::new(bytes);
    let unsupported = || image::ImageError::Unsupported(image::error::UnsupportedError::from(image::error::ImageFormatHint::Unknown));
    match image::guess_format(bytes)? {
        ImageFormat::Png => {
            let d = PngDecoder::new(c)?;
            if d.is_apng()? {
                return Err(unsupported());
            }
            upright(d)
        }
        ImageFormat::WebP => {
            let d = WebPDecoder::new(c)?;
            if d.has_animation() {
                return Err(unsupported());
            }
            upright(d)
        }
        ImageFormat::Jpeg => upright(image::codecs::jpeg::JpegDecoder::new(c)?),
        _ => Err(unsupported()),
    }
}

fn upright(mut d: impl ImageDecoder) -> ImageResult<DynamicImage> {
    // A phone's picture fits easily; a crafted one claiming to be huge is refused before it's read.
    let mut limits = Limits::default();
    limits.max_image_width = Some(16384);
    limits.max_image_height = Some(16384);
    limits.max_alloc = Some(256 * 1024 * 1024);
    d.set_limits(limits)?;
    let o = d.orientation()?;
    let mut img = DynamicImage::from_decoder(d)?;
    img.apply_orientation(o);
    Ok(img)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageEncoder, Rgb, RgbImage, Rgba, RgbaImage};

    fn picked(name: &str, bytes: Vec<u8>) -> Picked {
        Picked { name: name.into(), bytes: Zeroizing::new(bytes) }
    }

    /// Noise that compresses poorly, from a fixed seed.
    fn noise(w: u32, h: u32) -> RgbImage {
        let mut s = 1u32;
        RgbImage::from_fn(w, h, |_, _| {
            s = s.wrapping_mul(1664525).wrapping_add(1013904223);
            Rgb([(s >> 24) as u8, (s >> 16) as u8, (s >> 8) as u8])
        })
    }

    fn png(img: &RgbaImage) -> Vec<u8> {
        let mut v = Vec::new();
        image::codecs::png::PngEncoder::new(&mut v).write_image(img, img.width(), img.height(), image::ExtendedColorType::Rgba8).unwrap();
        v
    }

    fn jpeg_size(bytes: &[u8]) -> (u32, u32) {
        let d = image::codecs::jpeg::JpegDecoder::new(Cursor::new(bytes)).unwrap();
        d.dimensions()
    }

    #[test]
    fn a_screenshot_shrinks_to_a_smaller_jpeg() {
        // hushphone's Android screenshot, at the 1080x2400 a phone's screen takes.
        let shot = image::open(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/android.png")).unwrap();
        let shot = shot.resize_exact(1080, 2400, FilterType::CatmullRom).into_rgba8();
        let p = picked("Screenshot_20261112.png", png(&shot));
        let small = shrink(&p).expect("shrunk");
        assert_eq!(small.name, "Screenshot_20261112.jpg");
        assert_eq!(infer::get(&small.bytes).unwrap().mime_type(), "image/jpeg");
        assert_eq!(jpeg_size(&small.bytes), (720, 1600));
        eprintln!("screenshot: {} -> {} bytes", p.bytes.len(), small.bytes.len());
        assert!(p.bytes.len() > 300_000 && small.bytes.len() < 100_000);
        // At its own size it shrinks too, and keeps its size.
        let p = picked("ping.png", std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/ping/ping.png")).unwrap());
        let small = shrink(&p).expect("shrunk");
        eprintln!("screenshot: {} -> {} bytes", p.bytes.len(), small.bytes.len());
        assert_eq!(jpeg_size(&small.bytes), (1100, 720));
        assert!(small.bytes.len() < p.bytes.len() * 3 / 4);
    }

    /// A JPEG with an APP1 EXIF segment: orientation 6 (turn right) and a GPS latitude.
    fn jpeg_with_exif(img: &RgbImage) -> Vec<u8> {
        let mut plain = Vec::new();
        JpegEncoder::new_with_quality(&mut plain, 100).encode_image(img).unwrap();
        let mut tiff: Vec<u8> = b"II*\0\x08\0\0\0".to_vec();
        // IFD0 at 8: Orientation = 6, GPS IFD at 38.
        tiff.extend([2, 0]);
        tiff.extend([0x12, 0x01, 3, 0, 1, 0, 0, 0, 6, 0, 0, 0]);
        tiff.extend([0x25, 0x88, 4, 0, 1, 0, 0, 0, 38, 0, 0, 0]);
        tiff.extend([0, 0, 0, 0]);
        // GPS IFD at 38: GPSLatitudeRef = "N".
        tiff.extend([1, 0]);
        tiff.extend([0x01, 0x00, 2, 0, 2, 0, 0, 0, b'N', 0, 0, 0]);
        tiff.extend([0, 0, 0, 0]);
        let mut app1 = b"Exif\0\0".to_vec();
        app1.extend(tiff);
        let mut out = plain[..2].to_vec();
        out.extend([0xFF, 0xE1]);
        out.extend(((app1.len() + 2) as u16).to_be_bytes());
        out.extend(app1);
        out.extend(&plain[2..]);
        out
    }

    #[test]
    fn exif_is_applied_then_dropped() {
        let p = picked("photo.jpeg", jpeg_with_exif(&noise(400, 200)));
        let mut d = image::codecs::jpeg::JpegDecoder::new(Cursor::new(&p.bytes[..])).unwrap();
        assert!(d.exif_metadata().unwrap().is_some(), "the test's EXIF is read");
        let small = shrink(&p).expect("shrunk");
        assert_eq!(small.name, "photo.jpg");
        // Turned right: the wide picture stands upright.
        assert_eq!(jpeg_size(&small.bytes), (200, 400));
        let mut d = image::codecs::jpeg::JpegDecoder::new(Cursor::new(&small.bytes[..])).unwrap();
        assert_eq!(d.exif_metadata().unwrap(), None);
        assert!(!small.bytes.windows(4).any(|w| w == b"Exif"));
        assert!(!small.bytes.windows(2).any(|w| w == [0xFF, 0xE1]), "no APP1 segment");
    }

    #[test]
    fn transparency_shows_white() {
        let mut img = RgbaImage::from_pixel(300, 300, Rgba([0, 0, 0, 0]));
        let n = noise(300, 150);
        for (x, y, px) in n.enumerate_pixels() {
            img.put_pixel(x, y + 150, Rgba([px[0], px[1], px[2], 255]));
        }
        let small = shrink(&picked("a.png", png(&img))).expect("shrunk");
        let back = image::load_from_memory(&small.bytes).unwrap().into_rgb8();
        assert!(back.get_pixel(150, 20).0.iter().all(|&c| c > 245), "{:?}", back.get_pixel(150, 20));
    }

    #[test]
    fn small_files_and_other_files_are_left_alone() {
        let tiny = png(&RgbaImage::from_pixel(64, 64, Rgba([1, 2, 3, 255])));
        assert!(tiny.len() < SMALL);
        assert!(shrink(&picked("tiny.png", tiny)).is_none());
        let mut pdf = b"%PDF-1.7\n".to_vec();
        pdf.resize(100_000, b'x');
        assert!(shrink(&picked("a.pdf", pdf)).is_none());
        assert!(shrink(&picked("junk.png", vec![0x89; 50_000])).is_none());
    }

    #[test]
    fn a_picture_that_would_not_shrink_is_left_alone() {
        // Already a small JPEG, below the side limit: encoding it again gains nothing.
        let mut v = Vec::new();
        JpegEncoder::new_with_quality(&mut v, 40).encode_image(&noise(600, 400)).unwrap();
        assert!(v.len() > SMALL);
        assert!(shrink(&picked("q40.jpg", v)).is_none());
    }
}
