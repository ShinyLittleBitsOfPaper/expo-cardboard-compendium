//! Perceptual hashing of card art (docs/phash-design.md, step P1).
//!
//! Everything below the decoder is a pure function over an 8-bit grayscale buffer, with a hand-written area
//! downscale, so the compiler and (later) the on-device engine produce identical bits from identical pixels.
//! Two 64-bit hashes: `dhash` (9x8 horizontal gradient) and `phash` (32x32 DCT, low 8x8 minus DC, median split).
use image::{codecs::jpeg::JpegEncoder, ExtendedColorType, ImageReader};
use std::path::Path;

/// 8-bit grayscale image, row-major.
#[derive(Clone)]
pub struct Gray {
    pub w: usize,
    pub h: usize,
    pub px: Vec<u8>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Hashes {
    pub dhash: u64,
    pub phash: u64,
}

pub fn load_gray(path: &Path) -> Result<Gray, String> {
    let img = ImageReader::open(path).map_err(|e| e.to_string())?.with_guessed_format().map_err(|e| e.to_string())?
        .decode().map_err(|e| format!("{}: {e}", path.display()))?;
    let g = img.to_luma8();
    Ok(Gray { w: g.width() as usize, h: g.height() as usize, px: g.into_raw() })
}

/// Area-average downscale to `ow` x `oh` (exact fractional coverage; deterministic f32 math).
pub fn area_resize(g: &Gray, ow: usize, oh: usize) -> Vec<f32> {
    let mut out = vec![0f32; ow * oh];
    let (sx, sy) = (g.w as f32 / ow as f32, g.h as f32 / oh as f32);
    for oy in 0..oh {
        let (y0, y1) = (oy as f32 * sy, (oy + 1) as f32 * sy);
        for ox in 0..ow {
            let (x0, x1) = (ox as f32 * sx, (ox + 1) as f32 * sx);
            let (mut acc, mut wsum) = (0f32, 0f32);
            let mut y = y0.floor() as usize;
            while (y as f32) < y1 && y < g.h {
                let wy = ((y + 1) as f32).min(y1) - (y as f32).max(y0);
                let mut x = x0.floor() as usize;
                while (x as f32) < x1 && x < g.w {
                    let wx = ((x + 1) as f32).min(x1) - (x as f32).max(x0);
                    acc += g.px[y * g.w + x] as f32 * wx * wy;
                    wsum += wx * wy;
                    x += 1;
                }
                y += 1;
            }
            out[oy * ow + ox] = if wsum > 0.0 { acc / wsum } else { 0.0 };
        }
    }
    out
}

pub fn dhash(g: &Gray) -> u64 {
    let s = area_resize(g, 9, 8);
    let mut bits = 0u64;
    for y in 0..8 {
        for x in 0..8 {
            bits = (bits << 1) | (s[y * 9 + x] < s[y * 9 + x + 1]) as u64;
        }
    }
    bits
}

pub fn phash(g: &Gray) -> u64 {
    const N: usize = 32;
    let s = area_resize(g, N, N);
    // cos table; only the low 8 frequencies are needed in each axis
    let mut cos = [[0f32; N]; 8];
    for (k, row) in cos.iter_mut().enumerate() {
        for (n, c) in row.iter_mut().enumerate() {
            *c = ((std::f32::consts::PI / N as f32) * (n as f32 + 0.5) * k as f32).cos();
        }
    }
    let mut rows = [[0f32; 8]; N]; // DCT along x for each row
    for y in 0..N {
        for k in 0..8 {
            rows[y][k] = (0..N).map(|x| s[y * N + x] * cos[k][x]).sum();
        }
    }
    let mut coef = [0f32; 64];
    for ky in 0..8 {
        for kx in 0..8 {
            coef[ky * 8 + kx] = (0..N).map(|y| rows[y][kx] * cos[ky][y]).sum();
        }
    }
    let mut sorted: Vec<f32> = coef[1..].to_vec(); // drop DC
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = sorted[sorted.len() / 2];
    let mut bits = 0u64;
    for c in coef.iter() {
        bits = (bits << 1) | (*c > median) as u64;
    }
    bits
}

pub fn hash(g: &Gray) -> Hashes {
    Hashes { dhash: dhash(g), phash: phash(g) }
}

/// The art box of a straightened, tightly cropped modern-frame card image (x 8.3-91.7%, y 11.3-55%; matches Scryfall's art_crop
/// closely on standard frames, see docs/phash-design.md). Old-border, full-art and borderless cards sit a little differently, so
/// the +-3% query shifts matter. Returns the whole image if it is too small to cut.
pub fn card_art_region(card: &Gray) -> Gray {
    let (x0, y0) = ((card.w as f32 * 0.083).round() as usize, (card.h as f32 * 0.113).round() as usize);
    let (x1, y1) = ((card.w as f32 * 0.917).round() as usize, (card.h as f32 * 0.55).round() as usize);
    if x1 <= x0 + 8 || y1 <= y0 + 8 || x1 > card.w || y1 > card.h {
        return card.clone();
    }
    let mut px = Vec::with_capacity((x1 - x0) * (y1 - y0));
    for y in y0..y1 {
        px.extend_from_slice(&card.px[y * card.w + x0..y * card.w + x1]);
    }
    Gray { w: x1 - x0, h: y1 - y0, px }
}

/// The stored/queried art hash: pHash of the inner 80% of the art (docs/phash-design.md, P1b: best variant on real art).
pub fn art_phash(g: &Gray) -> u64 {
    phash(&window(g, 0.8, 0.0, 0.0))
}

/// Photo-side hashes: the same inner-80% pHash of a 94% window at 3x3 shifts of +-3% (compare the best distance).
/// The stored hash is `art_phash` of the reference art; shifts only exist on the query side.
pub fn query_hashes(g: &Gray) -> Vec<u64> {
    GRID.iter().map(|&(ox, oy)| art_phash(&window(g, 0.94, ox, oy))).collect()
}

pub fn hamming(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}

// ---- perturbations (what a phone photo of a card does to the art, roughly) ----

#[derive(Clone, Copy, Debug)]
pub enum Perturb {
    Blur,
    BrightUp,
    BrightDown,
    ContrastUp,
    ContrastDown,
    RotatePlus2,
    RotateMinus2,
    CropJitter,
    CropJitterNeg,
    Jpeg40,
    Perspective,
}

pub const ALL_PERTURBS: [Perturb; 11] = [
    Perturb::Blur, Perturb::BrightUp, Perturb::BrightDown, Perturb::ContrastUp, Perturb::ContrastDown,
    Perturb::RotatePlus2, Perturb::RotateMinus2, Perturb::CropJitter, Perturb::CropJitterNeg, Perturb::Jpeg40, Perturb::Perspective,
];

fn sample(g: &Gray, x: f32, y: f32) -> f32 {
    let (x, y) = (x.clamp(0.0, g.w as f32 - 1.0), y.clamp(0.0, g.h as f32 - 1.0));
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(g.w - 1), (y0 + 1).min(g.h - 1));
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let p = |xx: usize, yy: usize| g.px[yy * g.w + xx] as f32;
    (p(x0, y0) * (1.0 - fx) + p(x1, y0) * fx) * (1.0 - fy) + (p(x0, y1) * (1.0 - fx) + p(x1, y1) * fx) * fy
}

