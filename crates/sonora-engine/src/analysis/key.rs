//! Musical key estimation, and key notations (musical, Camelot, Open Key).
//!
//! One streaming pass collects a fine pitch-class histogram (10 bins per
//! semitone) from the tonal peaks of consecutive ~0.37 s spectra between
//! 50 Hz and 3 kHz. Every spectral peak that stands 12 dB above its
//! neighbourhood adds its (cube-root compressed) amplitude at its interpolated
//! pitch, and every frame counts equally, so loud drops do not drown out
//! quieter harmonic passages, and drums (broadband, few clear peaks) count
//! little. At the end:
//!
//! 1. the tuning reference is estimated from where the peaks sit within the
//!    semitone, so recordings tuned to A = 432 Hz or off-speed vinyl rips
//!    are read correctly;
//! 2. the histogram is folded into 12 pitch classes, with the bass line
//!    (below 150 Hz) counted separately and added at half weight, since it
//!    tends to sit on the tonic and the chord roots;
//! 3. the result is correlated with the Krumhansl–Kessler major and minor
//!    key profiles, extended with the harmonics real notes produce (Gómez,
//!    2006), giving the key, the runner-up and a confidence. Histogram methods
//!    confuse a key with its relative major/minor more than with anything
//!    else; on the Camelot wheel those share a number and still mix;
//! 4. keys of 10-second segments show whether one key holds through the track.
//!
//! Keys are numbered 0–23: 0–11 are major keys with that tonic pitch class
//! (0 = C), 12–23 the minor keys (12 = C minor).

use crate::dsp::fft::RealFft;
use crate::dsp::hann;

const LO_HZ: f64 = 50.0;
/// Peaks below this belong to the bass line, which is weighed separately.
const BASS_SPLIT_HZ: f64 = 150.0;
/// Weight of the bass profile relative to everything above it.
const BASS_WEIGHT: f64 = 0.5;
/// Amplitude ratio between successive harmonics assumed when key profiles
/// are turned into the pitch-class profiles real (harmonic-rich) notes produce.
const HARMONIC_DECAY: f64 = 0.75;
const HI_HZ: f64 = 3000.0;
const FINE: usize = 120;
const SEGMENT_SECS: f64 = 10.0;
const MAX_SECONDS: f64 = 1800.0;
/// A peak must stand this far above the mean of its neighbourhood (×, amplitude:
/// 12 dB, which random noise peaks practically never reach).
const PEAK_RATIO: f32 = 4.0;
/// Neighbourhood half-width for that mean, in bins.
const NEIGHBOURHOOD: usize = 12;
/// Peaks quieter than this (amplitude re full scale, about −80 dBFS) are ignored.
const FLOOR: f32 = 1e-4;
/// Shortest amount of tonal material (seconds) given a key.
const MIN_TONAL_SECS: f64 = 6.0;

/// Krumhansl & Kessler (1982) probe-tone profiles, tonic first.
const KK_MAJOR: [f64; 12] = [6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88];
const KK_MINOR: [f64; 12] = [6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17];

#[derive(Clone, Debug, PartialEq)]
pub struct KeyResult {
    /// 0–11 major, 12–23 minor (tonic pitch class = `key % 12`, 0 = C).
    pub key: u8,
    /// 0–1.
    pub confidence: f64,
    /// The next most likely key (often the relative or a neighbouring key).
    pub second: u8,
    /// Deviation of the tuning reference from A = 440 Hz, in cents.
    pub tuning_cents: f64,
    /// Pitch-class profile of the track (C first), summing to 1.
    pub chroma: [f64; 12],
    /// Parts of the track are clearly in another key (modulation, key changes).
    pub changes: bool,
}

/// Streaming pitch-class collector; [`KeyMeter::finish`] estimates the key.
pub struct KeyMeter {
    ch: usize,
    n: usize,
    hop: usize,
    fs: f64,
    fft: RealFft,
    window: Vec<f32>,
    mono: Vec<f32>,
    lo_bin: usize,
    hi_bin: usize,
    buf: Vec<f32>,
    re: Vec<f32>,
    im: Vec<f32>,
    mag: Vec<f32>,
    prefix: Vec<f32>,
    frame_bass: [f32; FINE],
    frame_treble: [f32; FINE],
    bass_total: [f64; FINE],
    treble_total: [f64; FINE],
    /// Per segment: bass and treble histograms.
    segments: Vec<([f32; FINE], [f32; FINE])>,
    seg_tonal: Vec<u32>,
    frames_per_segment: usize,
    frames: usize,
    tonal_frames: usize,
    max_frames: usize,
    scale: f32,
}

