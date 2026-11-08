//! Local microphone transmission gate, independent of the App Server command queue.
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

#[derive(Default)]
pub(crate) struct MicrophoneGate {
    // Low bit: muted. Remaining bits: transition generation, so a rapid mute/unmute
    // cannot be missed by a delayed audio worker that must discard its input backlog.
    state: AtomicU64,
    // Headphones keep near-end speech available while the AI speaks. Speakers can
    // retain the existing half-duplex echo reduction without muting AI output.
    echo_reduction: AtomicBool,
}
impl MicrophoneGate {
    pub fn set_echo_reduction(&self, enabled: bool) {
        self.echo_reduction.store(enabled, Ordering::SeqCst);
    }
    pub fn suppress_for_output(&self, recent_speech: bool) -> bool {
        recent_speech && self.echo_reduction.load(Ordering::SeqCst)
    }
    pub fn set(&self, muted: bool) -> Result<(), String> {
        for _ in 0..16 {
            let state = self.snapshot();
            if (state & 1 != 0) == muted {
                return Ok(());
            }
            let next = state.checked_add(2).ok_or("Microphone mute counter exhausted; disconnect and reconnect Codex")?;
            let next = (next & !1) | u64::from(muted);
            if self.state.compare_exchange(state, next, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
                return Ok(());
            }
        }
        Err("Microphone mute changed concurrently; retry".into())
    }
    pub fn snapshot(&self) -> u64 {
        self.state.load(Ordering::SeqCst)
    }
    pub fn muted(snapshot: u64) -> bool {
        snapshot & 1 != 0
    }
    /// Recheck immediately before RTP submission. A transition during this frame
    /// suppresses it even if the final state is unmuted; the next frame drops the
    /// device backlog and resampler history before collecting fresh audio.
    pub fn protect_packet(&self, frame_snapshot: u64, pcmu: &mut [u8]) -> bool {
        let current = self.snapshot();
        let suppressed = current != frame_snapshot || Self::muted(current);
        if suppressed {
            pcmu.fill(255); // G.711 mu-law silence.
        }
        suppressed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mute_suppresses_audio_and_unmute_requires_a_fresh_frame() {
        let gate = MicrophoneGate::default();
        let initial = gate.snapshot();
        let mut frame = vec![3, 4, 5];
        assert!(!gate.protect_packet(initial, &mut frame));
        gate.set(true).unwrap();
        assert!(gate.protect_packet(initial, &mut frame));
        assert_eq!(frame, vec![255; 3]);
        let muted = gate.snapshot();
        gate.set(false).unwrap();
        assert!(gate.protect_packet(muted, &mut frame));
        let mut fresh = vec![6, 7, 8];
        assert!(!gate.protect_packet(gate.snapshot(), &mut fresh));
        assert_eq!(fresh, vec![6, 7, 8]);
    }
    #[test]
    fn rapid_toggle_is_visible_to_a_delayed_worker_without_a_command_queue() {
        let gate = MicrophoneGate::default();
        let old = gate.snapshot();
        gate.set(true).unwrap();
        gate.set(false).unwrap();
        assert!(!MicrophoneGate::muted(gate.snapshot()));
        assert_ne!(old, gate.snapshot(), "worker must discard input and resampler history");
        let mut pending = vec![5; 160];
        assert!(gate.protect_packet(old, &mut pending));
        assert!(pending.iter().all(|b| *b == 255));
        let current = gate.snapshot();
        gate.set(false).unwrap();
        assert_eq!(current, gate.snapshot(), "idempotent UI events do not drop fresh audio");
    }
    #[test]
    fn exhausted_counter_returns_error_without_changing_the_gate() {
        let gate = MicrophoneGate { state: AtomicU64::new(u64::MAX - 1), ..Default::default() };
        assert!(gate.set(true).is_err());
        assert_eq!(gate.snapshot(), u64::MAX - 1);
    }
}
