//! Match two renditions of a song using chroma and constrained dynamic time warping.
//! Original, dependency-free DSP. This is a suggested edit, not a lip-sync guarantee.
use crate::fft::Fft;

#[path = "performance_passages.rs"]
pub mod passages;

pub const RATE: u32 = 8_000;
const HOP: usize = 800;
const WINDOW: usize = 2048;
const MAX_SAMPLES: usize = RATE as usize * 600;
const MAX_CELLS: usize = 20_000_000;

#[derive(Clone, Debug)]
pub struct Anchor {
    pub reference_sample: usize,
    pub live_sample: usize,
    pub similarity: f32,
}

#[derive(Clone, Debug)]
pub struct Match {
    pub anchors: Vec<Anchor>,
    pub similarity: f32,
}

#[derive(Clone, Debug)]
struct Feature {
    chroma: [f32; 12],
    onset: f32,
}

fn features(audio: &[f32]) -> Result<Vec<Feature>, String> {
    if audio.len() < RATE as usize * 2 || audio.len() > MAX_SAMPLES {
        return Err("Use a trimmed song between 2 seconds and 10 minutes.".into());
    }
    if audio.iter().any(|v| !v.is_finite() || v.abs() > 1_000.0) {
        return Err("Audio contains invalid samples.".into());
    }
    let energy = audio.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>();
    if energy / (audio.len() as f64) < 1e-12 {
        return Err("The recording is silent.".into());
    }
    let fft = Fft::new(WINDOW);
    let mut re = vec![0.0; WINDOW];
    let mut im = vec![0.0; WINDOW];
    let mut result = Vec::with_capacity(audio.len().div_ceil(HOP));
    let mut previous = [0.0f32; 12];
    for center in (0..audio.len()).step_by(HOP) {
        re.fill(0.0);
        im.fill(0.0);
        for (k, v) in re.iter_mut().enumerate() {
            let index = center.checked_add(k).and_then(|i| i.checked_sub(WINDOW / 2));
            let sample = index.and_then(|i| audio.get(i)).copied().unwrap_or(0.0);
            let window = 0.5 - 0.5 * (2.0 * std::f32::consts::PI * k as f32 / (WINDOW - 1) as f32).cos();
            *v = sample * window;
        }
        fft.forward(&mut re, &mut im);
        let mut chroma = [0.0f32; 12];
        for (bin, (&r, &i)) in re.iter().zip(&im).enumerate().take(WINDOW / 2).skip(1) {
            let freq = bin as f32 * RATE as f32 / WINDOW as f32;
            if !(65.0..=2000.0).contains(&freq) {
                continue;
            }
            let midi = (69.0 + 12.0 * (freq / 440.0).log2()).round() as i32;
            let pitch = midi.rem_euclid(12) as usize;
            if let Some(v) = chroma.get_mut(pitch) {
                *v += (r * r + i * i).sqrt();
            }
        }
        let norm = chroma.iter().map(|v| v * v).sum::<f32>().sqrt();
        if norm > 1e-12 {
            chroma.iter_mut().for_each(|v| *v /= norm);
        }
        let onset = chroma.iter().zip(previous).map(|(a, b)| (a - b).max(0.0)).sum::<f32>();
        previous = chroma;
        result.push(Feature { chroma, onset });
    }
    let maximum = result.iter().map(|f| f.onset).fold(0.0f32, f32::max);
    if maximum > 1e-12 {
        result.iter_mut().for_each(|f| f.onset /= maximum);
    }
    Ok(result)
}

fn similarity(a: &Feature, b: &Feature) -> f32 {
    let pitch = a.chroma.iter().zip(b.chroma).map(|(x, y)| x * y).sum::<f32>().clamp(0.0, 1.0);
    0.85 * pitch + 0.15 * (1.0 - (a.onset - b.onset).abs().min(1.0))
}

