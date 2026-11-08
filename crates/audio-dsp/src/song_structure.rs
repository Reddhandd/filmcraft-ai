//! Conservative, editable structure suggestions from beat self-similarity and energy.
use crate::remix::Analysis;

#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub start: i64,
    pub end: i64,
    pub label: String,
}

/// Novelty peaks identify changes; recurrence plus energy suggests chorus versus verse.
/// These are musical hypotheses, not semantic recognition of lyrics or song form.
pub fn suggest(a: &Analysis, mono: &[f32]) -> Result<Vec<Section>, String> {
    let n = a.beats.len();
    if !(10..=2400).contains(&n)
        || a.sample_rate == 0
        || a.len <= 0
        || a.len as usize != mono.len()
        || mono.iter().any(|x| !x.is_finite())
        || a.sim.len() != n.saturating_mul(n)
        || a.sim.iter().any(|x| !x.is_finite())
        || a.beats.iter().any(|b| *b < 0 || *b >= a.len)
        || a.beats.windows(2).any(|w| w.first() >= w.get(1))
    {
        return Err("Invalid or oversized music analysis.".into());
    }
    let similarity = |i: usize, j: usize| a.sim.get(i * n + j).copied().unwrap_or(0.0);
    let mut novelty = vec![0.0f32; n];
    for (i, value) in novelty.iter_mut().enumerate().take(n - 4).skip(4) {
        let mut same = 0.0;
        let mut cross = 0.0;
        for x in 0..4 {
            for y in 0..4 {
                same += similarity(i - 1 - x, i - 1 - y) + similarity(i + x, i + y);
                cross += 2.0 * similarity(i - 1 - x, i + y);
            }
        }
        *value = ((same - cross) / 32.0).max(0.0);
    }
    let mean = novelty.iter().sum::<f32>() / n as f32;
    let sd = (novelty.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / n as f32).sqrt();
    let threshold = (mean + sd * 0.7).max(0.06);
    let mut candidates: Vec<usize> = (8..n.saturating_sub(8)).filter(|i| novelty.get(*i).copied().unwrap_or(0.0) > threshold).collect();
    candidates.sort_by(|x, y| novelty.get(*y).unwrap_or(&0.0).total_cmp(novelty.get(*x).unwrap_or(&0.0)));
    let mut boundaries = Vec::new();
    for i in candidates {
        if boundaries.iter().all(|b: &usize| b.abs_diff(i) >= 8) {
            boundaries.push(i);
        }
        if boundaries.len() >= 63 {
            break;
        }
    }
    boundaries.sort_unstable();
    let mut ranges = vec![(0usize, 0i64)];
    for i in boundaries {
        if let Some(&sample) = a.beats.get(i) {
            ranges.push((i, sample));
        }
    }
    ranges.push((n, a.len));
    let mut energies = Vec::new();
    for w in ranges.windows(2) {
        let (Some(&(_, start)), Some(&(_, end))) = (w.first(), w.get(1)) else { continue };
        let samples = mono.get(start as usize..end as usize).ok_or("Invalid section range")?;
        energies.push(samples.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / samples.len().max(1) as f64);
    }
    let mut repeated = vec![false; energies.len()];
    for i in 0..energies.len() {
        for j in i + 1..energies.len() {
            let (Some(&(bi, _)), Some(&(ei, _)), Some(&(bj, _)), Some(&(ej, _))) = (ranges.get(i), ranges.get(i + 1), ranges.get(j), ranges.get(j + 1)) else {
                continue;
            };
            let length = (ei - bi).min(ej - bj).min(32);
            if length < 8 {
                continue;
            }
            let score = (0..length).map(|k| similarity(bi + k, bj + k)).sum::<f32>() / length as f32;
            if score > 0.80 {
                if let Some(v) = repeated.get_mut(i) {
                    *v = true;
                }
                if let Some(v) = repeated.get_mut(j) {
                    *v = true;
                }
            }
        }
    }
    let peak = energies.iter().copied().fold(0.0, f64::max);
    let mut out = Vec::new();
    for (i, w) in ranges.windows(2).enumerate() {
        let (Some(&(_, start)), Some(&(_, end))) = (w.first(), w.get(1)) else { continue };
        let energy = energies.get(i).copied().unwrap_or(0.0);
        let repeats = repeated.get(i).copied().unwrap_or(false);
        let label = if repeats && energy >= peak * 0.65 {
            "Chorus"
        } else if repeats {
            "Verse"
        } else if i == 0 && energies.len() > 1 {
            "Intro"
        } else if i + 1 == energies.len() && energies.len() > 1 {
            "Outro"
        } else if energies.len() >= 4 && i >= energies.len() / 2 {
            "Bridge"
        } else {
            "Section"
        };
        out.push(Section { start, end, label: label.into() });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_malformed_without_panicking() {
        let a = Analysis { sample_rate: 8000, len: 8, beats: vec![0; 12], period: 1.0, bpm: 120.0, sim: vec![] };
        assert!(suggest(&a, &[0.0; 8]).is_err());
    }
    #[test]
    fn uniform_music_is_not_forced_into_verse_chorus_bridge() {
        let a = Analysis { sample_rate: 8000, len: 128, beats: (0..16).map(|i| i * 8).collect(), period: 8.0, bpm: 120.0, sim: vec![1.0; 256] };
        let result = suggest(&a, &[0.1; 128]).unwrap();
        assert_eq!(result, vec![Section { start: 0, end: 128, label: "Section".into() }]);
    }
}