fn nearest_pow2(x: f64) -> usize {
    1usize << (x.max(1.0).log2().round() as u32)
}

impl KeyMeter {
    pub fn new(fs: u32, ch: usize) -> KeyMeter {
        let fs_f = fs.max(1) as f64;
        let n = nearest_pow2(fs_f / 2.7).clamp(1024, 65_536);
        // Non-overlapping frames: a long-term profile does not need more.
        let hop = n;
        let df = fs_f / n as f64;
        let lo_bin = ((LO_HZ / df).floor() as usize).max(NEIGHBOURHOOD + 1);
        let hi_bin = ((HI_HZ.min(fs_f * 0.45) / df).ceil() as usize).min(n / 2 - NEIGHBOURHOOD - 2);
        KeyMeter {
            ch: ch.max(1),
            n,
            hop,
            fs: fs_f,
            fft: RealFft::new(n),
            window: hann(n),
            mono: Vec::with_capacity(n + 32_768),
            lo_bin,
            hi_bin: hi_bin.max(lo_bin + 1),
            buf: vec![0.0; n],
            re: vec![0.0; n / 2 + 1],
            im: vec![0.0; n / 2 + 1],
            mag: vec![0.0; n / 2 + 1],
            prefix: vec![0.0; n / 2 + 2],
            frame_bass: [0.0; FINE],
            frame_treble: [0.0; FINE],
            bass_total: [0.0; FINE],
            treble_total: [0.0; FINE],
            segments: Vec::new(),
            seg_tonal: Vec::new(),
            frames_per_segment: ((SEGMENT_SECS * fs_f / hop as f64).round() as usize).max(1),
            frames: 0,
            tonal_frames: 0,
            max_frames: (MAX_SECONDS * fs_f / hop as f64) as usize,
            scale: 2.0 / (n as f32 / 2.0),
        }
    }

    pub fn process(&mut self, data: &[f32]) {
        if self.frames >= self.max_frames {
            return;
        }
        let inv = 1.0 / self.ch as f32;
        for fr in data.chunks_exact(self.ch) {
            self.mono.push(fr.iter().sum::<f32>() * inv);
        }
        let mut pos = 0;
        while pos + self.n <= self.mono.len() {
            self.frame(pos);
            pos += self.hop;
            if self.frames >= self.max_frames {
                self.mono.clear();
                return;
            }
        }
        self.mono.drain(..pos);
    }

