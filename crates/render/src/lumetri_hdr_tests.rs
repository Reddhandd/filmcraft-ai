//! Lumetri in HDR working spaces (known cd/m² values through PQ and HLG) and HSL Secondary ▸
//! Refine (Denoise, Blur of the key).

use filmcraft_color::{GradeSpace, REFERENCE_WHITE_NITS, WorkingSpace};
use filmcraft_project::{EffectInstance, ParamValue, find_effect};
use filmcraft_time::Tick;

use crate::Image;
use crate::effects::{FxCtx, apply};

const RW: f32 = REFERENCE_WHITE_NITS as f32;

fn cx(working: WorkingSpace) -> FxCtx<'static> {
    FxCtx { t: Tick::ZERO, px_scale: 1.0, seconds: 0.0, timecode: "", clip_name: "", project: None, env: None, frame_rate: 24.0, working }
}

fn lumetri(params: &[(&str, ParamValue)]) -> EffectInstance {
    let mut e = find_effect("lumetri").unwrap().instance();
    for (k, v) in params {
        e.params.get_mut(*k).unwrap_or_else(|| panic!("no param {k}")).value = v.clone();
    }
    e
}

/// A row of grey pixels at these cd/m², graded; returns their cd/m² afterwards.
fn grade(working: WorkingSpace, nits: &[f32], e: &EffectInstance) -> Vec<f32> {
    let mut img = Image::new(nits.len(), 1);
    for (i, n) in nits.iter().enumerate() {
        img.px[i * 4..i * 4 + 4].copy_from_slice(&[n / RW, n / RW, n / RW, 1.0]);
    }
    apply(&mut img, e, &cx(working));
    (0..nits.len()).map(|i| img.px[i * 4 + 1] * RW).collect()
}

fn close(got: &[f32], want: &[f32], rel: f32) {
    for (g, w) in got.iter().zip(want) {
        assert!((g - w).abs() <= w.abs() * rel + 0.02, "got {got:?}, want {want:?}");
    }
}

const F: fn(f64) -> ParamValue = ParamValue::Float;
const LEVELS: [f32; 6] = [1.0, 26.0, 100.0, 203.0, 1000.0, 4000.0];

#[test]
fn identity_keeps_hdr_highlights_in_pq_and_hlg() {
    let e = lumetri(&[]);
    close(&grade(WorkingSpace::Rec2100Pq, &LEVELS, &e), &LEVELS, 2e-3);
    // HLG signal tops out around 12× its peak's scene light: 4000 cd/m² is still inside
    close(&grade(WorkingSpace::Rec2100Hlg, &LEVELS, &e), &LEVELS, 2e-3);
    // the same grade in a Rec. 709 sequence clips at SDR white (1.0 = 203 cd/m² here)
    let sdr = grade(WorkingSpace::Rec709, &[1000.0], &e);
    assert!((sdr[0] - RW).abs() < 0.5, "{sdr:?}");
}

#[test]
fn exposure_is_stops_of_light_in_hdr() {
    let e = lumetri(&[("exposure", F(1.0))]);
    let want: Vec<f32> = LEVELS.iter().map(|n| n * 2.0).collect();
    close(&grade(WorkingSpace::Rec2100Pq, &LEVELS, &e), &want, 3e-3);
    close(&grade(WorkingSpace::Rec2100Hlg, &LEVELS[..5], &e), &want[..5], 3e-3);
    let e = lumetri(&[("exposure", F(-2.0))]);
    close(&grade(WorkingSpace::Rec2100Pq, &[400.0, 4000.0], &e), &[100.0, 1000.0], 3e-3);
}

#[test]
fn whites_and_contrast_scale_with_hdr_white() {
    // Whites +100 stretches the signal so 85 % of HDR White becomes HDR White
    for white in [1000.0f32, 4000.0] {
        let gs = GradeSpace::new(WorkingSpace::Rec2100Pq, white);
        let e = lumetri(&[("whites", F(100.0)), ("hdr_white", F(white as f64))]);
        let src = gs.nits_of_signal(0.85);
        let got = grade(WorkingSpace::Rec2100Pq, &[0.0, src], &e);
        assert!(got[0].abs() < 1e-3, "black stays black");
        assert!((got[1] - white).abs() < white * 3e-3, "HDR White {white}: {src} cd/m² → {} (want {white})", got[1]);
    }
    // Contrast pivots on the middle of the HDR White range and leaves speculars above it alone
    let gs = GradeSpace::new(WorkingSpace::Rec2100Pq, 1000.0);
    let (mid, dark, bright) = (gs.nits_of_signal(0.5), gs.nits_of_signal(0.3), gs.nits_of_signal(0.7));
    let e = lumetri(&[("contrast", F(60.0))]);
    let got = grade(WorkingSpace::Rec2100Pq, &[mid, dark, bright, 4000.0], &e);
    assert!((got[0] - mid).abs() < mid * 3e-3, "pivot {mid} → {}", got[0]);
    assert!(got[1] < dark && got[2] > bright, "{got:?}");
    assert!((got[3] - 4000.0).abs() < 4000.0 * 3e-3, "specular kept: {}", got[3]);
    // the same in HLG: reference white is 75 % of a 1000 cd/m² display's signal
    let hlg = GradeSpace::new(WorkingSpace::Rec2100Hlg, 1000.0);
    assert!((hlg.signal_of_nits(203.0) - 0.75).abs() < 2e-3);
    let e = lumetri(&[("whites", F(100.0))]);
    let got = grade(WorkingSpace::Rec2100Hlg, &[hlg.nits_of_signal(0.85)], &e);
    assert!((got[0] - 1000.0).abs() < 4.0, "{got:?}");
}

