//! Read-only coarse passage retrieval. Candidates are not trim or edit instructions.
//! Endpoints of sustained/repeated phrases are inherently uncertain at this stage.
use super::{Feature, HOP, features};

const MAX_CELLS: usize = 20_000_000;
const MAX_CANDIDATES: usize = 5;
const SEPARATION: usize = 50;

#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub live_start_sample: usize,
    pub live_end_sample: usize,
    /// Uncalibrated ranking value, never a probability of a correct match.
    pub score: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Search {
    pub candidates: Vec<Candidate>,
    pub resolution_samples: usize,
    /// Passage identity and boundaries require review, even with one candidate.
    pub requires_review: bool,
}

/// Find complete-query hypotheses inside a longer live recording. This cannot
/// establish arrangement completeness or identify skipped/repeated sections.
/// Cancellation is checked before extraction, between extractions and each DP row.
pub fn find(reference: &[f32], live: &[f32], cancelled: impl Fn() -> bool) -> Result<Search, String> {
    let check = || -> Result<(), String> { if cancelled() { Err("Passage search cancelled.".into()) } else { Ok(()) } };
    check()?;
    let a = features(reference)?;
    check()?;
    let b = features(live)?;
    check()?;
    let mut candidates = search_features(&a, &b, &cancelled)?;
    for candidate in &mut candidates {
        candidate.live_end_sample = candidate.live_end_sample.min(live.len());
    }
    Ok(Search { candidates, resolution_samples: HOP, requires_review: true })
}

fn search_features(a: &[Feature], b: &[Feature], cancelled: &impl Fn() -> bool) -> Result<Vec<Candidate>, String> {
    let (n, m) = (a.len(), b.len());
    if n < 2 || m < 2 || n.checked_mul(m).is_none_or(|cells| cells > MAX_CELLS) {
        return Err("Passage search is too large or too short. Use a shorter song section.".into());
    }
    // All DP indices below are bounded by these validated dimensions. Feature
    // extraction bounds each dimension to 6000; row allocations are capped here.
    if n > 6000 || m > 6000 || n > m.saturating_mul(2) {
        return Err("The live range is too short or too large for passage search.".into());
    }
    let mut previous = vec![0.0f32; m + 1];
    let mut older = vec![f32::INFINITY; m + 1];
    let mut previous_start: Vec<usize> = (0..=m).collect();
    let mut older_start = previous_start.clone();
    let mut row = vec![f32::INFINITY; m + 1];
    let mut row_start = vec![0usize; m + 1];
    for i in 1..=n {
        if cancelled() {
            return Err("Passage search cancelled.".into());
        }
        row.fill(f32::INFINITY);
        for j in 1..=m {
            let cost = 1.0 - super::similarity(&a[i - 1], &b[j - 1]);
            let mut best = (previous[j - 1] + cost, previous_start[j - 1]);
            if i >= 2 {
                let value = older[j - 1] + 2.0 * cost + 0.03;
                if value < best.0 {
                    best = (value, older_start[j - 1]);
                }
            }
            if j >= 2 {
                let value = previous[j - 2] + 2.0 * cost + 0.03;
                if value < best.0 {
                    best = (value, previous_start[j - 2]);
                }
            }
            row[j] = best.0;
            row_start[j] = best.1;
        }
        std::mem::swap(&mut older, &mut previous);
        std::mem::swap(&mut previous, &mut row);
        std::mem::swap(&mut older_start, &mut previous_start);
        std::mem::swap(&mut previous_start, &mut row_start);
    }
    let mut ranked = Vec::with_capacity(m);
    for end in 1..=m {
        let start = previous_start[end];
        if previous[end].is_finite() && end.saturating_sub(start) >= n / 2 {
            ranked.push((1.0 - previous[end] / n as f32, start, end));
        }
    }
    ranked.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut picked: Vec<Candidate> = Vec::with_capacity(MAX_CANDIDATES);
    for (score, start, end) in ranked {
        let start = start.checked_mul(HOP).ok_or("Passage offset is too large.")?;
        let end = end.checked_mul(HOP).ok_or("Passage offset is too large.")?;
        // Adjacent endpoint variants are not independent competing passages.
        // Always retain separated repeat hypotheses, including exact ties.
        if picked.iter().all(|p| p.live_start_sample.abs_diff(start) > SEPARATION * HOP && p.live_end_sample.abs_diff(end) > SEPARATION * HOP) {
            picked.push(Candidate { live_start_sample: start, live_end_sample: end, score });
        }
        if picked.len() == MAX_CANDIDATES {
            break;
        }
    }
    Ok(picked)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn note(k: usize) -> Feature {
        let mut chroma = [0.0; 12];
        chroma[k % 12] = 1.0;
        Feature { chroma, onset: 0.0 }
    }
    fn phrase() -> Vec<Feature> {
        [0, 4, 7, 2, 5, 9, 1, 8].into_iter().flat_map(|k| std::iter::repeat_n(note(k), 30)).collect()
    }
    #[test]
    fn identical_repeats_keep_tied_alternatives() {
        let a = phrase();
        let mut b = a.clone();
        b.extend(std::iter::repeat_n(note(11), 60));
        b.extend(a.clone());
        let result = search_features(&a, &b, &|| false).unwrap();
        assert!(result.len() >= 2);
        assert!((result[0].score - result[1].score).abs() < 1e-6);
        assert_eq!(result[0].live_start_sample.abs_diff(result[1].live_start_sample), 300 * HOP);
    }
    #[test]
    fn sustained_tail_is_not_a_verified_trim_boundary() {
        let a = phrase();
        let mut b = vec![note(11); 40];
        for (i, k) in [0, 4, 7, 2, 5, 9, 1, 8].into_iter().enumerate() {
            b.extend(std::iter::repeat_n(note(k), if i < 4 { 24 } else { 36 }));
        }
        b.extend(std::iter::repeat_n(note(11), 40));
        let result = search_features(&a, &b, &|| false).unwrap();
        assert_eq!(result[0].live_start_sample, 40 * HOP);
        // Regression evidence: a valid coarse retrieval can truncate this tail.
        // It must never be exposed as an automatically verified exact trim.
        assert!(result[0].live_end_sample <= b.len() * HOP);
    }
    #[test]
    fn cancellation_and_hostile_dimensions_are_errors() {
        assert!(find(&[], &[], || true).is_err());
        assert!(find(&vec![f32::NAN; 16000], &vec![0.0; 16000], || false).is_err());
        assert!(search_features(&[], &[], &|| false).is_err());
        assert!(search_features(&vec![note(0); 5000], &vec![note(0); 5000], &|| false).is_err());
        assert!(search_features(&phrase(), &phrase(), &|| true).is_err());
    }

    #[test]
    fn even_identity_audio_requires_review_and_stays_in_source_bounds() {
        let audio: Vec<f32> = (0..32001).map(|n| (n as f32 * 0.103).sin() * 0.4).collect();
        let found = find(&audio, &audio, || false).unwrap();
        assert!(found.requires_review);
        assert!(!found.candidates.is_empty());
        assert!(found.candidates.iter().all(|c| c.live_start_sample < c.live_end_sample && c.live_end_sample <= audio.len()));
    }
}
