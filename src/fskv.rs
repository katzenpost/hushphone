//! The voucher modem of the T-Deck chat (pigeonhole_reticulous's components/kq_fskv), ported bit
//! for bit: 32 bytes, framed [len][voucher][crc16] and RS(51,35) coded, sent as MFSK in the voice
//! band at 16 kHz: 4 groups of 16 tones at once, 62.5 Hz apart, 1000-4938 Hz, 25 symbols a second,
//! 1.2 s a frame.

use sha3::Shake256;
use sha3::digest::{ExtendableOutput, Update, XofReader};

pub const FS: u32 = 16000;
pub const PAYLOAD: usize = 32;
const FRAME: usize = 1 + PAYLOAD + 2; // len, voucher, crc16
const NSYM: usize = 16; // RS parity bytes: corrects 8 byte errors
const CW: usize = FRAME + NSYM; // 51
const SPS: usize = 640; // samples a symbol: a 32 ms window behind an 8 ms echo guard
const WIN: usize = 512;
const KS: usize = 4; // sync symbols
const SYMBOLS: usize = KS + 26;
const KD: usize = SYMBOLS - KS;
/// Samples a frame lasts at 16 kHz.
pub const SAMPLES: usize = SYMBOLS * SPS;
const TONES: usize = 64;
const GROUPS: usize = 4;
const GTONES: usize = 16;
const BPS: usize = 16; // bits a symbol: 4 a group
const PI_F: f32 = std::f32::consts::PI; // the T-Deck's 3.14159265358979f
const F0: f32 = 1000.0;
const DF: f32 = 62.5;
const GDF: f32 = 1000.0;
/// Samples after a frame's start that `decode_at` may read.
const SPAN: usize = (SAMPLES as f64 * 1.007) as usize + 2 * WIN;

// The sync word: its 16 nibbles are 0..15 once each, so every tone is trained.
const SYNC: [u8; 8] = [0x3A, 0xF1, 0x6C, 0x08, 0xD5, 0x2E, 0x94, 0xB7];

/// The 4 digits both screens show for a voucher, so that the two people can tell it arrived
/// whole: SHAKE256 of the voucher, its first 4 bytes big-endian, mod 10000.
pub fn check(voucher: &[u8]) -> String {
    let mut h = Shake256::default();
    h.update(voucher);
    let mut d = [0u8; 4];
    h.finalize_xof().read(&mut d);
    format!("{:04}", u32::from_be_bytes(d) % 10000)
}

// ---- CRC-16/CCITT-FALSE ----
pub fn crc16(p: &[u8]) -> u16 {
    let mut c: u16 = 0xFFFF;
    for &b in p {
        c ^= (b as u16) << 8;
        for _ in 0..8 {
            c = if c & 0x8000 != 0 { (c << 1) ^ 0x1021 } else { c << 1 };
        }
    }
    c
}

// ---- Reed-Solomon over GF(256), x^8+x^4+x^3+x^2+1, roots a^0..a^(NSYM-1) ----
struct Gf {
    exp: [u8; 512],
    log: [u8; 256],
    gen_: [u8; NSYM + 1], // generator, highest degree first
}

const GF: Gf = Gf::new();

impl Gf {
    const fn new() -> Gf {
        let mut g = Gf { exp: [0; 512], log: [0; 256], gen_: [0; NSYM + 1] };
        let mut x: usize = 1;
        let mut i = 0;
        while i < 255 {
            g.exp[i] = x as u8;
            g.log[x] = i as u8;
            x <<= 1;
            if x & 0x100 != 0 {
                x ^= 0x11d;
            }
            i += 1;
        }
        while i < 512 {
            g.exp[i] = g.exp[i - 255];
            i += 1;
        }
        g.gen_[0] = 1;
        let mut i = 0;
        while i < NSYM {
            // gen *= (x + a^i)
            let mut j = i + 1;
            while j > 0 {
                g.gen_[j] ^= g.mul(g.gen_[j - 1], g.exp[i]);
                j -= 1;
            }
            i += 1;
        }
        g
    }

    const fn mul(&self, a: u8, b: u8) -> u8 {
        if a != 0 && b != 0 { self.exp[self.log[a as usize] as usize + self.log[b as usize] as usize] } else { 0 }
    }

    fn div(&self, a: u8, b: u8) -> u8 {
        if a != 0 { self.exp[(self.log[a as usize] as usize + 255 - self.log[b as usize] as usize) % 255] } else { 0 }
    }
}

/// `msg` followed by its NSYM parity bytes.
pub fn rs_encode(msg: &[u8]) -> Vec<u8> {
    let mut rem = [0u8; NSYM];
    for &m in msg {
        let fb = m ^ rem[0];
        rem.copy_within(1.., 0);
        rem[NSYM - 1] = 0;
        for (r, &g) in rem.iter_mut().zip(&GF.gen_[1..]) {
            *r ^= GF.mul(g, fb);
        }
    }
    let mut cw = msg.to_vec();
    cw.extend_from_slice(&rem);
    cw
}

fn syndromes(cw: &[u8], s: &mut [u8; NSYM]) -> bool {
    let mut any = 0;
    for (i, si) in s.iter_mut().enumerate() {
        let mut v = 0;
        for &c in cw {
            v = GF.mul(v, GF.exp[i]) ^ c;
        }
        *si = v;
        any |= v;
    }
    any != 0
}