fn map_px(g: &Gray, f: impl Fn(usize, usize) -> (f32, f32)) -> Gray {
    let mut px = vec![0u8; g.w * g.h];
    for y in 0..g.h {
        for x in 0..g.w {
            let (sx, sy) = f(x, y);
            px[y * g.w + x] = sample(g, sx, sy).round().clamp(0.0, 255.0) as u8;
        }
    }
    Gray { w: g.w, h: g.h, px }
}

fn rotate(g: &Gray, deg: f32) -> Gray {
    let (c, s) = (deg.to_radians().cos(), deg.to_radians().sin());
    let (cx, cy) = (g.w as f32 / 2.0, g.h as f32 / 2.0);
    map_px(g, |x, y| {
        let (dx, dy) = (x as f32 - cx, y as f32 - cy);
        (cx + c * dx + s * dy, cy - s * dx + c * dy)
    })
}

fn blur(g: &Gray) -> Gray {
    let r = (g.w.max(g.h) / 150).max(1) as isize; // ~ a 1-2 px defocus at 300 px
    let mut px = vec![0u8; g.w * g.h];
    for y in 0..g.h as isize {
        for x in 0..g.w as isize {
            let (mut a, mut n) = (0u32, 0u32);
            for dy in -r..=r {
                for dx in -r..=r {
                    let (xx, yy) = ((x + dx).clamp(0, g.w as isize - 1), (y + dy).clamp(0, g.h as isize - 1));
                    a += g.px[yy as usize * g.w + xx as usize] as u32;
                    n += 1;
                }
            }
            px[y as usize * g.w + x as usize] = (a / n) as u8;
        }
    }
    Gray { w: g.w, h: g.h, px }
}

