//! Bounded microphone packet encoding independent of speaker playback.
use super::{MicrophoneGate, audio::FRAME, pcm};
use filmcraft_ui_egui::console::VoiceInputStats;

/// Device collection has already applied explicit mute/backlog handling. The caller
/// still uses `protect_packet` immediately before RTP transmission for mute races.
pub(super) fn encode(mono: &[Vec<f32>], recent_speech: bool, gate: &MicrophoneGate, stats: &mut VoiceInputStats) -> Vec<u8> {
    let suppress = gate.suppress_for_output(recent_speech);
    let samples = mono.first().map(Vec::as_slice).unwrap_or_default();
    let mut peak = 0.0f32;
    let mut frame = Vec::with_capacity(FRAME);
    for i in 0..FRAME {
        let sample = samples.get(i).copied().filter(|s| s.is_finite()).unwrap_or(0.0).clamp(-1.0, 1.0);
        peak = peak.max(sample.abs());
        frame.push(pcm::encode(if suppress { 0.0 } else { sample }));
    }
    stats.input_peak = stats.input_peak.max(peak);
    stats.transmit_peak = stats.transmit_peak.max(if suppress { 0.0 } else { peak });
    stats.gated = suppress;
    frame
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headphones_transmit_speech_over_ai_output_and_speaker_mode_reduces_echo() {
        let gate = MicrophoneGate::default();
        let spoken = vec![vec![0.4; FRAME]];
        let mut stats = VoiceInputStats::default();
        let mut frame = encode(&spoken, true, &gate, &mut stats);
        assert_eq!(frame.len(), FRAME);
        assert!(frame.iter().all(|s| pcm::decode(*s) > 0.35));
        assert!(!stats.gated);
        assert!(stats.transmit_peak > 0.35);
        assert!(!gate.protect_packet(gate.snapshot(), &mut frame));

        gate.set_echo_reduction(true);
        let mut stats = VoiceInputStats::default();
        assert_eq!(encode(&spoken, true, &gate, &mut stats), vec![255; FRAME]);
        assert!(stats.gated);
        assert!(stats.input_peak > 0.35);
        assert_eq!(stats.transmit_peak, 0.0);
        assert!(encode(&spoken, false, &gate, &mut stats).iter().any(|s| *s != 255));
    }

    #[test]
    fn explicit_mute_and_rapid_transitions_still_prevent_transmission_in_headphones_mode() {
        let gate = MicrophoneGate::default();
        let spoken = vec![vec![0.4; FRAME]];
        let captured = gate.snapshot();
        let mut frame = encode(&spoken, true, &gate, &mut VoiceInputStats::default());
        gate.set(true).unwrap();
        assert!(gate.protect_packet(captured, &mut frame));
        assert_eq!(frame, vec![255; FRAME]);
        gate.set(false).unwrap();
        let mut stale = encode(&spoken, true, &gate, &mut VoiceInputStats::default());
        assert!(gate.protect_packet(captured, &mut stale));
        assert_eq!(stale, vec![255; FRAME]);
        let current = gate.snapshot();
        let mut fresh = encode(&spoken, true, &gate, &mut VoiceInputStats::default());
        assert!(!gate.protect_packet(current, &mut fresh));
        assert!(fresh.iter().all(|s| *s != 255));
    }

    #[test]
    fn malformed_or_short_device_frames_remain_bounded_and_finite() {
        let gate = MicrophoneGate::default();
        for mono in [vec![], vec![vec![]], vec![vec![f32::NAN, f32::INFINITY, f32::NEG_INFINITY]], vec![vec![2.0; FRAME * 4]]] {
            let mut stats = VoiceInputStats::default();
            let frame = encode(&mono, true, &gate, &mut stats);
            assert_eq!(frame.len(), FRAME);
            assert!(stats.input_peak.is_finite() && stats.input_peak <= 1.0);
            assert!(stats.transmit_peak.is_finite() && stats.transmit_peak <= 1.0);
        }
    }
}