/// Corrects `cw` in place: the byte errors fixed, or `None` if it can't be.
pub fn rs_decode(cw: &mut [u8]) -> Option<usize> {
    let n = cw.len();
    let mut s = [0u8; NSYM];
    if !syndromes(cw, &mut s) {
        return Some(0);
    }
    // Berlekamp-Massey; polynomials lowest degree first. Byte cw[i] is the coefficient of x^(n-1-i).
    let (mut c, mut b_) = ([0u8; NSYM + 1], [0u8; NSYM + 1]);
    c[0] = 1;
    b_[0] = 1;
    let (mut l, mut m, mut b) = (0usize, 1usize, 1u8);
    for r in 0..NSYM {
        let mut d = s[r];
        for i in 1..=l {
            d ^= GF.mul(c[i], s[r - i]);
        }
        if d == 0 {
            m += 1;
            continue;
        }
        let coef = GF.div(d, b);
        let t = c;
        for i in 0..(NSYM + 1).saturating_sub(m) {
            c[i + m] ^= GF.mul(coef, b_[i]);
        }
        if 2 * l <= r {
            l = r + 1 - l;
            b_ = t;
            b = d;
            m = 1;
        } else {
            m += 1;
        }
    }
    if 2 * l > NSYM {
        return None;
    }
    // Omega = S * Lambda mod x^NSYM
    let mut o = [0u8; NSYM];
    for i in 0..NSYM {
        for j in 0..=i.min(l) {
            o[i] ^= GF.mul(c[j], s[i - j]);
        }
    }
    let mut found = 0;
    for p in 0..n {
        // Chien search over position p (power of x)
        let xi = GF.exp[(255 - (p % 255)) % 255]; // X^-1
        let (mut lv, mut xp) = (0u8, 1u8);
        for &ci in &c[..=l] {
            lv ^= GF.mul(ci, xp);
            xp = GF.mul(xp, xi);
        }
        if lv != 0 {
            continue;
        }
        let (mut ov, mut dv) = (0u8, 0u8);
        xp = 1;
        for &oi in &o {
            ov ^= GF.mul(oi, xp);
            xp = GF.mul(xp, xi);
        }
        xp = 1; // Lambda'(x) = sum over odd i of C[i] x^(i-1)
        for i in (1..=l).step_by(2) {
            dv ^= GF.mul(c[i], xp);
            xp = GF.mul(xp, GF.mul(xi, xi));
        }
        if dv == 0 {
            return None;
        }
        cw[n - 1 - p] ^= GF.mul(GF.exp[p % 255], GF.div(ov, dv)); // Forney, first root a^0
        found += 1;
    }
    if found != l || syndromes(cw, &mut s) {
        return None;
    }
    Some(found)
}

// ---- framing ----
fn getbit(p: &[u8], i: usize) -> usize {
    if i < p.len() * 8 { ((p[i >> 3] >> (7 - (i & 7))) & 1) as usize } else { 0 }
}

/// Tone of group g in symbol k of the frame: the sync, then the coded bytes (`cw` None: the sync
/// only).
fn tone_of(cw: Option<&[u8; CW]>, k: usize, g: usize) -> usize {
    let (src, base): (&[u8], usize) = if k < KS { (&SYNC, k * BPS + g * 4) } else { (cw.unwrap(), (k - KS) * BPS + g * 4) };
    (0..4).fold(0, |t, i| (t << 1) | getbit(src, base + i))
}

pub fn frame(v: &[u8; PAYLOAD]) -> [u8; CW] {
    let mut f = [0u8; FRAME];
    f[0] = PAYLOAD as u8;
    f[1..=PAYLOAD].copy_from_slice(v);
    let c = crc16(&f[..1 + PAYLOAD]);
    f[FRAME - 2..].copy_from_slice(&c.to_be_bytes());
    rs_encode(&f).try_into().unwrap()
}

/// The frame at peak amplitude `amp` (at most 32767), sampled at `fs` Hz, a multiple of 25. At
/// 16 kHz it is the T-Deck's, sample for sample. Continuous phase, with 2 ms ramps at either end.
pub fn modulate(v: &[u8; PAYLOAD], amp: f32, fs: u32) -> Vec<i16> {
    let cw = frame(v);
    let mut ph = [0f32; GROUPS];
    let (sps, ramp) = (fs as usize / 25, fs as usize / 500);
    let n_all = SYMBOLS * sps;
    let mut out = Vec::with_capacity(n_all);
    for k in 0..SYMBOLS {
        let mut w = [0f32; GROUPS];
        for (g, wg) in w.iter_mut().enumerate() {
            *wg = 2.0 * PI_F * (F0 + g as f32 * GDF + tone_of(Some(&cw), k, g) as f32 * DF) / fs as f32;
        }
        for n in k * sps..(k + 1) * sps {
            let mut y = 0f32;
            for g in 0..GROUPS {
                ph[g] += w[g];
                if ph[g] > 2.0 * PI_F {
                    ph[g] -= 2.0 * PI_F;
                }
                y += ph[g].sin();
            }
            let mut r = 1f32;
            if n < ramp {
                r = 0.5 - 0.5 * (PI_F * n as f32 / ramp as f32).cos();
            }
            if n_all - n < ramp {
                r = 0.5 - 0.5 * (PI_F * (n_all - n) as f32 / ramp as f32).cos();
            }
            out.push((amp / GROUPS as f32 * y * r).round_ties_even() as i16);
        }
    }
    out
}