    fn frame(&mut self, pos: usize) {
        let x = &self.mono[pos..pos + self.n];
        for ((b, &v), &w) in self.buf.iter_mut().zip(x).zip(&self.window) {
            *b = v * w;
        }
        self.fft.forward(&self.buf, &mut self.re, &mut self.im);
        let (lo, hi) = (self.lo_bin - NEIGHBOURHOOD, self.hi_bin + NEIGHBOURHOOD + 1);
        for k in lo..=hi {
            self.mag[k] = (self.re[k] * self.re[k] + self.im[k] * self.im[k]).sqrt() * self.scale;
        }
        self.prefix[lo] = 0.0;
        for k in lo..=hi {
            self.prefix[k + 1] = self.prefix[k] + self.mag[k];
        }
        self.frame_bass = [0.0; FINE];
        self.frame_treble = [0.0; FINE];
        let (mut bass_peaks, mut treble_peaks) = (0usize, 0usize);
        let df = self.fs / self.n as f64;
        for k in self.lo_bin..=self.hi_bin {
            let a = self.mag[k];
            if a <= FLOOR || a <= self.mag[k - 1] || a < self.mag[k + 1] {
                continue;
            }
            let (s, e) = (k - NEIGHBOURHOOD, k + NEIGHBOURHOOD + 1);
            let local = (self.prefix[e] - self.prefix[s] - a) / (e - s - 1) as f32;
            if a < PEAK_RATIO * local.max(1e-12) {
                continue;
            }
            // Parabolic interpolation of the log magnitude.
            let (y0, y1, y2) = (self.mag[k - 1].max(1e-12).ln(), a.ln(), self.mag[k + 1].max(1e-12).ln());
            let den = y0 - 2.0 * y1 + y2;
            let delta = if den.abs() > 1e-12 { (0.5 * (y0 - y2) / den).clamp(-0.5, 0.5) } else { 0.0 };
            let f = (k as f64 + delta as f64) * df;
            let pitch = 69.0 + 12.0 * (f / 440.0).log2();
            let pos = pitch * 10.0;
            let i0 = pos.floor();
            let frac = (pos - i0) as f32;
            // Cube-root amplitude: harmonics keep their (smaller) share, as
            // the key profiles below assume, without one loud note taking over.
            let w = a.cbrt();
            let i0 = (i0 as i64).rem_euclid(FINE as i64) as usize;
            let hist = if f < BASS_SPLIT_HZ {
                bass_peaks += 1;
                &mut self.frame_bass
            } else {
                treble_peaks += 1;
                &mut self.frame_treble
            };
            hist[i0] += w * (1.0 - frac);
            hist[(i0 + 1) % FINE] += w * frac;
        }
        let seg = self.frames / self.frames_per_segment;
        if self.segments.len() <= seg {
            self.segments.push(([0.0; FINE], [0.0; FINE]));
            self.seg_tonal.push(0);
        }
        self.frames += 1;
        let bass_sum: f32 = self.frame_bass.iter().sum();
        let treble_sum: f32 = self.frame_treble.iter().sum();
        let mut tonal = false;
        if bass_peaks >= 1 && bass_sum > 0.0 {
            tonal = true;
            let inv = 1.0 / bass_sum;
            for (i, &v) in self.frame_bass.iter().enumerate() {
                self.bass_total[i] += (v * inv) as f64;
                self.segments[seg].0[i] += v * inv;
            }
        }
        if treble_peaks >= 2 && treble_sum > 0.0 {
            tonal = true;
            let inv = 1.0 / treble_sum;
            for (i, &v) in self.frame_treble.iter().enumerate() {
                self.treble_total[i] += (v * inv) as f64;
                self.segments[seg].1[i] += v * inv;
            }
        }
        if tonal {
            self.tonal_frames += 1;
            self.seg_tonal[seg] += 1;
        }
    }

    pub fn finish(self) -> Option<KeyResult> {
        let secs_per_frame = self.hop as f64 / self.fs;
        if (self.tonal_frames as f64) * secs_per_frame < MIN_TONAL_SECS {
            return None;
        }
        let mut all = [0.0; FINE];
        for (a, (b, t)) in all.iter_mut().zip(self.bass_total.iter().zip(&self.treble_total)) {
            *a = b + t;
        }
        let tuning = tuning_offset(&all);
        let bass = normalized(fold_chroma(&self.bass_total, tuning));
        let treble = normalized(fold_chroma(&self.treble_total, tuning));
        let chroma = combine(&bass, &treble);
        // A flat profile has no tonal centre to speak of.
        let mean = chroma.iter().sum::<f64>() / 12.0;
        if chroma.iter().copied().fold(0.0, f64::max) < 1.6 * mean {
            return None;
        }
        let scores = key_scores(&chroma);
        let (best, second) = best_two(&scores);
        let r1 = scores[best];
        let r2 = scores[second];
        if r1 < 0.3 {
            return None;
        }
        // Margin over the runner-up and the absolute fit both matter.
        let confidence = (((r1 - r2) / 0.12).clamp(0.0, 1.0) * 0.6 + ((r1 - 0.3) / 0.5).clamp(0.0, 1.0) * 0.4).clamp(0.0, 1.0);

        // Does one key hold? Segments with enough tonal frames vote.
        let mut voters = 0;
        let mut distant = 0;
        for ((seg_bass, seg_treble), &tonal) in self.segments.iter().zip(&self.seg_tonal) {
            if (tonal as usize) * 3 < self.frames_per_segment {
                continue;
            }
            let c = combine(&normalized(fold_chroma(&seg_bass.map(|v| v as f64), tuning)), &normalized(fold_chroma(&seg_treble.map(|v| v as f64), tuning)));
            let s = key_scores(&c);
            let (k, _) = best_two(&s);
            voters += 1;
            if camelot_distance(k as u8, best as u8) > 1 {
                distant += 1;
            }
        }
        let changes = voters >= 3 && distant as f64 / voters as f64 > 0.35;
        let total: f64 = chroma.iter().sum();
        let mut norm = [0.0; 12];
        for (o, &c) in norm.iter_mut().zip(&chroma) {
            *o = (c / total.max(1e-12) * 1000.0).round() / 1000.0;
        }
        Some(KeyResult {
            key: best as u8,
            confidence: (confidence * 1000.0).round() / 1000.0,
            second: second as u8,
            tuning_cents: (tuning * 100.0 * 10.0).round() / 10.0,
            chroma: norm,
            changes,
        })
    }
}