fn align(a: &[Feature], b: &[Feature]) -> Result<(Vec<(usize, usize)>, f32), String> {
    let (n, m) = (a.len(), b.len());
    if n < 2 || m < 2 || n > 6000 || m > 6000 || n > m.saturating_mul(2) || m > n.saturating_mul(2) {
        return Err("The song lengths differ too much. Trim both clips to the same arrangement.".into());
    }
    let band = (m / 8).max(30);
    let cells = n.checked_mul(band.saturating_mul(2).saturating_add(1)).ok_or("Analysis is too large.")?;
    if cells > MAX_CELLS {
        return Err("Analysis is too large. Use a shorter section.".into());
    }
    let mut trace: Vec<(usize, Vec<u8>)> = Vec::with_capacity(n + 1);
    trace.push((0, vec![0]));
    let mut older = vec![f32::INFINITY; m + 1];
    let mut previous = older.clone();
    if let Some(v) = previous.get_mut(0) {
        *v = 0.0;
    }
    for i in 1..=n {
        let center = i.saturating_mul(m) / n;
        let lo = center.saturating_sub(band).max(1);
        let hi = center.saturating_add(band).min(m);
        let mut row = vec![f32::INFINITY; m + 1];
        let mut dirs = Vec::with_capacity(hi.saturating_sub(lo) + 1);
        let fa = a.get(i - 1).ok_or("Invalid analysis row.")?;
        for j in lo..=hi {
            let fb = b.get(j - 1).ok_or("Invalid analysis column.")?;
            let cost = 1.0 - similarity(fa, fb);
            let mut best = (previous.get(j - 1).copied().unwrap_or(f32::INFINITY) + cost, 1u8);
            if i >= 2 {
                let v = older.get(j - 1).copied().unwrap_or(f32::INFINITY) + cost * 2.0 + 0.03;
                if v < best.0 {
                    best = (v, 2);
                }
            }
            if j >= 2 {
                let v = previous.get(j - 2).copied().unwrap_or(f32::INFINITY) + cost * 2.0 + 0.03;
                if v < best.0 {
                    best = (v, 3);
                }
            }
            if let Some(v) = row.get_mut(j) {
                *v = best.0;
            }
            dirs.push(best.1);
        }
        trace.push((lo, dirs));
        older = previous;
        previous = row;
    }
    if !previous.get(m).copied().unwrap_or(f32::INFINITY).is_finite() {
        return Err("Could not find a continuous match. Trim the recordings more closely.".into());
    }
    let (mut i, mut j) = (n, m);
    let mut path = Vec::with_capacity(n + m);
    let mut score = 0.0;
    while i > 0 && j > 0 {
        let (lo, dirs) = trace.get(i).ok_or("Invalid match path.")?;
        let direction = j.checked_sub(*lo).and_then(|k| dirs.get(k)).copied().ok_or("Match left the search window.")?;
        path.push((i - 1, j - 1));
        score += similarity(a.get(i - 1).ok_or("Invalid path.")?, b.get(j - 1).ok_or("Invalid path.")?);
        match direction {
            1 => {
                i -= 1;
                j -= 1;
            }
            2 if i >= 2 => {
                i -= 2;
                j -= 1;
            }
            3 if j >= 2 => {
                i -= 1;
                j -= 2;
            }
            _ => return Err("Invalid match direction.".into()),
        }
    }
    if i != 0 || j != 0 || path.is_empty() {
        return Err("The song beginnings do not match.".into());
    }
    score /= path.len() as f32;
    path.reverse();
    Ok((path, score))
}

/// Look for the studio song's final musical phrase before the end of a roughly
/// trimmed live clip. Conservative by design: ambiguous/repeated endings abstain.
/// Returns (suggested end sample, mean chroma similarity).
pub fn suggest_live_endpoint(reference: &[f32], live: &[f32]) -> Option<(usize, f32)> {
    let a = features(reference).ok()?;
    let b = features(live).ok()?;
    suggest_endpoint_features(&a, &b).map(|(frame, confidence)| ((frame * HOP).min(live.len()), confidence))
}

