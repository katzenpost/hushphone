//! Reading a QR code out of an image: a screenshot, a picture, a camera's frame.

/// The text of the first QR code in a `w` by `h` greyscale image, whose pixels `luma` gives;
/// light on dark codes are read too.
pub fn decode(w: usize, h: usize, luma: impl Fn(usize, usize) -> u8) -> Option<String> {
    let read = |invert: u8| {
        let mut img = rqrr::PreparedImage::prepare_from_greyscale(w, h, |x, y| luma(x, y) ^ invert);
        img.detect_grids().into_iter().find_map(|g| g.decode().ok().map(|(_, text)| text))
    };
    read(0).or_else(|| read(0xff))
}

/// The text of the first QR code in an RGBA image, `w` by `h`, seen against white.
pub fn decode_rgba(w: usize, h: usize, rgba: &[u8]) -> Option<String> {
    if rgba.len() < w * h * 4 {
        return None;
    }
    decode(w, h, |x, y| {
        let p = &rgba[(y * w + x) * 4..][..4];
        let l = (p[0] as u32 * 299 + p[1] as u32 * 587 + p[2] as u32 * 114) / 1000;
        (l * p[3] as u32 / 255 + 255 - p[3] as u32) as u8
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode_png(name: &str) -> Option<String> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data").join(name);
        let img = slint::Image::load_from_path(&path).unwrap().to_rgba8().unwrap();
        decode_rgba(img.width() as usize, img.height() as usize, img.as_bytes())
    }

    #[test]
    fn reads_the_t_deck_join_screen() {
        // pigeonhole_reticulous's lcd-sim, dark mode: esp-idf/test/lcd-sim/screens/07-join-voucher.png
        assert_eq!(decode_png("t-deck-voucher.png").as_deref(), Some("cvV8Q1uGC31DYm4pNGXfCg8ihJA1Pj7sYkyK/eBIP28="));
    }

    #[test]
    fn reads_a_hushphone_voucher_card() {
        // A minted voucher's page, dark mode, as Linux shows it.
        assert_eq!(decode_png("hushphone-voucher.png").as_deref(), Some(HUSHPHONE_CODE));
    }

    #[test]
    fn reads_light_on_dark_and_nothing_from_noise() {
        let n = 21 + 8;
        let qr = qrcodegen::QrCode::encode_text("hello", qrcodegen::QrCodeEcc::Medium).unwrap();
        let dark = |x: usize, y: usize| qr.get_module(x as i32 / 4 - 4, y as i32 / 4 - 4);
        assert_eq!(decode(n * 4, n * 4, |x, y| if dark(x, y) { 255 } else { 0 }).as_deref(), Some("hello"));
        assert_eq!(decode(64, 64, |x, y| ((x * 7919 + y * 104729) % 251) as u8), None);
        assert_eq!(decode_rgba(10, 10, &[0; 4]), None);
    }

    /// The code on the voucher card in hushphone-voucher.png.
    const HUSHPHONE_CODE: &str = "rYftB9lL3klTpijbh2cDQf148H1y6jsCcGTsBVG/CJo=";
}