/// Tuning offset in semitones (−0.5, 0.5]: the circular mean of where peaks sit within the semitone.
fn tuning_offset(hist: &[f64; FINE]) -> f64 {
    let (mut sx, mut sy, mut total) = (0.0, 0.0, 0.0);
    for (i, &v) in hist.iter().enumerate() {
        let a = 2.0 * std::f64::consts::PI * (i % 10) as f64 / 10.0;
        sx += v * a.cos();
        sy += v * a.sin();
        total += v;
    }
    if total <= 0.0 || (sx * sx + sy * sy).sqrt() / total < 0.05 {
        return 0.0;
    }
    sy.atan2(sx) / (2.0 * std::f64::consts::PI)
}

fn normalized(c: [f64; 12]) -> [f64; 12] {
    let s: f64 = c.iter().sum();
    if s <= 0.0 {
        return c;
    }
    c.map(|v| v / s)
}

/// The profile matched against the keys: everything above the bass, plus the bass line weighted by [`BASS_WEIGHT`].
fn combine(bass: &[f64; 12], treble: &[f64; 12]) -> [f64; 12] {
    let mut c = [0.0; 12];
    for i in 0..12 {
        c[i] = treble[i] + BASS_WEIGHT * bass[i];
    }
    c
}

fn fold_chroma(hist: &[f64; FINE], tuning: f64) -> [f64; 12] {
    let mut c = [0.0; 12];
    for (i, &v) in hist.iter().enumerate() {
        let s = i as f64 / 10.0 - tuning;
        let pc = (s.round() as i64).rem_euclid(12) as usize;
        c[pc] += v;
    }
    c
}

fn pearson(a: &[f64; 12], b: &[f64; 12]) -> f64 {
    let ma = a.iter().sum::<f64>() / 12.0;
    let mb = b.iter().sum::<f64>() / 12.0;
    let (mut num, mut da, mut db) = (0.0, 0.0, 0.0);
    for i in 0..12 {
        let (x, y) = (a[i] - ma, b[i] - mb);
        num += x * y;
        da += x * x;
        db += y * y;
    }
    if da <= 0.0 || db <= 0.0 {
        0.0
    } else {
        num / (da * db).sqrt()
    }
}

fn rotate(profile: &[f64; 12], tonic: usize) -> [f64; 12] {
    let mut out = [0.0; 12];
    for (pc, slot) in out.iter_mut().enumerate() {
        *slot = profile[(pc + 12 - tonic) % 12];
    }
    out
}

/// A key profile as heard through harmonic-rich notes: each pitch class also
/// sounds its octaves, its fifth (3rd and 6th harmonics) and its major third
/// (5th harmonic), with amplitudes falling by [`HARMONIC_DECAY`] per harmonic.
fn with_harmonics(profile: &[f64; 12]) -> [f64; 12] {
    let s = HARMONIC_DECAY;
    let harmonics = [(0usize, 1.0), (0, s), (7, s.powi(2)), (0, s.powi(3)), (4, s.powi(4)), (7, s.powi(5))];
    let mut out = [0.0; 12];
    for (pc, &w) in profile.iter().enumerate() {
        for &(offset, a) in &harmonics {
            out[(pc + offset) % 12] += w * a;
        }
    }
    out
}