fn suggest_endpoint_features(a: &[Feature], b: &[Feature]) -> Option<(usize, f32)> {
    // Compare six seconds at the end of the studio recording, sampled over
    // three two-second phrases to avoid treating one repeated note as a match.
    let phrase = 60usize; // HOP=100ms
    if a.len() < phrase + 20 || b.len() < phrase + 40 {
        return None;
    }
    let start = b.len().saturating_sub(300).max(phrase + 1);
    let end = b.len().saturating_sub(20);
    if start >= end {
        return None;
    }
    let rate = [0.9_f64, 1.0, 1.1];
    let score = |ending: usize| -> Option<f32> {
        let mut best = 0.0f32;
        for tempo in rate {
            let count = (phrase as f64 * tempo).round() as usize;
            if ending < count {
                continue;
            }
            let mut sum = 0.0;
            for k in 0..phrase {
                let i = a.len() - phrase + k;
                let j = ending - count + (k as f64 * tempo).floor() as usize;
                sum += similarity(a.get(i)?, b.get(j.min(ending - 1))?);
            }
            best = best.max(sum / phrase as f32);
        }
        Some(best)
    };
    let baseline = score(b.len())?;
    let mut best = (0usize, 0.0f32);
    let mut runner_up = 0.0f32;
    for candidate in (start..end).step_by(2) {
        let value = score(candidate)?;
        if value > best.1 {
            if candidate.abs_diff(best.0) > 25 {
                runner_up = runner_up.max(best.1);
            }
            best = (candidate, value);
        } else if candidate.abs_diff(best.0) > 25 {
            runner_up = runner_up.max(value);
        }
    }
    // A high, clearly superior correspondence is needed; otherwise require
    // manual trimming. Never silently change source or matching endpoints.
    (best.1 >= 0.83 && best.1 >= baseline + 0.08 && best.1 >= runner_up + 0.035).then_some(best)
}

/// Audio must be mono at RATE. Both ranges must contain the same complete song arrangement;
/// extra applause, count-ins, missing verses and changed keys need trimming/manual corrections.
pub fn match_performance(reference: &[f32], live: &[f32]) -> Result<Match, String> {
    let a = features(reference)?;
    let b = features(live)?;
    let (path, score) = align(&a, &b)?;
    let mut anchors = vec![Anchor { reference_sample: 0, live_sample: 0, similarity: score }];
    for &(i, j) in &path {
        let Some(last) = anchors.last() else {
            return Err("Missing start anchor.".into());
        };
        let (rs, ls) = (i.saturating_mul(HOP), j.saturating_mul(HOP));
        if rs >= last.reference_sample.saturating_add(20 * HOP) && ls > last.live_sample {
            anchors.push(Anchor {
                reference_sample: rs,
                live_sample: ls,
                similarity: similarity(a.get(i).ok_or("Invalid anchor.")?, b.get(j).ok_or("Invalid anchor.")?),
            });
        }
    }
    // Avoid a tiny final segment; the exact sample lengths form the endpoint.
    if anchors.last().is_some_and(|p| reference.len().saturating_sub(p.reference_sample) < 10 * HOP) && anchors.len() > 1 {
        anchors.pop();
    }
    anchors.push(Anchor { reference_sample: reference.len(), live_sample: live.len(), similarity: score });
    Ok(Match { anchors, similarity: score })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn feature(k: usize) -> Feature {
        let mut chroma = [0.0; 12];
        chroma[k % 12] = 1.0;
        Feature { chroma, onset: 0.0 }
    }
    #[test]
    fn follows_a_changing_tempo_between_different_renditions() {
        let notes = [0, 4, 7, 2, 5, 9, 1, 8];
        let mut a = Vec::new();
        let mut b = Vec::new();
        for (index, k) in notes.into_iter().enumerate() {
            a.extend(std::iter::repeat_n(feature(k), 30));
            b.extend(std::iter::repeat_n(feature(k), if index < 4 { 24 } else { 36 }));
        }
        let (path, score) = align(&a, &b).unwrap();
        assert!(score > 0.98);
        let midpoint = path.iter().find(|(i, _)| *i >= 120).unwrap();
        assert!((midpoint.1 as i64 - 96).abs() < 4);
        assert!(path.windows(2).all(|p| p[0].0 < p[1].0 && p[0].1 < p[1].1));
    }
    #[test]
    fn unrelated_pitch_sequences_score_low() {
        let a = vec![feature(0); 100];
        let b = vec![feature(6); 100];
        let (_, score) = align(&a, &b).unwrap();
        assert!(score < 0.3);
    }
    #[test]
    fn hostile_audio_is_rejected_without_panicking() {
        for audio in [vec![], vec![0.0; 16000], vec![f32::NAN; 16000], vec![f32::INFINITY; 16000]] {
            assert!(match_performance(&audio, &audio).is_err());
        }
        assert!(align(&[], &[]).is_err());
    }
    #[test]
    fn identical_audio_has_identity_anchors_and_exact_endpoint() {
        let audio: Vec<f32> = (0..64000)
            .map(|n| {
                let f = [130.81, 164.81, 196.0, 146.83][n / 16000];
                (2.0 * std::f32::consts::PI * f * n as f32 / RATE as f32).sin() * 0.4
            })
            .collect();
        let r = match_performance(&audio, &audio).unwrap();
        assert!(r.similarity > 0.99);
        assert!(r.anchors.iter().all(|p| p.reference_sample == p.live_sample));
        assert_eq!(r.anchors.last().unwrap().reference_sample, audio.len());
    }

    #[test]
    fn matches_audio_with_different_timbre_and_varying_tempo() {
        let pitches = [130.81, 164.81, 196.0, 146.83, 174.61, 220.0, 138.59, 207.65];
        let synth = |lengths: &[usize], harmonics: bool| {
            let mut audio = Vec::new();
            for (&f, &length) in pitches.iter().zip(lengths) {
                for n in 0..length {
                    let phase = 2.0 * std::f32::consts::PI * f * n as f32 / RATE as f32;
                    let envelope = (n as f32 / 100.0).min(1.0) * ((length - n) as f32 / 100.0).min(1.0);
                    let v = if harmonics { phase.sin() * 0.3 + (phase * 2.0).sin() * 0.12 + (phase * 3.0).sin() * 0.08 } else { phase.sin() * 0.4 };
                    audio.push(v * envelope);
                }
            }
            audio
        };
        let a = synth(&[24000; 8], false);
        let b = synth(&[19200, 19200, 19200, 19200, 28800, 28800, 28800, 28800], true);
        let result = match_performance(&a, &b).unwrap();
        assert!(result.similarity > 0.8, "{}", result.similarity);
        let middle = result.anchors.iter().find(|p| p.reference_sample >= 96000).unwrap();
        assert!((middle.live_sample as i64 - 76800).abs() <= 2400, "{middle:?}");
    }
}