fn tone(g: &Gray, gain: f32, bias: f32) -> Gray {
    Gray { w: g.w, h: g.h, px: g.px.iter().map(|&p| ((p as f32 - 128.0) * gain + 128.0 + bias).round().clamp(0.0, 255.0) as u8).collect() }
}

fn crop_jitter(g: &Gray) -> Gray {
    let (mx, my) = (g.w as f32 * 0.03, g.h as f32 * 0.03);
    let (w, h) = (g.w as f32 - 2.0 * mx, g.h as f32 - 2.0 * my);
    // crop 3% off the left/top, 3% more than the right/bottom: an off-centre crop, scaled back to full size
    map_px(g, |x, y| (mx * 2.0 + x as f32 / g.w as f32 * w, my * 2.0 + y as f32 / g.h as f32 * h))
}

fn crop_jitter_neg(g: &Gray) -> Gray {
    // the mirror image: 6% cut from the right/bottom instead
    let (mx, my) = (g.w as f32 * 0.03, g.h as f32 * 0.03);
    let (w, h) = (g.w as f32 - 2.0 * mx, g.h as f32 - 2.0 * my);
    map_px(g, |x, y| (x as f32 / g.w as f32 * w, y as f32 / g.h as f32 * h))
}

fn perspective(g: &Gray) -> Gray {
    // shrink the right edge by 4% top and bottom (a card photographed slightly off-axis)
    let k = 0.04;
    map_px(g, |x, y| {
        let u = x as f32 / g.w as f32;
        let squeeze = 1.0 + k * u; // >1 near the right edge
        let cy = g.h as f32 / 2.0;
        (x as f32, cy + (y as f32 - cy) * squeeze)
    })
}

fn jpeg(g: &Gray, quality: u8) -> Gray {
    let mut buf = Vec::new();
    JpegEncoder::new_with_quality(&mut buf, quality)
        .encode(&g.px, g.w as u32, g.h as u32, ExtendedColorType::L8).expect("encode");
    let img = image::load_from_memory_with_format(&buf, image::ImageFormat::Jpeg).expect("decode").to_luma8();
    Gray { w: img.width() as usize, h: img.height() as usize, px: img.into_raw() }
}

pub fn perturb(g: &Gray, p: Perturb) -> Gray {
    match p {
        Perturb::Blur => blur(g),
        Perturb::BrightUp => tone(g, 1.0, 51.0),
        Perturb::BrightDown => tone(g, 1.0, -51.0),
        Perturb::ContrastUp => tone(g, 1.2, 0.0),
        Perturb::ContrastDown => tone(g, 0.8, 0.0),
        Perturb::RotatePlus2 => rotate(g, 2.0),
        Perturb::RotateMinus2 => rotate(g, -2.0),
        Perturb::CropJitter => crop_jitter(g),
        Perturb::CropJitterNeg => crop_jitter_neg(g),
        Perturb::Jpeg40 => jpeg(g, 40),
        Perturb::Perspective => perspective(g),
    }
}