#[test]
fn hdr_specular_sets_highlights_above_hdr_white() {
    let e = lumetri(&[("hdr_specular", F(-100.0))]);
    let got = grade(WorkingSpace::Rec2100Pq, &[203.0, 1000.0, 4000.0], &e);
    close(&got, &[203.0, 1000.0, 1000.0], 3e-3);
    let gs = GradeSpace::new(WorkingSpace::Rec2100Pq, 1000.0);
    let e = lumetri(&[("hdr_specular", F(50.0))]);
    let got = grade(WorkingSpace::Rec2100Pq, &[1000.0, 4000.0], &e);
    let want = gs.nits_of_signal(1.0 + (gs.signal_of_nits(4000.0) - 1.0) * 1.5);
    close(&got, &[1000.0, want], 3e-3);
    // no effect in SDR (the slider is HDR-only)
    let sdr = grade(WorkingSpace::Rec709, &[150.0], &lumetri(&[("hdr_specular", F(-100.0))]));
    assert!((sdr[0] - 150.0).abs() < 0.2);
}

#[test]
fn curves_span_the_hdr_range() {
    // a luma curve lifting the middle: 0.5 → 0.6 of the HDR Range
    let curve = ParamValue::Curve(vec![[0.0, 0.0], [0.5, 0.6], [1.0, 1.0]]);
    for range in [1000.0f32, 2000.0] {
        let gs = GradeSpace::new(WorkingSpace::Rec2100Pq, range);
        let e = lumetri(&[("curve_luma", curve.clone()), ("curves_hdr_range", F(range as f64))]);
        let got = grade(WorkingSpace::Rec2100Pq, &[gs.nits_of_signal(0.5), range, 6000.0], &e);
        close(&got, &[gs.nits_of_signal(0.6), range, 6000.0], 4e-3);
    }
}

/// A 40×40 image: grey background, a red 16×16 square, and isolated red speckles.
fn keyed_image() -> Image {
    let mut img = Image::filled(40, 40, [0.2, 0.2, 0.2, 1.0]);
    let red = [0.8, 0.02, 0.02, 1.0];
    for y in 12..28 {
        for x in 12..28 {
            img.px[(y * 40 + x) * 4..][..4].copy_from_slice(&red);
        }
    }
    for (x, y) in [(3, 3), (35, 6), (5, 33), (33, 34), (20, 4)] {
        img.px[(y * 40 + x) * 4..][..4].copy_from_slice(&red);
    }
    img
}

fn key(denoise: f64, blur: f64) -> Image {
    let mut img = keyed_image();
    let e = lumetri(&[
        ("hsl_on", ParamValue::Bool(true)),
        ("hsl_hue", F(0.0)),
        ("hsl_hue_range", F(40.0)),
        ("hsl_show_mask", ParamValue::Choice(3)),
        ("hsl_denoise", F(denoise)),
        ("hsl_blur", F(blur)),
    ]);
    apply(&mut img, &e, &cx(WorkingSpace::Rec709));
    img
}

#[test]
fn hsl_refine_denoise_removes_speckles() {
    let m = |img: &Image, x: usize, y: usize| filmcraft_color::linear_to_srgb(img.get(x, y)[1]);
    let raw = key(0.0, 0.0);
    assert!(m(&raw, 3, 3) > 0.9 && m(&raw, 20, 20) > 0.9 && m(&raw, 8, 20) < 0.01, "the key: speckles and square");
    let clean = key(100.0, 0.0);
    for (x, y) in [(3, 3), (35, 6), (5, 33), (33, 34), (20, 4)] {
        assert!(m(&clean, x, y) < 0.05, "speckle at {x},{y}: {}", m(&clean, x, y));
    }
    assert!(m(&clean, 20, 20) > 0.95, "the square stays keyed");
    // half denoise: half way
    let half = key(50.0, 0.0);
    assert!((0.3..0.7).contains(&m(&half, 3, 3)), "{}", m(&half, 3, 3));
}