// ---- demodulation ----
/// Tone energies E[g*16+t] of symbol k of a frame at t0, clock ratio a (RX samples per TX sample).
fn energies(x: &[i16], t0: f64, a: f64, k: usize, e: &mut [f32; TONES], macs: &mut u64) -> bool {
    let i0 = (t0 + (k * SPS + SPS - WIN) as f64 * a).round() as i64;
    let l = (WIN as f64 * a).round() as usize;
    if i0 < 0 || i0 as usize + l > x.len() {
        return false;
    }
    let p = &x[i0 as usize..i0 as usize + l];
    for g in 0..GROUPS {
        for t in 0..GTONES {
            let w = 2.0 * PI_F * (F0 + g as f32 * GDF + t as f32 * DF) / (a * FS as f64) as f32;
            let (c, s) = (w.cos(), w.sin());
            let (mut zr, mut zi, mut ar, mut ai) = (1f32, 0f32, 0f32, 0f32);
            for &pj in p {
                ar += pj as f32 * zr;
                ai -= pj as f32 * zi;
                let nr = zr * c - zi * s;
                zi = zr * s + zi * c;
                zr = nr;
            }
            e[g * GTONES + t] = ar * ar + ai * ai;
        }
    }
    *macs += (TONES * l) as u64;
    true
}

/// How much more the expected tone holds than the others: about 1 on the sync word, 0 on noise.
fn contrast(e: &[f32; TONES], k: usize) -> f64 {
    let mut sum = 0.0;
    for g in 0..GROUPS {
        let eg = &e[g * GTONES..(g + 1) * GTONES];
        let tot = eg.iter().fold(1e-9, |s, &v| s + v as f64);
        let ex = eg[tone_of(None, k, g)] as f64;
        sum += (ex - (tot - ex) / (GTONES - 1) as f64) / tot;
    }
    sum / GROUPS as f64
}

fn sync_metric(x: &[i16], t0: f64, a: f64, macs: &mut u64) -> f64 {
    let mut e = [0f32; TONES];
    let mut sum = 0.0;
    for k in 0..KS {
        if !energies(x, t0, a, k, &mut e, macs) {
            return -1.0;
        }
        sum += contrast(&e, k);
    }
    sum / KS as f64
}

fn refine(x: &[i16], lo: f64, hi: f64, step: f64, a: f64, t0: &mut f64, macs: &mut u64) -> f64 {
    let mut best = -2.0;
    let mut t = lo;
    while t <= hi {
        let v = sync_metric(x, t, a, macs);
        if v > best {
            best = v;
            *t0 = t;
        }
        t += step;
    }
    best
}

/// Hard decisions on data symbols [k0, k1) into cw (when given); returns the mean share of each
/// group's energy in its strongest tone.
fn demod(x: &[i16], t0: f64, a: f64, k0: usize, k1: usize, mut cw: Option<&mut [u8; CW]>, macs: &mut u64) -> f64 {
    let mut e = [0f32; TONES];
    let mut share = 0.0;
    if let Some(cw) = cw.as_deref_mut() {
        cw.fill(0);
    }
    for k in k0..k1 {
        if !energies(x, t0, a, KS + k, &mut e, macs) {
            e.fill(0.0);
        }
        for g in 0..GROUPS {
            let eg = &e[g * GTONES..(g + 1) * GTONES];
            let mut bt = 0;
            for t in 1..GTONES {
                if eg[t] > eg[bt] {
                    bt = t;
                }
            }
            let tot = eg.iter().fold(1e-9, |s, &v| s + v as f64);
            share += eg[bt] as f64 / tot;
            let Some(cw) = cw.as_deref_mut() else { continue };
            for i in 0..4 {
                let bit = k * BPS + g * 4 + i;
                if bit < CW * 8 && (bt >> (3 - i)) & 1 != 0 {
                    cw[bit >> 3] |= 0x80 >> (bit & 7);
                }
            }
        }
    }
    share / ((k1 - k0) * GROUPS) as f64
}

/// How a decode went.
#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    /// Where the frame was found, and the clock ratio (RX samples per TX sample) it decoded at.
    pub t0: f64,
    pub a: f64,
    /// Its sync metric: about 1 clean, 0 on noise.
    pub metric: f64,
    /// Byte errors RS corrected, -1 if it failed.
    pub rs_fixed: i32,
    /// Clock ratios handed to RS.
    pub tries: usize,
    pub macs: u64,
}

const DRIFT_STEP: f64 = 0.0005; // the clock ratios tried: 1 +- 0.7% in steps of 0.05%
const NA: usize = 2 * 14 + 1;
const TAIL: usize = 8; // data symbols at the end of the frame that rank the clock ratios
const STEP: f64 = (WIN / 8) as f64;