/// Result of `evaluate`: per algorithm, hits[t] = fraction of perturbed hashes within distance t of the clean hash
/// (t = 0..=16); `nearest_other[i]` = distance from image i's clean hash to the closest other image's clean hash.
pub struct AlgoReport {
    pub name: &'static str,
    pub hits: [f64; 17],
    pub per_perturb: Vec<(Perturb, f64)>, // mean distance
    pub nearest_other: Vec<u32>,
}

pub fn evaluate(images: &[Gray]) -> Vec<AlgoReport> {
    let clean: Vec<Hashes> = images.iter().map(hash).collect();
    let mut out = Vec::new();
    for (name, pick) in [("dhash", (|h: &Hashes| h.dhash) as fn(&Hashes) -> u64), ("phash", |h: &Hashes| h.phash)] {
        let mut counts = [0usize; 17];
        let mut total = 0usize;
        let mut sums = vec![0f64; ALL_PERTURBS.len()];
        for (i, g) in images.iter().enumerate() {
            for (pi, p) in ALL_PERTURBS.iter().enumerate() {
                let d = hamming(pick(&clean[i]), pick(&hash(&perturb(g, *p))));
                sums[pi] += d as f64;
                total += 1;
                for t in d as usize..=16 {
                    counts[t] += 1;
                }
            }
        }
        let mut hits = [0f64; 17];
        for t in 0..17 {
            hits[t] = counts[t] as f64 / total.max(1) as f64;
        }
        let nearest_other = (0..images.len())
            .map(|i| (0..images.len()).filter(|&j| j != i).map(|j| hamming(pick(&clean[i]), pick(&clean[j]))).min().unwrap_or(64))
            .collect();
        let per_perturb = ALL_PERTURBS.iter().enumerate().map(|(pi, p)| (*p, sums[pi] / images.len().max(1) as f64)).collect();
        out.push(AlgoReport { name, hits, per_perturb, nearest_other });
    }
    out
}

// ---- matching variants (P1b): how to hash the reference and the photo so crop misalignment hurts less ----

/// Window of `keep` x `keep` (fractions of the image) centred, then moved by (ox, oy) image fractions.
pub fn window(g: &Gray, keep: f32, ox: f32, oy: f32) -> Gray {
    let (cw, ch) = ((g.w as f32 * keep).round().max(8.0) as usize, (g.h as f32 * keep).round().max(8.0) as usize);
    let x0 = (((g.w as f32 - cw as f32) / 2.0 + ox * g.w as f32).round().max(0.0) as usize).min(g.w - cw.min(g.w));
    let y0 = (((g.h as f32 - ch as f32) / 2.0 + oy * g.h as f32).round().max(0.0) as usize).min(g.h - ch.min(g.h));
    let (cw, ch) = (cw.min(g.w), ch.min(g.h));
    let mut px = Vec::with_capacity(cw * ch);
    for y in y0..y0 + ch {
        px.extend_from_slice(&g.px[y * g.w + x0..y * g.w + x0 + cw]);
    }
    Gray { w: cw, h: ch, px }
}

/// A way to match: the reference is hashed once; the photo is hashed at every offset and the best (smallest) distance wins.
pub struct Variant {
    pub name: &'static str,
    pub ref_keep: f32,
    pub query_keep: f32,
    pub offsets: &'static [(f32, f32)],
}

const GRID: [(f32, f32); 9] = [
    (-0.03, -0.03), (0.0, -0.03), (0.03, -0.03), (-0.03, 0.0), (0.0, 0.0), (0.03, 0.0), (-0.03, 0.03), (0.0, 0.03), (0.03, 0.03),
];

pub const VARIANTS: [Variant; 4] = [
    Variant { name: "full (baseline)", ref_keep: 1.0, query_keep: 1.0, offsets: &[(0.0, 0.0)] },
    Variant { name: "inner 80%", ref_keep: 0.8, query_keep: 0.8, offsets: &[(0.0, 0.0)] },
    Variant { name: "3x3 shifts (+-3%)", ref_keep: 1.0, query_keep: 0.94, offsets: &GRID },
    Variant { name: "inner 80% + 3x3 shifts", ref_keep: 0.8, query_keep: 0.8, offsets: &GRID },
];