#[test]
fn hsl_refine_blur_softens_the_key() {
    let soft = |img: &Image| (0..40).filter(|&x| (0.05..0.95).contains(&img.get(x, 20)[1])).count();
    let hard = key(0.0, 0.0);
    assert_eq!(soft(&hard), 0, "a hard key");
    let blurred = key(0.0, 25.0);
    let n = soft(&blurred);
    assert!(n >= 6, "soft edge pixels: {n}");
    // the key's total is kept (blur only moves it around)
    let total = |img: &Image| (0..40 * 40).map(|i| filmcraft_color::linear_to_srgb(img.px[i * 4 + 1])).sum::<f32>();
    assert!((total(&blurred) - total(&hard)).abs() / total(&hard) < 0.05);
    // the correction follows the refined key: a hue shift inside, soft at the edge
    let mut img = keyed_image();
    let e =
        lumetri(&[("hsl_on", ParamValue::Bool(true)), ("hsl_hue", F(0.0)), ("hsl_hue_range", F(40.0)), ("hsl_hue_shift", F(120.0)), ("hsl_denoise", F(100.0))]);
    apply(&mut img, &e, &cx(WorkingSpace::Rec709));
    let inside = img.get(20, 20);
    assert!(inside[1] > inside[0], "red turned green inside: {inside:?}");
    let speck = img.get(3, 3);
    assert!(speck[0] > speck[1], "speckle left alone after denoise: {speck:?}");
}

/// A negative black endpoint can straddle zero luma with slightly unequal channels. The
/// shadow lift must approach black continuously instead of amplifying that residue at 1e-5.
#[test]
fn negative_black_endpoint_has_no_colored_shadow_step() {
    let e = lumetri(&[
        ("exposure", F(-0.65)),
        ("contrast", F(40.0)),
        ("highlights", F(-95.0)),
        ("shadows", F(14.0)),
        ("whites", F(-40.0)),
        ("blacks", F(-12.0)),
        ("temperature", F(-6.0)),
        ("tint", F(3.0)),
        ("saturation", F(75.0)),
        ("creative_on", ParamValue::Bool(false)),
        ("curves_on", ParamValue::Bool(false)),
        ("vignette_on", ParamValue::Bool(false)),
    ]);
    let mut source = Image::new(2048, 3);
    for y in 0..3 {
        for x in 0..2048 {
            let base = x as f32 * (0.01 / 2047.0);
            let jitter = [0.0, 0.00001, 0.0001][y];
            source.px[(y * 2048 + x) * 4..][..4].copy_from_slice(&[(base + jitter).max(0.0), base, (base - jitter).max(0.0), 1.0]);
        }
    }
    let context = cx(WorkingSpace::Rec709);
    let mut reference = source.clone();
    apply(&mut reference, &e, &context);
    let mut planned = source;
    crate::gpufx::FxOp::eval(&e, &context, 2048, 3).unwrap().apply(&mut planned);
    for (label, image) in [("reference", &reference), ("planned", &planned)] {
        assert!(image.px.iter().all(|v| v.is_finite()), "{label}");
        for y in 0..3 {
            for x in 1..2048 {
                let a = image.get(x - 1, y);
                let b = image.get(x, y);
                for k in 0..3 {
                    assert!((a[k] - b[k]).abs() < 0.01, "{label}: colored step at {x},{y}: {a:?} -> {b:?}");
                }
            }
        }
    }
    for (a, b) in reference.px.iter().zip(&planned.px) {
        assert!((a - b).abs() < 1e-5, "reference/planned mismatch: {a} vs {b}");
    }
}

#[test]
fn shadow_toe_preserves_black_and_the_existing_outer_response() {
    use crate::gpufx::black_anchored_shadow_weight as weight;
    for l in [-10.0f32, -0.01, 0.0] {
        assert_eq!(weight(l), 0.0);
    }
    for l in [0.05f32, 0.1, 0.5, 1.0, 4.0] {
        assert_eq!(weight(l), (1.0 - l).clamp(0.0, 1.0).powi(3));
    }
    // The gain contribution weight/l approaches zero, avoiding a hidden new division floor.
    assert!(weight(1e-7) / 1e-7 < 0.001);
    assert!((weight(0.05 - 1e-6) - weight(0.05 + 1e-6)).abs() < 1e-5);
}

#[test]
fn lifted_hdr_shadows_remain_finite_and_black_anchored() {
    for working in [WorkingSpace::Rec2100Pq, WorkingSpace::Rec2100Hlg] {
        for shadows in [-100.0, 14.0, 100.0] {
            let e = lumetri(&[("shadows", F(shadows))]);
            let levels = [0.0, 0.00001, 0.001, 1.0, 203.0, 1000.0, 4000.0];
            let got = grade(working, &levels, &e);
            assert!(got.iter().all(|v| v.is_finite() && *v >= 0.0), "{working:?}, {shadows}: {got:?}");
            assert!(got[0] < 0.001, "black lifted: {working:?}, {shadows}: {got:?}");
            assert!(got[1] < 0.1, "near-black flash: {working:?}, {shadows}: {got:?}");
        }
    }
}