/// Decodes the frame whose sync was found near t (at the nominal clock). Drift shows most at the
/// end of the frame, so the ratios go to RS cleanest tail first: the right one is nearly always
/// the first tried.
pub fn decode_at(x: &[i16], t: f64, st: &mut Stats) -> Option<[u8; PAYLOAD]> {
    let mut s = Stats { t0: t, a: 1.0, metric: -2.0, rs_fixed: -1, tries: 0, macs: st.macs };
    let mut t1 = t;
    s.metric = refine(x, t - 2.0 * STEP, t + 2.0 * STEP, STEP / 4.0, 1.0, &mut t1, &mut s.macs);
    let ratio = |i: usize| 1.0 + (i as f64 - (NA / 2) as f64) * DRIFT_STEP;
    let mut sh = [0f64; NA];
    for (i, shi) in sh.iter_mut().enumerate() {
        *shi = demod(x, t1, ratio(i), KD - TAIL, KD, None, &mut s.macs);
    }
    for tries in 1..=NA {
        s.tries = tries;
        let mut i = 0;
        for j in 1..NA {
            if sh[j] > sh[i] {
                i = j;
            }
        }
        sh[i] = -1.0;
        let (a, mut ts) = (ratio(i), t1);
        refine(x, t1 - 3.0, t1 + 3.0, 1.0, a, &mut ts, &mut s.macs);
        let mut cw = [0u8; CW];
        demod(x, ts, a, 0, KD, Some(&mut cw), &mut s.macs);
        let Some(fixed) = rs_decode(&mut cw) else { continue };
        if cw[0] as usize != PAYLOAD || crc16(&cw[..1 + PAYLOAD]).to_be_bytes() != cw[FRAME - 2..FRAME] {
            continue;
        }
        s.t0 = ts;
        s.a = a;
        s.rs_fixed = fixed as i32;
        *st = s;
        return cw[1..=PAYLOAD].try_into().ok();
    }
    s.tries = NA;
    *st = s;
    None
}

/// Finds and decodes one frame anywhere in x.
pub fn decode(x: &[i16], st: &mut Stats) -> Option<[u8; PAYLOAD]> {
    const NCAND: usize = 3;
    let mut s = Stats { a: 1.0, metric: -2.0, rs_fixed: -1, ..Default::default() };
    let (mut ct, mut cm) = ([-1f64; NCAND], [-2f64; NCAND]);
    // Coarse scan at the nominal clock: the sync word is short enough for drift not to matter.
    // One symbol screens a position before all four are measured.
    let mut t = 0.0;
    while t + (SPAN as f64) < x.len() as f64 {
        let mut e = [0f32; TONES];
        if energies(x, t, 1.0, 0, &mut e, &mut s.macs) && contrast(&e, 0) >= 0.1 {
            let v1 = sync_metric(x, t, 1.0, &mut s.macs);
            let mut slot: isize = -1;
            for i in 0..NCAND {
                if ct[i] >= 0.0 && (ct[i] - t).abs() < (KS * SPS) as f64 {
                    slot = if cm[i] < v1 { i as isize } else { -2 };
                    break;
                }
            }
            if slot == -1 {
                // a new peak: replace the weakest
                slot = 0;
                for i in 1..NCAND {
                    if cm[i] < cm[slot as usize] {
                        slot = i as isize;
                    }
                }
                if cm[slot as usize] >= v1 {
                    slot = -2;
                }
            }
            if slot >= 0 {
                ct[slot as usize] = t;
                cm[slot as usize] = v1;
            }
        }
        t += STEP;
    }
    for _ in 0..NCAND {
        let mut c = 0;
        for i in 1..NCAND {
            if cm[i] > cm[c] {
                c = i;
            }
        }
        if ct[c] < 0.0 || cm[c] < 0.2 {
            break;
        }
        s.t0 = ct[c];
        if let Some(v) = decode_at(x, ct[c], &mut s) {
            *st = s;
            return Some(v);
        }
        cm[c] = -2.0;
        ct[c] = -1.0;
    }
    *st = s;
    None
}

// ---- the 512-point FFT, radix 2 ----
struct Twiddles([f32; 256], [f32; 256]);

fn twiddles() -> &'static Twiddles {
    static T: std::sync::OnceLock<Twiddles> = std::sync::OnceLock::new();
    T.get_or_init(|| {
        let mut t = Twiddles([0.0; 256], [0.0; 256]);
        for i in 0..256 {
            t.0[i] = (2.0 * PI_F * i as f32 / 512.0).cos();
            t.1[i] = (2.0 * PI_F * i as f32 / 512.0).sin();
        }
        t
    })
}

/// |X[k]|^2 of the 512-point DFT of x for k in [0, 256).
pub fn spectrum(x: &[i16], p: &mut [f32; 256]) {
    let tw = twiddles();
    let (mut re, mut im) = ([0f32; 512], [0f32; 512]);
    for (i, &xi) in x[..512].iter().enumerate() {
        re[(i as u32).reverse_bits() as usize >> 23] = xi as f32;
    }
    let mut len = 2;
    while len <= 512 {
        let (half, step) = (len / 2, 512 / len);
        for i in (0..512).step_by(len) {
            for k in 0..half {
                let (wr, wi) = (tw.0[k * step], -tw.1[k * step]);
                let (a, b) = (i + k, i + k + half);
                let (tr, ti) = (re[b] * wr - im[b] * wi, re[b] * wi + im[b] * wr);
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
            }
        }
        len <<= 1;
    }
    for k in 0..256 {
        p[k] = re[k] * re[k] + im[k] * im[k];
    }
}

// ---- the receiver ----
const HOP: u64 = 64;
const HIST: usize = 64; // hops of tone energies kept: the sync word spans 33
const THRESH: f64 = 0.2;
/// The samples the receiver keeps.
pub const RX_MIN: usize = SPAN + 8 * SPS;