pub struct VariantReport {
    pub variant: &'static str,
    pub algo: &'static str,
    pub hits: [f64; 17],
    pub per_perturb: Vec<(Perturb, f64)>,
    pub nearest_other: Vec<u32>,
}

pub fn evaluate_variants(images: &[Gray]) -> Vec<VariantReport> {
    evaluate_variants_progress(images, &mut |_, _| {})
}

/// Same as `evaluate_variants`, calling `progress(done, total)` after each image. Everything that does not depend on
/// the pair being compared is computed once: reference hashes, clean photo hashes per offset (so the false-match
/// matrix is popcounts only), and each perturbed image (built once per image, hashed for every variant).
pub fn evaluate_variants_progress(images: &[Gray], progress: &mut dyn FnMut(usize, usize)) -> Vec<VariantReport> {
    let algos: [(&'static str, fn(&Gray) -> u64); 2] = [("dhash", dhash), ("phash", phash)];
    let n = images.len();
    // refs[k][i], clean[k][i][o] with k = algo * VARIANTS.len() + variant
    let mut refs: Vec<Vec<u64>> = Vec::new();
    let mut clean: Vec<Vec<Vec<u64>>> = Vec::new();
    for (_, pick) in algos.iter() {
        for v in VARIANTS.iter() {
            refs.push(images.iter().map(|g| pick(&window(g, v.ref_keep, 0.0, 0.0))).collect());
            clean.push(images.iter().map(|g| v.offsets.iter().map(|&(ox, oy)| pick(&window(g, v.query_keep, ox, oy))).collect()).collect());
        }
    }
    let kcount = refs.len();
    let mut counts = vec![[0usize; 17]; kcount];
    let mut sums = vec![vec![0f64; ALL_PERTURBS.len()]; kcount];
    let mut total = vec![0usize; kcount];
    for (i, g) in images.iter().enumerate() {
        for (pi, p) in ALL_PERTURBS.iter().enumerate() {
            let pg = perturb(g, *p);
            for (a, (_, pick)) in algos.iter().enumerate() {
                for (vi, v) in VARIANTS.iter().enumerate() {
                    let k = a * VARIANTS.len() + vi;
                    let d = v.offsets.iter().map(|&(ox, oy)| hamming(refs[k][i], pick(&window(&pg, v.query_keep, ox, oy)))).min().unwrap();
                    sums[k][pi] += d as f64;
                    total[k] += 1;
                    for t in d as usize..=16 {
                        counts[k][t] += 1;
                    }
                }
            }
        }
        progress(i + 1, n);
    }
    let mut out = Vec::new();
    for (a, (algo, _)) in algos.iter().enumerate() {
        for (vi, v) in VARIANTS.iter().enumerate() {
            let k = a * VARIANTS.len() + vi;
            let mut hits = [0f64; 17];
            for t in 0..17 {
                hits[t] = counts[k][t] as f64 / total[k].max(1) as f64;
            }
            let nearest_other = (0..n)
                .map(|i| (0..n).filter(|&j| j != i).map(|j| clean[k][i].iter().map(|&q| hamming(refs[k][j], q)).min().unwrap()).min().unwrap_or(64))
                .collect();
            let per_perturb = ALL_PERTURBS.iter().enumerate().map(|(pi, p)| (*p, sums[k][pi] / n.max(1) as f64)).collect();
            out.push(VariantReport { variant: v.name, algo, hits, per_perturb, nearest_other });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Multi-octave value noise: structure at several scales, like real art (smooth sinusoids have almost no
    /// high-frequency energy, which makes pHash's low-order bits flip on noise and dHash collapse across seeds).
    fn synth(seed: u32, w: usize, h: usize) -> Gray {
        let lat = |ix: i32, iy: i32, o: u32| -> f32 {
            let mut v = (ix as u32).wrapping_mul(374761393) ^ (iy as u32).wrapping_mul(668265263) ^ seed.wrapping_mul(2246822519) ^ o.wrapping_mul(3266489917);
            v = (v ^ (v >> 13)).wrapping_mul(1274126177);
            ((v ^ (v >> 16)) % 10000) as f32 / 10000.0
        };
        let mut px = vec![0u8; w * h];
        for y in 0..h {
            for x in 0..w {
                let mut val = 0.0;
                let mut amp = 1.0;
                let mut norm = 0.0;
                for o in 0..4u32 {
                    let freq = 3.0 * (1 << o) as f32;
                    let (fx, fy) = (x as f32 / w as f32 * freq, y as f32 / h as f32 * freq);
                    let (ix, iy) = (fx.floor() as i32, fy.floor() as i32);
                    let (tx, ty) = (fx - ix as f32, fy - iy as f32);
                    let (tx, ty) = (tx * tx * (3.0 - 2.0 * tx), ty * ty * (3.0 - 2.0 * ty));
                    let a = lat(ix, iy, o) * (1.0 - tx) + lat(ix + 1, iy, o) * tx;
                    let b = lat(ix, iy + 1, o) * (1.0 - tx) + lat(ix + 1, iy + 1, o) * tx;
                    val += amp * (a * (1.0 - ty) + b * ty);
                    norm += amp;
                    amp *= 0.6;
                }
                px[y * w + x] = (40.0 + 170.0 * val / norm) as u8;
            }
        }
        Gray { w, h, px }
    }

    #[test]
    fn deterministic_and_self_distance_zero() {
        let g = synth(1, 200, 150);
        assert_eq!(hash(&g), hash(&g.clone()));
        assert_eq!(hamming(hash(&g).phash, hash(&g).phash), 0);
    }

    #[test]
    fn different_images_are_far_and_perturbed_are_near() {
        let imgs: Vec<Gray> = (1..=12).map(|s| synth(s, 220, 160)).collect();
        let rep = evaluate(&imgs);
        for r in &rep {
            assert!(r.hits[16] > 0.8, "{} hit@16 {}", r.name, r.hits[16]);
            let mean_other = r.nearest_other.iter().sum::<u32>() as f64 / imgs.len() as f64;
            assert!(mean_other > 8.0, "{} nearest-other {}", r.name, mean_other);
        }
    }

    #[test]
    fn shift_grid_recovers_crop_jitter() {
        let imgs: Vec<Gray> = (1..=8).map(|s| synth(s, 220, 160)).collect();
        let rep = evaluate_variants(&imgs);
        let mean = |algo: &str, var: &str| {
            let r = rep.iter().find(|r| r.algo == algo && r.variant == var).unwrap();
            r.per_perturb.iter().filter(|(p, _)| matches!(p, Perturb::CropJitter | Perturb::CropJitterNeg)).map(|(_, d)| d).sum::<f64>() / 2.0
        };
        assert!(mean("phash", "3x3 shifts (+-3%)") < mean("phash", "full (baseline)"));
    }

    #[test]
    fn card_art_region_dims() {
        let g = Gray { w: 1000, h: 1400, px: vec![0; 1_400_000] };
        let r = card_art_region(&g);
        assert_eq!((r.w, r.h), (834, 612));
        assert_eq!(r.px.len(), 834 * 612);
    }

    #[test]
    fn window_bounds() {
        let g = Gray { w: 100, h: 80, px: vec![7; 8000] };
        let w = window(&g, 0.94, 0.03, -0.03);
        assert_eq!((w.w, w.h), (94, 75));
        assert_eq!(w.px.len(), 94 * 75);
    }

    #[test]
    fn area_resize_exact_on_uniform() {
        let g = Gray { w: 7, h: 5, px: vec![100; 35] };
        assert!(area_resize(&g, 3, 2).iter().all(|v| (v - 100.0).abs() < 1e-4));
    }

    #[test]
    fn brightness_shift_keeps_dhash() {
        let g = synth(3, 200, 150);
        assert!(hamming(dhash(&g), dhash(&perturb(&g, Perturb::BrightUp))) <= 4);
    }
}