/// Fit of each of the 24 keys to a pitch-class profile (−1–1).
pub fn key_scores(chroma: &[f64; 12]) -> [f64; 24] {
    let major = with_harmonics(&KK_MAJOR);
    let minor = with_harmonics(&KK_MINOR);
    let mut s = [0.0; 24];
    for tonic in 0..12 {
        s[tonic] = pearson(chroma, &rotate(&major, tonic));
        s[12 + tonic] = pearson(chroma, &rotate(&minor, tonic));
    }
    s
}

fn best_two(scores: &[f64; 24]) -> (usize, usize) {
    let mut idx: Vec<usize> = (0..24).collect();
    idx.sort_by(|&a, &b| scores[b].partial_cmp(&scores[a]).unwrap_or(std::cmp::Ordering::Equal));
    (idx[0], idx[1])
}

// ------------------------------------------------------------ notations

/// How keys are written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Notation {
    /// C, D♭, …, Am, F♯m (the Camelot wheel's own spellings).
    Musical,
    /// C♯, D♯, F♯, G♯, A♯.
    Sharps,
    /// D♭, E♭, G♭, A♭, B♭.
    Flats,
    /// 1A–12A (minor), 1B–12B (major).
    Camelot,
    /// 1m–12m (minor), 1d–12d (major), as in Traktor.
    OpenKey,
}

impl Notation {
    pub fn from_id(id: &str) -> Option<Notation> {
        Some(match id {
            "musical" => Notation::Musical,
            "sharps" => Notation::Sharps,
            "flats" => Notation::Flats,
            "camelot" => Notation::Camelot,
            "openkey" => Notation::OpenKey,
            _ => return None,
        })
    }

    pub fn id(self) -> &'static str {
        match self {
            Notation::Musical => "musical",
            Notation::Sharps => "sharps",
            Notation::Flats => "flats",
            Notation::Camelot => "camelot",
            Notation::OpenKey => "openkey",
        }
    }
}