/// The receiver, fed the microphone at 16 kHz as it comes. It keeps the recent audio and one FFT
/// of it every 64 samples, and decodes where a sync word shows.
pub struct Rx {
    buf: Vec<i16>,
    cap: usize,
    base: u64, // stream index of buf[0]
    hop: u64,  // the next hop to transform: window [hop*64, hop*64 + 512)
    e: Box<[[f32; TONES]; HIST]>,
    /// Best sync metric of the frame waiting to be decoded, 0 if none.
    pub peak: f64,
    peak_at: u64, // where it starts, as a stream index
    /// Of the last decode tried.
    pub st: Stats,
}

impl Default for Rx {
    fn default() -> Rx {
        Rx::new(RX_MIN)
    }
}

impl Rx {
    pub fn new(cap: usize) -> Rx {
        let cap = cap.max(RX_MIN);
        Rx { buf: Vec::with_capacity(cap), cap, base: 0, hop: 0, e: Box::new([[0.0; TONES]; HIST]), peak: 0.0, peak_at: 0, st: Stats::default() }
    }

    // A frame at hop h0 has its sync symbols' windows at hops h0+2, +12, +22, +32 (640 = 10 hops).
    fn metric(&self, h0: u64) -> f64 {
        (0..KS).map(|k| contrast(&self.e[((h0 + 2 + 10 * k as u64) % HIST as u64) as usize], k)).sum::<f64>() / KS as f64
    }

    /// Takes samples; the voucher once a frame decoded from them.
    pub fn push(&mut self, mut x: &[i16]) -> Option<[u8; PAYLOAD]> {
        while !x.is_empty() {
            if self.buf.len() == self.cap {
                // keep what a waiting frame or the next sync word may still need
                let mut keep = if self.hop > 40 { (self.hop - 40) * HOP } else { 0 };
                if self.peak > 0.0 && self.peak_at < keep + 256 {
                    keep = self.peak_at.saturating_sub(256);
                }
                if keep <= self.base {
                    keep = self.base + self.cap as u64 / 4; // no room: the waiting frame is dropped
                }
                let drop = ((keep - self.base) as usize).min(self.buf.len());
                self.buf.drain(..drop);
                self.base += drop as u64;
                if self.peak > 0.0 && self.peak_at < self.base + 256 {
                    self.peak = 0.0;
                }
                if self.hop * HOP < self.base {
                    self.hop = self.base.div_ceil(HOP);
                }
            }
            let m = (self.cap - self.buf.len()).min(x.len());
            self.buf.extend_from_slice(&x[..m]);
            x = &x[m..];
            let end = self.base + self.buf.len() as u64;
            while self.hop * HOP + WIN as u64 <= end {
                let mut p = [0f32; 256];
                spectrum(&self.buf[(self.hop * HOP - self.base) as usize..], &mut p);
                let e = &mut self.e[(self.hop % HIST as u64) as usize];
                for g in 0..GROUPS {
                    for t in 0..GTONES {
                        e[g * GTONES + t] = p[32 + 32 * g + 2 * t]; // tone t of group g at 16 kHz
                    }
                }
                if self.hop >= 32 {
                    // the sync word starting 32 hops back is whole
                    let h0 = self.hop - 32;
                    let mt = self.metric(h0);
                    // One frame waits at a time: a stronger sync word replaces it.
                    if mt >= THRESH && mt > self.peak {
                        self.peak = mt;
                        self.peak_at = h0 * HOP;
                    }
                }
                self.hop += 1;
                // Decoded once no stronger peak can come and the whole frame is in.
                if self.peak > 0.0
                    && self.hop * HOP > self.peak_at + ((KS + 1) * SPS + WIN) as u64
                    && self.peak_at + SPAN as u64 <= end
                    && self.peak_at >= self.base + 256
                {
                    self.st.macs = 0;
                    let v = decode_at(&self.buf, (self.peak_at - self.base) as f64, &mut self.st);
                    self.st.t0 += self.base as f64;
                    self.peak = 0.0;
                    if v.is_some() {
                        return v;
                    }
                }
            }
        }
        None
    }
}

/// Turns a stream at `from` Hz into 16 kHz, for the receiver: a windowed sinc, cut off at 7.2 kHz.
pub struct Resampler {
    step: f64,
    fc: f64,
    pos: f64,
    buf: Vec<f32>,
}

const HALF: i64 = 24; // taps each side, at the input rate

impl Resampler {
    pub fn new(from: u32) -> Resampler {
        let step = from as f64 / FS as f64;
        Resampler { step, fc: 0.45 / step, pos: HALF as f64, buf: vec![0.0; HALF as usize] }
    }

    /// Takes samples from -1 to 1; adds what they make at 16 kHz to `out`.
    pub fn push(&mut self, x: &[f32], out: &mut Vec<i16>) {
        self.buf.extend_from_slice(x);
        let w = HALF as f64 + 0.5;
        while (self.pos.floor() as i64 + HALF) < self.buf.len() as i64 {
            let i0 = self.pos.floor() as i64;
            let mut acc = 0.0;
            for k in i0 - HALF + 1..=i0 + HALF {
                let d = self.pos - k as f64;
                let u = std::f64::consts::PI * 2.0 * self.fc * d;
                let s = if u.abs() < 1e-12 { 1.0 } else { u.sin() / u };
                acc += self.buf[k as usize] as f64 * 2.0 * self.fc * s * (0.5 + 0.5 * (std::f64::consts::PI * d / w).cos());
            }
            out.push((acc * 32768.0).round().clamp(-32768.0, 32767.0) as i16);
            self.pos += self.step;
        }
        let drop = (self.pos.floor() as i64 - HALF).max(0) as usize;
        self.buf.drain(..drop);
        self.pos -= drop as f64;
    }
}