#[cfg(test)]
mod endpoint_tests {
    use super::*;
    #[test]
    fn detects_distinct_live_tail_and_abstains_on_ambiguous_endings() {
        // A distinctive (not cyclic) final phrase: the detector should locate
        // its true end before unrelated live footage. A repeating 12-note cycle
        // is inherently ambiguous and correctly makes the detector abstain.
        let mut seed = 0x83c9_6a21u32;
        let mut a = Vec::new();
        for _ in 0..125 {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            a.push(Feature { chroma: feature_chroma(((seed >> 16) % 12) as usize), onset: 0.0 });
        }
        let mut b = a.clone();
        for n in 0..45 {
            b.push(Feature { chroma: feature_chroma((n * 7 + 5) % 12), onset: 0.0 });
        }
        let result = suggest_endpoint_features(&a, &b);
        assert!(result.is_some(), "expected recognizable phrase end: {result:?}");
        let found = result.unwrap().0;
        assert!(found.abs_diff(a.len()) <= 3);
        assert_eq!(suggest_endpoint_features(&a, &a), None);
        let mut repeating = Vec::new();
        for n in 0..125 {
            repeating.push(Feature { chroma: feature_chroma((n / 3) % 12), onset: 0.0 });
        }
        let mut repeated_live = repeating.clone();
        for n in 0..45 {
            repeated_live.push(Feature { chroma: feature_chroma((n / 3 + 5) % 12), onset: 0.0 });
        }
        assert_eq!(suggest_endpoint_features(&repeating, &repeated_live), None);
    }
    fn feature_chroma(index: usize) -> [f32; 12] {
        let mut v = [0.0; 12];
        v[index] = 1.0;
        v
    }
}