/// Spellings of the Camelot wheel (Mixed In Key): flats except F♯.
const MUSICAL_NAMES: [&str; 12] = ["C", "Db", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B"];
const SHARP_NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
const FLAT_NAMES: [&str; 12] = ["C", "Db", "D", "Eb", "E", "F", "Gb", "G", "Ab", "A", "Bb", "B"];

pub fn is_minor(key: u8) -> bool {
    key >= 12
}

pub fn tonic(key: u8) -> u8 {
    key % 12
}

/// Camelot number (1–12) and letter ('A' minor, 'B' major).
pub fn camelot(key: u8) -> (u8, char) {
    // Moving up a fifth moves one step round the wheel; C major is 8B and
    // every minor key shares its number with its relative major.
    let major_tonic = if is_minor(key) { (tonic(key) + 3) % 12 } else { tonic(key) };
    let n = ((major_tonic as u32 * 7) % 12 + 7) % 12 + 1;
    (n as u8, if is_minor(key) { 'A' } else { 'B' })
}

/// Key from a Camelot number (1–12) and letter.
pub fn from_camelot(number: u8, minor: bool) -> Option<u8> {
    if !(1..=12).contains(&number) {
        return None;
    }
    // Inverse of `camelot`: 7 is its own inverse modulo 12.
    let steps = (number as u32 + 12 - 8) % 12;
    let major_tonic = (steps * 7 % 12) as u8;
    Some(if minor { 12 + (major_tonic + 9) % 12 } else { major_tonic })
}

/// Formats a key. `ascii` uses `#`/`b` instead of `♯`/`♭` (for file tags).
pub fn format_key(key: u8, notation: Notation, ascii: bool) -> String {
    let key = key % 24;
    let (n, letter) = camelot(key);
    let name = |names: &[&str; 12]| {
        let base = names[tonic(key) as usize];
        let base = if ascii { base.to_string() } else { base.replace('#', "♯").replace('b', "♭") };
        if is_minor(key) {
            format!("{base}m")
        } else {
            base
        }
    };
    match notation {
        Notation::Camelot => format!("{n}{letter}"),
        Notation::OpenKey => {
            let o = (n as u32 + 12 - 8) % 12 + 1;
            format!("{o}{}", if is_minor(key) { 'm' } else { 'd' })
        }
        Notation::Musical => name(&MUSICAL_NAMES),
        Notation::Sharps => name(&SHARP_NAMES),
        Notation::Flats => name(&FLAT_NAMES),
    }
}

/// Long name, e.g. "A minor", "D♭ major".
pub fn long_name(key: u8) -> String {
    let base = MUSICAL_NAMES[tonic(key) as usize];
    format!("{} {}", base.replace('#', "♯").replace('b', "♭"), if is_minor(key) { "minor" } else { "major" })
}

/// Reads a key written in any common notation: "Am", "A min", "A minor",
/// "F#m", "G♭", "Dbmaj", "8A", "08b", "1m", "6d". Returns `None` for
/// anything else, including "o" (off key) and empty text.
pub fn parse_key(text: &str) -> Option<u8> {
    let t: String = text.trim().replace('♯', "#").replace('♭', "b").chars().filter(|c| !c.is_whitespace() && *c != '-' && *c != '_').collect();
    if t.is_empty() {
        return None;
    }
    let lower = t.to_ascii_lowercase();
    // Camelot (1A–12B) and Open Key (1m–12d).
    let digits: String = lower.chars().take_while(|c| c.is_ascii_digit()).collect();
    if !digits.is_empty() {
        let n: u8 = digits.parse().ok()?;
        let rest = &lower[digits.len()..];
        return match rest {
            "a" => from_camelot(n, true),
            "b" => from_camelot(n, false),
            "m" | "d" => {
                if !(1..=12).contains(&n) {
                    return None;
                }
                let camelot_n = (n + 6) % 12 + 1;
                from_camelot(camelot_n, rest == "m")
            }
            _ => None,
        };
    }
    let mut chars = t.chars();
    let letter = chars.next()?.to_ascii_uppercase();
    let base: i32 = match letter {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => return None,
    };
    let rest: String = chars.collect::<String>().to_ascii_lowercase();
    let mut rest = rest.as_str();
    let mut pc = base;
    // Accidentals come first; no mode word starts with "b".
    loop {
        if let Some(r) = rest.strip_prefix('#') {
            pc += 1;
            rest = r;
        } else if let Some(r) = rest.strip_prefix('b') {
            pc -= 1;
            rest = r;
        } else {
            break;
        }
    }
    let minor = match rest {
        "" | "maj" | "major" | "dur" => false,
        "m" | "min" | "minor" | "moll" => true,
        _ => return None,
    };
    let pc = pc.rem_euclid(12) as u8;
    Some(if minor { 12 + pc } else { pc })
}

/// Steps between two keys on the Camelot wheel: 0 same key, 1 for a
/// neighbour (±1 number, same letter) or the relative key (same number).
pub fn camelot_distance(a: u8, b: u8) -> u8 {
    let (na, la) = camelot(a);
    let (nb, lb) = camelot(b);
    let d = (na as i32 - nb as i32).rem_euclid(12);
    let around = d.min(12 - d) as u8;
    around + u8::from(la != lb)
}

/// Keys that mix harmonically with `key`: the key itself, its relative and
/// its neighbours on the Camelot wheel.
pub fn compatible_keys(key: u8) -> Vec<u8> {
    let (n, l) = camelot(key);
    let minor = l == 'A';
    let up = n % 12 + 1;
    let down = (n + 10) % 12 + 1;
    [from_camelot(n, minor), from_camelot(n, !minor), from_camelot(up, minor), from_camelot(down, minor)].into_iter().flatten().collect()
}

/// Wheel order for sorting: by Camelot number, minor (A) before major (B).
pub fn camelot_order(key: u8) -> u8 {
    let (n, l) = camelot(key);
    n * 2 + u8::from(l == 'B')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camelot_wheel() {
        let parse = |s: &str| parse_key(s).unwrap_or_else(|| panic!("cannot parse {s}"));
        assert_eq!(camelot(parse("C")), (8, 'B'));
        assert_eq!(camelot(parse("Am")), (8, 'A'));
        assert_eq!(camelot(parse("G")), (9, 'B'));
        assert_eq!(camelot(parse("B")), (1, 'B'));
        assert_eq!(camelot(parse("Abm")), (1, 'A'));
        assert_eq!(camelot(parse("F#")), (2, 'B'));
        assert_eq!(camelot(parse("Dbm")), (12, 'A'));
        assert_eq!(camelot(parse("E")), (12, 'B'));
        for k in 0..24u8 {
            let (n, l) = camelot(k);
            assert_eq!(from_camelot(n, l == 'A'), Some(k), "round trip {k}");
            for notation in [Notation::Musical, Notation::Sharps, Notation::Flats, Notation::Camelot, Notation::OpenKey] {
                for ascii in [true, false] {
                    let s = format_key(k, notation, ascii);
                    assert_eq!(parse_key(&s), Some(k), "{s} ({notation:?})");
                }
            }
        }
    }

    #[test]
    fn notations() {
        let am = parse_key("Am").unwrap();
        assert_eq!(format_key(am, Notation::Camelot, true), "8A");
        assert_eq!(format_key(am, Notation::OpenKey, true), "1m");
        assert_eq!(format_key(parse_key("C").unwrap(), Notation::OpenKey, true), "1d");
        assert_eq!(format_key(parse_key("E").unwrap(), Notation::OpenKey, true), "5d");
        assert_eq!(format_key(parse_key("8B").unwrap(), Notation::Musical, false), "C");
        assert_eq!(format_key(parse_key("1A").unwrap(), Notation::Musical, false), "A♭m");
        assert_eq!(format_key(parse_key("1A").unwrap(), Notation::Sharps, true), "G#m");
        assert_eq!(format_key(parse_key("2B").unwrap(), Notation::Flats, false), "G♭");
        assert_eq!(long_name(parse_key("Bbm").unwrap()), "B♭ minor");
        for (text, expect) in [
            ("A min", "8A"),
            ("a minor", "8A"),
            ("F♯m", "11A"),
            ("Gb", "2B"),
            ("Dbmaj", "3B"),
            ("08b", "8B"),
            ("12a", "12A"),
            ("6d", "1B"),
            ("Ebm", "2A"),
            ("Bb", "6B"),
            ("bbm", "3A"),
            ("Cb", "1B"),
        ] {
            assert_eq!(parse_key(text).map(|k| format_key(k, Notation::Camelot, true)).as_deref(), Some(expect), "{text}");
        }
        for bad in ["", "o", "13A", "0B", "H", "Am7", "8C", "x"] {
            assert_eq!(parse_key(bad), None, "{bad}");
        }
    }

    #[test]
    fn harmonic_neighbours() {
        let am = parse_key("8A").unwrap();
        let mut names: Vec<String> = compatible_keys(am).into_iter().map(|k| format_key(k, Notation::Camelot, true)).collect();
        names.sort();
        assert_eq!(names, ["7A", "8A", "8B", "9A"]);
        assert_eq!(camelot_distance(parse_key("12A").unwrap(), parse_key("1A").unwrap()), 1);
        assert_eq!(camelot_distance(parse_key("8A").unwrap(), parse_key("8B").unwrap()), 1);
        assert_eq!(camelot_distance(parse_key("8A").unwrap(), parse_key("2B").unwrap()), 7);
        assert!(camelot_order(parse_key("1A").unwrap()) < camelot_order(parse_key("1B").unwrap()));
        assert!(camelot_order(parse_key("1B").unwrap()) < camelot_order(parse_key("2A").unwrap()));
    }

    #[test]
    fn profile_fit_finds_the_key_of_a_scale() {
        // Pitch classes of a natural minor scale on D, tonic emphasised.
        let mut c = [0.0; 12];
        for (i, pc) in [2usize, 4, 5, 7, 9, 10, 0].iter().enumerate() {
            c[*pc] = if i == 0 { 3.0 } else if i == 2 || i == 4 { 2.0 } else { 1.0 };
        }
        let s = key_scores(&c);
        let (best, _) = best_two(&s);
        assert_eq!(format_key(best as u8, Notation::Musical, true), "Dm");
    }
}