/// A 16 kHz mono PCM16 WAV, as the T-Deck's tests write and read them.
pub fn wav(x: &[i16]) -> Vec<u8> {
    let d = (x.len() * 2) as u32;
    let mut w = Vec::with_capacity(44 + d as usize);
    w.extend(b"RIFF");
    w.extend((36 + d).to_le_bytes());
    w.extend(b"WAVEfmt ");
    w.extend(16u32.to_le_bytes());
    w.extend(1u16.to_le_bytes());
    w.extend(1u16.to_le_bytes());
    w.extend(FS.to_le_bytes());
    w.extend((FS * 2).to_le_bytes());
    w.extend(2u16.to_le_bytes());
    w.extend(16u16.to_le_bytes());
    w.extend(b"data");
    w.extend(d.to_le_bytes());
    x.iter().for_each(|s| w.extend(s.to_le_bytes()));
    w
}

/// The samples of a WAV that `wav` would write: 16 kHz mono PCM16 with a 44-byte header.
pub fn unwav(b: &[u8]) -> Option<Vec<i16>> {
    let h = b.get(..44)?;
    let ok = &h[..4] == b"RIFF"
        && &h[8..16] == b"WAVEfmt "
        && h[20] == 1
        && h[22] == 1
        && u32::from_le_bytes(h[24..28].try_into().ok()?) == FS
        && h[34] == 16
        && &h[36..40] == b"data";
    ok.then(|| b[44..].as_chunks::<2>().0.iter().map(|&c| i16::from_le_bytes(c)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pigeonhole_protocols::protocols::katzenqt1::engine::{parse_voucher_code, voucher_code};

    /// The voucher of pigeonhole_reticulous's voucher vector ("mint"), which its fixtures carry.
    const VECTOR: &str = "f2ca3f0fe0fdaa414c9e55ebcd95495de017271f1c06a56c8a1bbf856cae6d6f";

    fn fixture(name: &str) -> Vec<i16> {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/fskv").join(name);
        unwav(&std::fs::read(p).unwrap()).unwrap()
    }

    fn vector() -> [u8; 32] {
        hex::decode(VECTOR).unwrap().try_into().unwrap()
    }

    // ---- the T-Deck's room: drift, a small speaker's and a MEMS mic's band, reverb, noise ----
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            self.0.wrapping_mul(0x2545F4914F6CDD1D)
        }
        fn uniform(&mut self) -> f64 {
            (self.next() >> 11) as f64 * (1.0 / 9007199254740992.0)
        }
        fn gauss(&mut self) -> f64 {
            let (u, v) = (self.uniform() + 1e-300, self.uniform());
            (-2.0 * u.ln()).sqrt() * (2.0 * std::f64::consts::PI * v).cos()
        }
    }

    struct Bq([f64; 5], f64, f64);
    impl Bq {
        // RBJ cookbook: 0 lowpass, 1 highpass, 2 peak
        fn new(kind: u8, f: f64, q: f64, gain_db: f64) -> Bq {
            let w = 2.0 * std::f64::consts::PI * f / FS as f64;
            let (al, c, a) = (w.sin() / (2.0 * q), w.cos(), 10f64.powf(gain_db / 40.0));
            let (b0, b1, b2, a0, a1, a2) = match kind {
                0 => ((1.0 - c) / 2.0, 1.0 - c, (1.0 - c) / 2.0, 1.0 + al, -2.0 * c, 1.0 - al),
                1 => ((1.0 + c) / 2.0, -(1.0 + c), (1.0 + c) / 2.0, 1.0 + al, -2.0 * c, 1.0 - al),
                _ => (1.0 + al * a, -2.0 * c, 1.0 - al * a, 1.0 + al / a, -2.0 * c, 1.0 - al / a),
            };
            Bq([b0 / a0, b1 / a0, b2 / a0, a1 / a0, a2 / a0], 0.0, 0.0)
        }
        fn run(&mut self, x: &mut [f64]) {
            for v in x {
                let y = self.0[0] * *v + self.1;
                self.1 = self.0[1] * *v - self.0[3] * y + self.2;
                self.2 = self.0[2] * *v - self.0[4] * y;
                *v = y;
            }
        }
    }

    const SIG: f64 = 32768.0 * 0.0316227766; // -30 dBFS RMS

    /// `tx` after `lead` samples of quiet, through a bare, echoing room 20-50 cm across: stretched
    /// by 1 + drift, at -30 dBFS RMS with white noise `snr` dB below. Also where the frame starts.
    fn room(r: &mut Rng, tx: &[i16], lead: usize, drift: f64, snr: f64) -> (Vec<i16>, f64) {
        let nx = lead + tx.len() + 6400;
        let a = 1.0 + drift;
        let mut x = vec![0f64; nx];
        x[lead..lead + tx.len()].iter_mut().zip(tx).for_each(|(d, &s)| *d = s as f64);
        let n = (nx as f64 * a) as usize;
        let mut y: Vec<f64> = (0..n)
            .map(|i| {
                let u = i as f64 / a;
                let i0 = u.floor() as i64;
                (i0 - 15..=i0 + 16)
                    .filter(|&k| k >= 0 && (k as usize) < nx && x[k as usize] != 0.0)
                    .map(|k| {
                        let d = u - k as f64;
                        let s = if d.abs() < 1e-12 { 1.0 } else { (std::f64::consts::PI * d).sin() / (std::f64::consts::PI * d) };
                        x[k as usize] * s * (0.5 + 0.5 * (std::f64::consts::PI * d / 16.5).cos())
                    })
                    .sum()
            })
            .collect();
        for mut f in [Bq::new(1, 300.0, 0.707, 0.0), Bq::new(1, 300.0, 0.707, 0.0), Bq::new(0, 6000.0, 0.707, 0.0), Bq::new(0, 6000.0, 0.707, 0.0), Bq::new(2, 2500.0, 1.5, 6.0), Bq::new(2, 4500.0, 2.0, -6.0)] {
            f.run(&mut y);
        }
        // rt60 0.3 s, direct to reverberant 10 dB, six early reflections in 1-15 ms up to 0.3
        let (rt60, fs) = (0.3, FS as f64);
        let l = (rt60 * fs * 2.0 / 3.0) as usize;
        let mut h = vec![0f64; l];
        h[0] = 1.0;
        for _ in 0..6 {
            let at = (fs * (0.001 + 0.014 * r.uniform())) as usize;
            h[at] += if r.uniform() < 0.5 { -1.0 } else { 1.0 } * 0.3 * (0.4 + 0.6 * r.uniform());
        }
        let start = (0.005 * fs) as usize;
        let mut e = 0.0;
        for (i, hi) in h.iter_mut().enumerate().skip(start) {
            *hi += r.gauss() * (-6.9 * (i - start) as f64 / (rt60 * fs)).exp();
            e += *hi * *hi;
        }
        let g = (10f64.powf(-1.0) / (e + 1e-12)).sqrt();
        h[start..].iter_mut().for_each(|v| *v *= g);
        let mut z = vec![0f64; n];
        for i in 0..n {
            if y[i] != 0.0 {
                for k in 0..l.min(n - i) {
                    z[i + k] += y[i] * h[k];
                }
            }
        }
        let (s0, s1) = ((lead as f64 * a) as usize, ((lead + tx.len()) as f64 * a) as usize);
        let p = z[s0..s1].iter().map(|v| v * v).sum::<f64>() / (s1 - s0) as f64;
        let (g, sn) = (SIG / p.sqrt(), SIG / 10f64.powf(snr / 20.0));
        let out = z.iter().map(|v| (v * g + sn * r.gauss()).round().clamp(-32768.0, 32767.0) as i16).collect();
        (out, lead as f64 * a)
    }

    fn random_voucher(r: &mut Rng) -> [u8; 32] {
        std::array::from_fn(|_| r.next() as u8)
    }

    #[test]
    fn check_code_matches_the_t_deck() {
        // test_fskv: the vector's voucher checks as 1939, as Python's hashlib.shake_256 gives.
        assert_eq!(check(&vector()), "1939");
        assert_eq!(parse_voucher_code(&voucher_code(&vector())).as_deref(), Some(&vector()[..]));
    }

    #[test]
    fn crc_and_reed_solomon() {
        assert_eq!(crc16(b"123456789"), 0x29B1, "CRC-16/CCITT-FALSE check value");
        let mut r = Rng(0x9E3779B97F4A7C15);
        // Up to 8 byte errors are fixed; with 9 to 16, RS, the length and the CRC let none through.
        for it in 0..4000 {
            let msg: Vec<u8> = (0..FRAME).map(|_| r.next() as u8).collect();
            let cw = rs_encode(&msg);
            let mut rx = cw.clone();
            let ne = it % 9;
            for _ in 0..ne {
                let pos = loop {
                    let p = (r.next() % CW as u64) as usize;
                    if rx[p] == cw[p] {
                        break p;
                    }
                };
                rx[pos] ^= (1 + r.next() % 255) as u8;
            }
            assert_eq!(rs_decode(&mut rx), Some(ne));
            assert_eq!(rx, cw);
        }
        for it in 0..4000 {
            let v = random_voucher(&mut r);
            let mut cw = frame(&v);
            for _ in 0..9 + it % 8 {
                cw[(r.next() % CW as u64) as usize] ^= (1 + r.next() % 255) as u8;
            }
            if rs_decode(&mut cw).is_some() && cw[0] == 32 {
                assert!(crc16(&cw[..33]).to_be_bytes() != cw[33..35] || cw[1..33] == v, "a wrong voucher passed");
            }
        }
    }

    #[test]
    fn fft_is_the_dft() {
        let mut r = Rng(3);
        let x: Vec<i16> = (0..512).map(|_| (r.next() % 20000) as i16 - 10000).collect();
        let mut p = [0f32; 256];
        spectrum(&x, &mut p);
        for b in (32..160).step_by(2) {
            let (mut re, mut im) = (0f64, 0f64);
            for (j, &xj) in x.iter().enumerate() {
                let ph = 2.0 * std::f64::consts::PI * (b * j) as f64 / 512.0;
                re += xj as f64 * ph.cos();
                im -= xj as f64 * ph.sin();
            }
            let want = re * re + im * im;
            assert!((p[b] as f64 - want).abs() / want < 1e-3, "bin {b}");
        }
    }

    #[test]
    fn modulates_as_the_t_deck() {
        // tx.wav is fskv_modulate(vector, 16384) after 0.2 s of quiet, then 0.5 s of quiet.
        let tx = fixture("tx.wav");
        assert_eq!(tx.len(), 3200 + SAMPLES + 8000);
        assert_eq!(modulate(&vector(), 16384.0, FS), tx[3200..3200 + SAMPLES]);
        // The T-Deck's level: amplitude 9268 is -20 dBFS RMS.
        let x = modulate(&vector(), 9268.0, FS);
        let dbfs = 20.0 * ((x.iter().map(|&s| s as f64 * s as f64).sum::<f64>() / x.len() as f64).sqrt() / 32768.0).log10();
        assert!((dbfs + 20.0).abs() < 0.5, "{dbfs}");
    }

    #[test]
    fn decodes_the_t_deck_fixtures() {
        for f in ["tx.wav", "room-10dB.wav", "room-0dB.wav"] {
            let x = fixture(f);
            let mut st = Stats::default();
            assert_eq!(decode(&x, &mut st), Some(vector()), "{f}, blind");
            let mut rx = Rx::default();
            let got = x.chunks(320).find_map(|c| rx.push(c));
            assert_eq!(got, Some(vector()), "{f}, streamed");
        }
    }

    #[test]
    fn clean_loopback() {
        let mut x = vec![0i16; 3000];
        x.extend(modulate(&vector(), 9268.0, FS));
        x.extend([0; 3000]);
        let mut st = Stats::default();
        assert_eq!(decode(&x, &mut st), Some(vector()));
        // Any start up to the 8 ms guard early puts every window inside its symbol.
        assert!(st.t0 >= 3000.0 - 128.0 && st.t0 <= 3002.0, "{st:?}");
        assert!((st.a - 1.0).abs() <= 0.001 && st.rs_fixed == 0 && st.tries == 1, "{st:?}");
    }

    #[test]
    fn through_the_room_with_drift() {
        let mut r = Rng(1);
        for snr in [0.0, 10.0] {
            for t in 0..4 {
                let v = random_voucher(&mut r);
                let drift = if t & 1 == 1 { 0.005 } else { -0.005 };
                let lead = (16000.0 * (0.5 + 1.5 * r.uniform())) as usize;
                let (x, _) = room(&mut r, &modulate(&v, 16384.0, FS), lead, drift, snr);
                let mut st = Stats::default();
                assert_eq!(decode(&x, &mut st), Some(v), "blind, {snr} dB, drift {drift}: {st:?}");
                let mut rx = Rx::default();
                assert_eq!(x.chunks(320).find_map(|c| rx.push(c)), Some(v), "streamed, {snr} dB, drift {drift}");
            }
        }
    }

    #[test]
    fn nothing_in_noise() {
        let mut r = Rng(5);
        let sn = SIG;
        let x: Vec<i16> = (0..16000 * 15).map(|_| (sn * r.gauss()).round() as i16).collect();
        let mut rx = Rx::default();
        assert!(x.chunks(320).all(|c| rx.push(c).is_none()));
        assert_eq!(decode(&x, &mut Stats::default()), None);
    }

    #[test]
    fn played_at_48_and_44_khz_heard_at_16() {
        // What the speaker plays at the device's rate, and what the mic hears at it, both
        // resampled to the modem's 16 kHz.
        for fs in [48000, 44100, 16000] {
            let mut x = vec![0i16; fs as usize / 5];
            x.extend(modulate(&vector(), 9268.0, fs));
            x.extend(vec![0; fs as usize / 2]);
            let f: Vec<f32> = x.iter().map(|&s| s as f32 / 32768.0).collect();
            let (mut rs, mut y) = (Resampler::new(fs), Vec::new());
            rs.push(&f, &mut y); // in one block, as a listener that fell behind takes it
            let want = x.len() as f64 * 16000.0 / fs as f64;
            assert!((y.len() as f64 - want).abs() < 30.0, "{fs}: {} samples, not {want}", y.len());
            let mut st = Stats::default();
            assert_eq!(decode(&y, &mut st), Some(vector()), "{fs} Hz");
            assert!(st.rs_fixed == 0 && (st.a - 1.0).abs() < 0.001, "{fs} Hz: {st:?}");
            // As Listen hears it: the device's blocks, resampled and streamed to the receiver.
            let (mut rs, mut rx, mut pcm) = (Resampler::new(fs), Rx::default(), Vec::new());
            let heard = f.chunks(fs as usize / 50 + 7).find_map(|c| {
                pcm.clear();
                rs.push(c, &mut pcm);
                rx.push(&pcm)
            });
            assert_eq!(heard, Some(vector()), "{fs} Hz, streamed");
        }
    }

    #[test]
    fn wav_round_trip() {
        let x = [1i16, -2, 300, i16::MIN];
        assert_eq!(unwav(&wav(&x)).unwrap(), x);
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/fskv/tx.wav");
        assert_eq!(wav(&fixture("tx.wav")), std::fs::read(p).unwrap(), "the T-Deck's header");
        assert!(unwav(b"RIFF").is_none());
    }
}
