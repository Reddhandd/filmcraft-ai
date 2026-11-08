//! WebRTC speech peer. No microphone is opened until an explicit Start Voice request connects.
mod audio;
mod context;
mod input_frame;
mod mute;
mod pcm;
pub(super) mod selection;
pub(super) use mute::MicrophoneGate;
#[cfg(test)]
mod recovery_live;
use super::*;
use rtc::rtp_transceiver::rtp_sender::{RTCRtpCodec, RTCRtpCodecParameters, RTCRtpCodingParameters, RTCRtpEncodingParameters, RtpCodecKind};
use std::collections::BTreeMap;
use webrtc::{
    data_channel::DataChannelEvent,
    media_stream::{
        MediaStreamTrack,
        track_local::{TrackLocal, static_rtp::TrackLocalStaticRTP},
        track_remote::{TrackRemote, TrackRemoteEvent},
    },
    peer_connection::{
        MediaEngine, PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler, RTCIceGatheringState, RTCPeerConnectionState, RTCSessionDescription,
    },
};

static AUDIO_SLOT: AtomicBool = AtomicBool::new(false);
const SSRC: u32 = 0x46435631;
pub(super) enum Event {
    Offer(String),
    Ready,
    Data(Value),
    Stats { sent: u64, received: u64 },
    InputStats(filmcraft_ui_egui::console::VoiceInputStats),
    Failed(String),
    Closed,
}
enum Control {
    Answer(String),
}
pub(super) enum Input {
    Microphone(u16),
    #[cfg(test)]
    Fixture {
        samples: Vec<f32>,
        lifetime: Duration,
    },
}
pub(super) struct Peer {
    commands: mpsc::SyncSender<Control>,
    events: mpsc::Receiver<Event>,
    stop: Arc<AtomicBool>,
}
impl Peer {
    #[cfg(test)]
    pub fn start(input: Input) -> Result<Self, String> {
        Self::start_with_gate(input, Arc::new(MicrophoneGate::default()))
    }
    fn start_with_gate(input: Input, gate: Arc<MicrophoneGate>) -> Result<Self, String> {
        if AUDIO_SLOT.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_err() {
            return Err("The previous voice audio worker is still stopping; retry shortly".into());
        }
        let (commands, rx) = mpsc::sync_channel(4);
        let (tx, events) = mpsc::sync_channel(64);
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let spawned = std::thread::Builder::new().name("console-voice".into()).spawn(move || {
            struct Slot;
            impl Drop for Slot {
                fn drop(&mut self) {
                    AUDIO_SLOT.store(false, Ordering::SeqCst);
                }
            }
            let _slot = Slot;
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let runtime = tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .build()
                    .map_err(|e| format!("Cannot start voice runtime: {e}"))?;
                runtime.block_on(run(rx, &tx, &worker_stop, input, gate))
            }));
            let error = match result {
                Ok(Ok(())) => None,
                Ok(Err(error)) => Some(error),
                Err(_) => Some("Voice worker failed; stop and retry".into()),
            };
            if let Some(error) = error {
                let _ = tx.try_send(Event::Failed(error));
            }
            let _ = tx.try_send(Event::Closed);
        });
        if let Err(error) = spawned {
            AUDIO_SLOT.store(false, Ordering::SeqCst);
            return Err(format!("Cannot start voice worker: {error}"));
        }
        Ok(Self { commands, events, stop })
    }
    pub fn answer(&self, sdp: String) -> Result<(), String> {
        if sdp.len() > 128 * 1024 {
            return Err("Voice SDP answer is too large".into());
        }
        self.commands.try_send(Control::Answer(sdp)).map_err(|_| "Voice signaling queue is full or closed".into())
    }
    pub fn poll(&self) -> Result<Option<Event>, String> {
        match self.events.try_recv() {
            Ok(event) => Ok(Some(event)),
            Err(mpsc::TryRecvError::Empty) => Ok(None),
            Err(mpsc::TryRecvError::Disconnected) => Err("Voice worker closed".into()),
        }
    }
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}
impl Drop for Peer {
    fn drop(&mut self) {
        self.stop();
    }
}
struct Handler {
    connected: Arc<AtomicBool>,
    gathered: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    shared: Arc<audio::Shared>,
    tracks: mpsc::SyncSender<Arc<dyn TrackRemote>>,
}
#[async_trait::async_trait]
impl PeerConnectionEventHandler for Handler {
    async fn on_ice_gathering_state_change(&self, state: RTCIceGatheringState) {
        if state == RTCIceGatheringState::Complete {
            self.gathered.store(true, Ordering::SeqCst);
        }
    }
    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
        if state == RTCPeerConnectionState::Connected {
            self.connected.store(true, Ordering::SeqCst);
        }
        if matches!(state, RTCPeerConnectionState::Failed | RTCPeerConnectionState::Disconnected | RTCPeerConnectionState::Closed)
            && !self.stop.load(Ordering::SeqCst)
        {
            self.shared.fail("Voice media connection closed; stop and retry");
        }
    }
    async fn on_track(&self, track: Arc<dyn TrackRemote>) {
        if self.tracks.try_send(track).is_err() {
            self.shared.fail("Too many incoming voice tracks");
        }
    }
}
/// Small bounded reorder window; RTP sequence counters intentionally wrap at 16 bits.
#[derive(Default)]
struct Jitter {
    next: Option<u16>,
    packets: BTreeMap<u16, Vec<u8>>,
}
impl Jitter {
    fn push(&mut self, seq: u16, payload: Vec<u8>, shared: &audio::Shared) -> Result<usize, String> {
        if payload.len() > 8192 {
            return Err("Voice RTP packet exceeds the limit".into());
        }
        let expected = *self.next.get_or_insert(seq);
        let ahead = seq.wrapping_sub(expected);
        if ahead > 32768 {
            return Ok(0);
        }
        if ahead > 32 {
            self.packets.clear();
            self.next = Some(seq);
        }
        self.packets.entry(seq).or_insert(payload);
        let mut count = 0usize;
        for _ in 0..64 {
            let Some(next) = self.next else { break };
            if let Some(payload) = self.packets.remove(&next) {
                shared.push(&payload)?;
                count = count.saturating_add(payload.len());
                self.next = Some(next.wrapping_add(1));
            } else if self.packets.len() > 4 {
                shared.push(&[255; audio::FRAME])?;
                self.next = Some(next.wrapping_add(1));
            } else {
                break;
            }
        }
        if self.packets.len() > 64 {
            return Err("Voice jitter buffer is full".into());
        }
        Ok(count)
    }
}
async fn run(
    commands: mpsc::Receiver<Control>,
    events: &mpsc::SyncSender<Event>,
    stop: &Arc<AtomicBool>,
    input: Input,
    gate: Arc<MicrophoneGate>,
) -> Result<(), String> {
    let shared = Arc::new(audio::Shared { microphone_gate: gate, ..Default::default() });
    let connected = Arc::new(AtomicBool::new(false));
    let gathered = Arc::new(AtomicBool::new(false));
    let (tracks, track_rx) = mpsc::sync_channel(2);
    let codec = RTCRtpCodec { mime_type: "audio/PCMU".into(), clock_rate: audio::RATE, channels: 1, ..Default::default() };
    let mut media = MediaEngine::default();
    media.register_codec(RTCRtpCodecParameters { rtp_codec: codec.clone(), payload_type: 0 }, RtpCodecKind::Audio).map_err(|e| e.to_string())?;
    let pc = PeerConnectionBuilder::new()
        .with_media_engine(media)
        .with_handler(Arc::new(Handler {
            connected: Arc::clone(&connected),
            gathered: Arc::clone(&gathered),
            stop: Arc::clone(stop),
            shared: Arc::clone(&shared),
            tracks,
        }))
        .with_udp_addrs(vec!["0.0.0.0:0"])
        .build()
        .await
        .map_err(|e| format!("Cannot create voice peer: {e}"))?;
    let result = media_loop(&pc, commands, events, stop, input, shared, connected, gathered, track_rx, codec).await;
    let _ = tokio::time::timeout(Duration::from_secs(2), pc.close()).await;
    result
}
#[allow(clippy::too_many_arguments)]
async fn media_loop(
    pc: &impl PeerConnection,
    commands: mpsc::Receiver<Control>,
    events: &mpsc::SyncSender<Event>,
    stop: &Arc<AtomicBool>,
    input: Input,
    shared: Arc<audio::Shared>,
    connected: Arc<AtomicBool>,
    gathered: Arc<AtomicBool>,
    tracks: mpsc::Receiver<Arc<dyn TrackRemote>>,
    codec: RTCRtpCodec,
) -> Result<(), String> {
    let emit = |event| events.try_send(event).map_err(|_| "Voice event queue is full or closed".to_owned());
    let track = Arc::new(TrackLocalStaticRTP::new(MediaStreamTrack::new(
        "filmcraft-voice".into(),
        "microphone".into(),
        "FilmCraft Voice".into(),
        RtpCodecKind::Audio,
        vec![RTCRtpEncodingParameters {
            rtp_coding_parameters: RTCRtpCodingParameters { ssrc: Some(SSRC), ..Default::default() },
            active: true,
            codec,
            ..Default::default()
        }],
    )));
    pc.add_track(track.clone()).await.map_err(|e| e.to_string())?;
    let data = pc.create_data_channel("oai-events", None).await.map_err(|e| e.to_string())?;
    let offer = pc.create_offer(None).await.map_err(|e| e.to_string())?;
    pc.set_local_description(offer).await.map_err(|e| e.to_string())?;
    let start = Instant::now();
    while !gathered.load(Ordering::SeqCst) {
        if stop.load(Ordering::SeqCst) {
            return Ok(());
        }
        if start.elapsed() > Duration::from_secs(10) {
            return Err("Voice ICE gathering timed out".into());
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let offer = pc.local_description().await.ok_or("Voice peer has no local description")?;
    if offer.sdp.len() > 128 * 1024 {
        return Err("Voice SDP offer is too large".into());
    }
    emit(Event::Offer(offer.sdp))?;
    let mut devices = None;
    let mut ready = false;
    let mut service_ready = false;
    let mut was_silent = true;
    let mut remote: Option<Arc<dyn TrackRemote>> = None;
    let mut jitter = Jitter::default();
    let mut sequence = 0u16;
    let mut timestamp = 0u32;
    let (mut sent, mut received) = (0u64, 0u64);
    let mut last_stats = Instant::now();
    #[cfg(test)]
    let mut fixture_position = 0usize;
    let mut tick = tokio::time::interval(Duration::from_millis(20));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let lifetime = match &input {
        Input::Microphone(_) => Duration::from_secs(3600),
        #[cfg(test)]
        Input::Fixture { lifetime, .. } => *lifetime,
    };
    while !stop.load(Ordering::SeqCst) {
        if start.elapsed() > lifetime {
            return Err("Voice session reached its one-hour limit; start a new session".into());
        }
        if !ready && start.elapsed() > Duration::from_secs(60) {
            return Err("Voice media connection timed out; check the network and retry".into());
        }
        if let Some(error) = shared.error.lock().unwrap_or_else(PoisonError::into_inner).take() {
            return Err(error);
        }
        for _ in 0..4 {
            match commands.try_recv() {
                Ok(Control::Answer(answer)) => {
                    let answer = RTCSessionDescription::answer(answer).map_err(|e| e.to_string())?;
                    pc.set_remote_description(answer).await.map_err(|e| format!("Cannot negotiate voice audio: {e}"))?;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => return Ok(()),
            }
        }
        if let Ok(track) = tracks.try_recv() {
            if remote.is_some() {
                return Err("Unexpected extra voice audio track".into());
            }
            remote = Some(track);
        }
        if connected.load(Ordering::SeqCst) && service_ready && !ready {
            match &input {
                Input::Microphone(channel) => {
                    devices = Some(audio::Devices::start(Arc::clone(&shared), *channel)?);
                }
                #[cfg(test)]
                Input::Fixture { .. } => {}
            }
            if stop.load(Ordering::SeqCst) {
                return Ok(());
            }
            ready = true;
            emit(Event::Ready)?;
        }
        let incoming = remote.clone();
        tokio::select! {
            _=tick.tick()=>{
                if ready{
                    let input_snapshot = shared.microphone_gate.snapshot();
                    let mut payload=match &input{Input::Microphone(_)=>devices.as_mut().ok_or("Voice microphone did not start")?.frame()?,#[cfg(test)]Input::Fixture{samples,..}=>{
                        let mut frame=vec![255;audio::FRAME];for encoded in &mut frame{*encoded=pcm::encode(samples.get(fixture_position).copied().unwrap_or(0.0));fixture_position=fixture_position.saturating_add(1);}frame
                    }};
                    shared.microphone_gate.protect_packet(input_snapshot, &mut payload);
                    // Wrapping is required for RTP sequence and timestamp counters (RFC 3550).
                    let silent=payload.iter().all(|b|matches!(b,255|127));
                    let marker=!silent&&was_silent;was_silent=silent;
                    let packet=rtc::rtp::Packet{header:rtc::rtp::Header{version:2,marker,payload_type:0,sequence_number:sequence,timestamp,ssrc:SSRC,..Default::default()},payload:bytes::Bytes::from(payload)};
                    tokio::time::timeout(Duration::from_secs(1),track.write_rtp(packet)).await.map_err(|_|"Voice media send timed out")?.map_err(|e|e.to_string())?;
                    sequence=sequence.wrapping_add(1);timestamp=timestamp.wrapping_add(audio::FRAME as u32);sent=sent.saturating_add(audio::FRAME as u64);
                }
            },
            event=async{match incoming{Some(track)=>track.poll().await,None=>std::future::pending().await}}=>{
                match event{Some(TrackRemoteEvent::OnRtpPacket(packet))=>{
                    if packet.header.payload_type!=0{return Err("The voice peer selected an unsupported audio format".into());}
                    received=received.saturating_add(jitter.push(packet.header.sequence_number,packet.payload.to_vec(),&shared)? as u64);
                    #[cfg(test)]if matches!(input,Input::Fixture{..}){shared.speaker.lock().unwrap_or_else(PoisonError::into_inner).clear();}
                },Some(TrackRemoteEvent::OnError)=>return Err("Voice audio receiver failed".into()),Some(TrackRemoteEvent::OnEnded)|None=>return Err("Voice audio track ended".into()),_=>{}}
            },
            event=data.poll()=>{
                match event{Some(DataChannelEvent::OnMessage(message))=>{
                    if message.data.len()>64*1024{return Err("Voice data message is too large".into());}
                    if let Ok(value)=serde_json::from_slice::<Value>(&message.data){if value.get("type").and_then(Value::as_str)==Some("session.started"){service_ready=true;}emit(Event::Data(value))?;}
                },Some(DataChannelEvent::OnError)=>return Err("Voice data channel failed".into()),Some(DataChannelEvent::OnClose)|None=>return Err("Voice data channel closed".into()),_=>{}}
            }
        }
        if last_stats.elapsed() > Duration::from_secs(1) {
            emit(Event::Stats { sent, received })?;
            if let Some(devices) = devices.as_mut() {
                emit(Event::InputStats(devices.take_stats()))?;
            }
            last_stats = Instant::now();
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn jitter_handles_reordering_duplicates_wrap_and_hostile_payloads() {
        let shared = audio::Shared::default();
        let mut jitter = Jitter::default();
        assert_eq!(jitter.push(65534, vec![255; 160], &shared).unwrap(), 160);
        assert_eq!(jitter.push(0, vec![255; 160], &shared).unwrap(), 0);
        assert_eq!(jitter.push(65535, vec![255; 160], &shared).unwrap(), 320);
        assert_eq!(jitter.push(65535, vec![255; 160], &shared).unwrap(), 0);
        assert!(jitter.push(1, vec![255; 8193], &shared).is_err());
    }
}

/// App Server owns speech, conversation and tool handoffs; the peer only carries audio.
#[derive(Default)]
pub(super) struct Controller {
    peer: Option<Peer>,
    active: bool,
    starting: Option<String>,
    stopping: Option<String>,
    next: u64,
    pub waiting_model: bool,
    failed: bool,
    needs_reset: bool,
    data_transcripts: bool,
    input_channel: u16,
    selected_voice: Option<String>,
    available_voices: Vec<String>,
    roles: BTreeMap<String, String>,
    since: Option<Instant>,
    session_started: Option<Instant>,
    renewing: bool,
    media_ready: bool,
    context: context::Context,
}
impl Controller {
    pub fn restore_context(&mut self, thread: &str, request: &filmcraft_ui_egui::console::sessions::SessionRequest) {
        self.context.restore(thread, request);
    }
    pub fn ready(&self) -> bool {
        self.active && self.media_ready && self.starting.is_none() && self.stopping.is_none() && !self.failed
    }
    pub fn session_age(&self) -> Option<Duration> {
        self.session_started.map(|start| start.elapsed())
    }
    pub fn renewal_stopped(&self) -> bool {
        self.renewing && self.peer.is_none() && self.starting.is_none() && self.stopping.is_none() && !self.needs_reset && !self.failed
    }
    pub fn prepare_renewal(&mut self, wire: &chat::Wire) -> Result<(), String> {
        if !self.renewal_stopped() {
            return Err("The previous voice session has not closed; voice was not restarted".into());
        }
        let muted = MicrophoneGate::muted(wire.process.microphone_gate.snapshot());
        let voices = self.available_voices.clone();
        let choice = self.selected_voice.clone();
        self.prepare_selected(wire, self.input_channel, &voices, choice)?;
        // No peer or device exists while prepare resets its gate. Restore mute before opening one.
        wire.process.microphone_gate.set(muted)
    }
    pub fn failed(&self) -> bool {
        self.failed && !self.active
    }
    pub fn active(&self) -> bool {
        self.active
    }
    pub fn prepare(&mut self, wire: &chat::Wire, channel: u16) -> Result<(), String> {
        if self.needs_reset {
            return Err(
                "Voice shutdown was not confirmed. Disconnect and reconnect Codex to reset voice; your project and saved conversation remain available.".into(),
            );
        }
        if self.peer.is_some() || self.waiting_model || self.starting.is_some() || self.stopping.is_some() {
            return Err("A voice session is already running or stopping".into());
        }
        self.since = Some(Instant::now());
        self.session_started = None;
        self.renewing = false;
        self.media_ready = false;
        if channel > 255 {
            return Err("Microphone channel is out of range".into());
        }
        wire.process.microphone_gate.set(false)?;
        self.input_channel = channel;
        self.failed = false;
        self.data_transcripts = false;
        self.context.end_stream();
        wire.emit(filmcraft_ui_egui::console::ChatEvent::Voice(filmcraft_ui_egui::console::VoiceStatus::Connecting))
    }
    pub fn prepare_selected(&mut self, wire: &chat::Wire, channel: u16, voices: &[String], choice: Option<String>) -> Result<(), String> {
        selection::validate_choice(voices, choice.as_deref())?;
        self.prepare(wire, channel)?;
        self.selected_voice = choice;
        self.available_voices = voices.to_vec();
        Ok(())
    }
    pub fn start_peer(&mut self, wire: &chat::Wire) -> Result<(), String> {
        self.waiting_model = false;
        let started = Instant::now();
        match Peer::start_with_gate(Input::Microphone(self.input_channel), Arc::clone(&wire.process.microphone_gate)) {
            Ok(peer) => {
                self.peer = Some(peer);
                self.session_started = Some(started);
                self.since = Some(Instant::now());
                Ok(())
            }
            Err(error) => self.fail(wire, error),
        }
    }
    fn id(&mut self) -> Result<String, String> {
        self.next = self.next.checked_add(1).ok_or("Voice request counter exhausted")?;
        Ok(format!("voice-{}", self.next))
    }
    pub fn stop(&mut self, writer: &mut impl Write, wire: &chat::Wire, thread: &str) -> Result<(), String> {
        self.renewing = false;
        self.session_started = None;
        self.stop_inner(writer, wire, thread)
    }
    pub fn stop_for_renewal(&mut self, writer: &mut impl Write, wire: &chat::Wire, thread: &str) -> Result<(), String> {
        self.renewing = true;
        self.stop_inner(writer, wire, thread)
    }
    fn stop_inner(&mut self, writer: &mut impl Write, wire: &chat::Wire, thread: &str) -> Result<(), String> {
        let remote_session = self.active || self.starting.is_some();
        self.waiting_model = false;
        self.active = false;
        self.media_ready = false;
        self.peer = None;
        if self.stopping.is_none() && remote_session {
            let id = self.id()?;
            self.stopping = Some(id.clone());
            self.since = Some(Instant::now());
            send(writer, &serde_json::json!({"id":id,"method":"thread/realtime/stop","params":{"threadId":thread}}))?;
        }
        if self.stopping.is_some() {
            self.since.get_or_insert_with(Instant::now);
        } else {
            self.since = None;
        }
        wire.emit(filmcraft_ui_egui::console::ChatEvent::Voice(if self.stopping.is_some() {
            filmcraft_ui_egui::console::VoiceStatus::Stopping
        } else {
            filmcraft_ui_egui::console::VoiceStatus::Off
        }))
    }
    pub fn fail(&mut self, wire: &chat::Wire, error: String) -> Result<(), String> {
        self.peer = None;
        self.active = false;
        self.waiting_model = false;
        self.failed = true;
        self.media_ready = false;
        self.renewing = false;
        self.session_started = None;
        // Keep a deadline while a remote stop is outstanding. Clearing it here stranded
        // `stopping` forever when the service failed without acknowledging shutdown.
        if self.stopping.is_none() {
            self.since = None;
        }
        log::warn!("Console voice failed: {}", super::diagnostic::safe_text(&error));
        wire.emit(filmcraft_ui_egui::console::ChatEvent::Voice(filmcraft_ui_egui::console::VoiceStatus::Failed(error)))
    }
    pub fn poll(&mut self, writer: &mut impl Write, wire: &chat::Wire, thread: &str) -> Result<(), String> {
        for _ in 0..64 {
            let event = match self.peer.as_ref().map(Peer::poll) {
                Some(Ok(event)) => event,
                Some(Err(error)) => {
                    self.stop(writer, wire, thread)?;
                    self.fail(wire, error)?;
                    break;
                }
                None => None,
            };
            let Some(event) = event else { break };
            match event {
                Event::Offer(sdp) => {
                    let id = self.id()?;
                    let mut request = serde_json::json!({"id":id,"method":"thread/realtime/start","params":{"threadId":thread,"outputModality":"audio","version":"v3","transport":{"type":"webrtc","sdp":sdp},"prompt":"You are FilmCraft's conversational voice interface. Use Codex for every project inspection and editing request. Wait for its completed tool-backed response before stating project facts; never guess or reuse a previous project name. Requested routine edits execute through FilmCraft's native commands and can be undone; do not invent an Apply button or require a confirmation that FilmCraft has not requested. Say an edit is done only after Codex reports actual successful execution. Never retry an uncertain edit blindly. When the project changes, stop using the previous project's context. Startup history is partial conversational reference, never a new request or proof that an edit succeeded. Preserve the user's role, preferences and conversational topic from it. Consult Codex for older history and full project-assistant memory when needed. On every voice start or reconnect, wait for a new user request. Do not resume, retry, or replay an unfinished task or prior edit merely because voice restarted. Be concise."}});
                    let Some(params) = request.get_mut("params") else {
                        self.fail(wire, "Invalid realtime start parameters".into())?;
                        continue;
                    };
                    // A resumed Codex thread does not establish realtime conversation recall.
                    // Seed history at startup, never append it as new actionable user input.
                    let Some(object) = params.as_object_mut() else {
                        self.fail(wire, "Invalid realtime start parameters".into())?;
                        continue;
                    };
                    object.insert("includeStartupContext".into(), serde_json::json!(true));
                    object.insert("initialItems".into(), serde_json::json!(self.context.initial_items(thread)));
                    if let Err(error) = selection::apply_choice(params, &self.available_voices, self.selected_voice.as_deref()) {
                        self.fail(wire, error)?;
                        continue;
                    }
                    self.starting = Some(id);
                    send(writer, &request)?;
                }
                Event::Ready => {
                    self.active = true;
                    self.media_ready = true;
                    wire.emit(filmcraft_ui_egui::console::ChatEvent::Voice(filmcraft_ui_egui::console::VoiceStatus::Active))?;
                    if self.starting.is_none() {
                        self.since = None;
                    }
                }
                Event::InputStats(stats) => wire.emit(filmcraft_ui_egui::console::ChatEvent::VoiceInput(stats))?,
                Event::Stats { sent, received } => wire.emit(filmcraft_ui_egui::console::ChatEvent::VoiceStats { sent, received })?,
                Event::Data(value) => {
                    if let Err(error) = self.data(wire, &value) {
                        self.stop(writer, wire, thread)?;
                        self.fail(wire, error)?;
                        break;
                    }
                }
                Event::Failed(error) => {
                    self.stop(writer, wire, thread)?;
                    self.fail(wire, error)?;
                    break;
                }
                Event::Closed => {
                    self.stop(writer, wire, thread)?;
                    break;
                }
            }
        }
        if self.since.is_some_and(|since| since.elapsed() > Duration::from_secs(65)) {
            if self.stopping.is_some() {
                // The stop request already went out. A missing acknowledgement must not
                // retain the local retry gate forever or issue another remote request.
                self.starting = None;
                self.stopping = None;
                self.since = None;
                self.needs_reset = true;
                self.fail(wire, "Codex voice shutdown was not confirmed. Disconnect and reconnect Codex to reset voice. Text chat remains available.".into())?;
            } else {
                self.stop(writer, wire, thread)?;
                self.fail(wire, "Codex voice startup timed out; stopping the previous session before retry".into())?;
            }
        }
        Ok(())
    }
    fn data(&mut self, wire: &chat::Wire, value: &Value) -> Result<(), String> {
        let kind = value.get("type").and_then(Value::as_str).unwrap_or("");
        if kind == "error" {
            return Err("Codex voice reported an error; turn voice off and retry".into());
        }
        let (role, partial, text) = match kind {
            "input_transcript.added" => ("user", true, value.pointer("/item/text").and_then(Value::as_str).unwrap_or("")),
            "turn.created" => {
                let role = value.pointer("/turn/role").and_then(Value::as_str).unwrap_or("assistant");
                if let Some(id) = value.pointer("/turn/id").and_then(Value::as_str).filter(|s| s.len() <= 256) {
                    if self.roles.len() >= 16 {
                        self.roles.clear();
                    }
                    self.roles.insert(id.into(), role.into());
                }
                (role, true, value.pointer("/turn/transcript").and_then(Value::as_str).unwrap_or(""))
            }
            "turn.delta" => {
                let role = value.get("turn_id").and_then(Value::as_str).and_then(|id| self.roles.get(id)).map(String::as_str).unwrap_or("assistant");
                (role, true, value.get("delta").and_then(Value::as_str).unwrap_or(""))
            }
            "session.input_transcript.delta" | "conversation.item.input_audio_transcription.delta" => {
                ("user", true, value.get("delta").and_then(Value::as_str).unwrap_or(""))
            }
            "session.output_transcript.delta" | "response.audio_transcript.delta" | "response.output_audio_transcript.delta" => {
                ("assistant", true, value.get("delta").and_then(Value::as_str).unwrap_or(""))
            }
            "conversation.item.input_audio_transcription.completed" => ("user", false, value.get("transcript").and_then(Value::as_str).unwrap_or("")),
            "response.audio_transcript.done" | "response.output_audio_transcript.done" => {
                ("assistant", false, value.get("transcript").and_then(Value::as_str).unwrap_or(""))
            }
            _ => return Ok(()),
        };
        if !matches!(role, "user" | "assistant") {
            return Ok(());
        }
        if text.len() > 64 * 1024 {
            return Err("Voice transcript exceeded the display limit".into());
        }
        if !text.is_empty() {
            self.data_transcripts = true;
            self.context.record(role, text, partial);
            wire.emit(filmcraft_ui_egui::console::ChatEvent::VoiceText { role: role.into(), text: text.into(), partial })?;
        }
        Ok(())
    }
    pub fn message(&mut self, writer: &mut impl Write, wire: &chat::Wire, thread: &str, message: &Value) -> Result<(), String> {
        if let Some(id) = message.get("id").and_then(Value::as_str) {
            if self.starting.as_deref() == Some(id) {
                self.starting = None;
                if message.get("error").is_some() {
                    self.fail(wire, format!("Could not start voice: {}", super::diagnostic::error_details(message)))?;
                } else if self.media_ready {
                    self.since = None;
                }
            }
            if self.stopping.as_deref() == Some(id) {
                self.starting = None;
                if message.get("error").is_some() {
                    self.stopping = None;
                    self.since = None;
                    self.needs_reset = true;
                    self.fail(
                        wire,
                        format!("Voice stop failed: {}. Disconnect and reconnect Codex to reset voice.", super::diagnostic::error_details(message)),
                    )?;
                }
                // A reply acknowledges the request, not necessarily closure. Retain the
                // bounded stop gate until `thread/realtime/closed`; otherwise its late
                // notification can tear down a newly started peer on the same thread.
            }
        }
        let p = message.get("params").unwrap_or(&Value::Null);
        if p.get("threadId").and_then(Value::as_str) != Some(thread) {
            return Ok(());
        }
        match message.get("method").and_then(Value::as_str) {
            Some("thread/realtime/started") => {
                if self.peer.is_some() {
                    self.active = true;
                }
            }
            Some("thread/realtime/sdp") => {
                if let Some(peer) = self.peer.as_ref() {
                    let sdp = p.get("sdp").and_then(Value::as_str).ok_or("Codex voice returned no SDP answer")?;
                    if let Err(error) = peer.answer(sdp.into()) {
                        self.stop(writer, wire, thread)?;
                        self.fail(wire, error)?;
                    }
                }
            }
            Some("thread/realtime/error") => {
                self.stop(writer, wire, thread)?;
                self.fail(wire, format!("Voice connection failed: {}. Text chat remains available.", super::diagnostic::error_details(p)))?;
            }
            Some("thread/realtime/closed") => {
                self.peer = None;
                self.active = false;
                self.media_ready = false;
                self.starting = None;
                self.stopping = None;
                self.since = None;
                self.needs_reset = false;
                if !self.failed {
                    wire.emit(filmcraft_ui_egui::console::ChatEvent::Voice(if self.renewing {
                        filmcraft_ui_egui::console::VoiceStatus::Connecting
                    } else {
                        filmcraft_ui_egui::console::VoiceStatus::Off
                    }))?;
                }
            }
            Some("thread/realtime/transcript/done") => {
                let role = p.get("role").and_then(Value::as_str).unwrap_or("");
                let text = p.get("text").and_then(Value::as_str).unwrap_or("");
                if !self.data_transcripts && matches!(role, "user" | "assistant") && text.len() <= 64 * 1024 {
                    self.context.record(role, text, false);
                    wire.emit(filmcraft_ui_egui::console::ChatEvent::VoiceText { role: role.into(), text: text.into(), partial: false })?;
                }
            }
            _ => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod recovery_tests {
    use super::*;
    #[test]
    fn short_voice_restart_seeds_prior_dialogue_as_history_without_appending_a_new_request() {
        let (wire, _events) = wire();
        let mut controller = Controller::default();
        controller.restore_context(
            "same-thread",
            &filmcraft_ui_egui::console::sessions::SessionRequest { memory: "Role: music video editor".into(), ..Default::default() },
        );
        controller.prepare(&wire, 0).unwrap();
        controller
            .data(&wire, &json!({"type":"conversation.item.input_audio_transcription.completed","transcript":"Remember copper otter. Do not edit."}))
            .unwrap();
        controller.active = true;
        let mut out = Vec::new();
        controller.stop(&mut out, &wire, "same-thread").unwrap();
        controller.message(&mut out, &wire, "same-thread", &json!({"method":"thread/realtime/closed","params":{"threadId":"same-thread"}})).unwrap();
        controller.prepare(&wire, 0).unwrap();
        let (events, receiver) = mpsc::sync_channel(2);
        let (commands, _) = mpsc::sync_channel(2);
        controller.peer = Some(Peer { commands, events: receiver, stop: Arc::new(AtomicBool::new(false)) });
        events.send(Event::Offer("synthetic-offer".into())).unwrap();
        controller.poll(&mut out, &wire, "same-thread").unwrap();
        let sent: Vec<Value> = String::from_utf8(out).unwrap().lines().map(|s| serde_json::from_str(s).unwrap()).collect();
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[0]["method"], "thread/realtime/stop");
        assert_eq!(sent[1]["method"], "thread/realtime/start");
        let params = &sent[1]["params"];
        assert_eq!(params["threadId"], "same-thread");
        assert_eq!(params["includeStartupContext"], true);
        assert!(params["initialItems"].to_string().contains("music video editor"));
        assert!(params["initialItems"].to_string().contains("copper otter"));
        assert!(params["prompt"].as_str().unwrap().contains("never a new request"));
    }

    fn wire() -> (chat::Wire, mpsc::Receiver<filmcraft_ui_egui::console::ChatEvent>) {
        let (_, commands) = mpsc::channel();
        let (events, receiver) = mpsc::sync_channel(16);
        (chat::Wire { process: Arc::new(Process::default()), commands, events, ctx: egui::Context::default(), session: None }, receiver)
    }

    #[test]
    fn renewal_waits_for_closure_and_preserves_mute_voice_and_channel() {
        let (wire, events) = wire();
        wire.process.microphone_gate.set(true).unwrap();
        let mut controller = Controller {
            active: true,
            media_ready: true,
            input_channel: 2,
            selected_voice: Some("sol".into()),
            available_voices: vec!["sol".into()],
            session_started: Some(Instant::now()),
            ..Default::default()
        };
        controller.restore_context(
            "same-thread",
            &filmcraft_ui_egui::console::sessions::SessionRequest {
                recent_dialogue: vec![("user".into(), "Remember silver ibis through automatic renewal".into())],
                ..Default::default()
            },
        );
        let mut out = Vec::new();
        controller.stop_for_renewal(&mut out, &wire, "same-thread").unwrap();
        assert!(!controller.renewal_stopped());
        assert!(controller.prepare_renewal(&wire).is_err());
        let id = controller.stopping.clone().unwrap();
        controller.message(&mut out, &wire, "same-thread", &json!({"id":id,"result":{}})).unwrap();
        assert!(!controller.renewal_stopped(), "request acknowledgement is not closure");
        controller.message(&mut out, &wire, "same-thread", &json!({"method":"thread/realtime/closed","params":{"threadId":"same-thread"}})).unwrap();
        assert!(controller.renewal_stopped());
        controller.prepare_renewal(&wire).unwrap();
        assert!(serde_json::to_string(&controller.context.initial_items("same-thread")).unwrap().contains("silver ibis"));
        assert!(MicrophoneGate::muted(wire.process.microphone_gate.snapshot()));
        assert_eq!(controller.selected_voice.as_deref(), Some("sol"));
        assert_eq!(controller.input_channel, 2);
        assert!(controller.peer.is_none(), "preparation must not open a device");
        assert!(!controller.ready());
        assert!(!events.try_iter().any(|event| matches!(event, filmcraft_ui_egui::console::ChatEvent::Voice(filmcraft_ui_egui::console::VoiceStatus::Off))));
        assert_eq!(String::from_utf8(out).unwrap().matches("thread/realtime/stop").count(), 1);
    }

    #[test]
    fn explicit_stop_cancels_renewal_before_remote_closure() {
        let (wire, events) = wire();
        let mut controller = Controller { active: true, media_ready: true, ..Default::default() };
        let mut out = Vec::new();
        controller.stop_for_renewal(&mut out, &wire, "same-thread").unwrap();
        controller.stop(&mut out, &wire, "same-thread").unwrap();
        controller.message(&mut out, &wire, "same-thread", &json!({"method":"thread/realtime/closed","params":{"threadId":"same-thread"}})).unwrap();
        assert!(!controller.renewal_stopped());
        assert!(controller.prepare_renewal(&wire).is_err());
        assert!(events.try_iter().any(|event| matches!(event, filmcraft_ui_egui::console::ChatEvent::Voice(filmcraft_ui_egui::console::VoiceStatus::Off))));
        assert_eq!(String::from_utf8(out).unwrap().matches("thread/realtime/stop").count(), 1);
    }

    #[test]
    fn media_readiness_alone_cannot_claim_a_confirmed_start() {
        let (wire, _) = wire();
        let mut controller =
            Controller { active: true, media_ready: true, starting: Some("voice-start".into()), since: Some(Instant::now()), ..Default::default() };
        assert!(!controller.ready());
        controller.message(&mut Vec::new(), &wire, "thread", &json!({"id":"voice-start","result":{}})).unwrap();
        assert!(controller.ready());
        assert!(controller.since.is_none());
    }
    #[test]
    fn unavailable_voice_is_rejected_before_preparing_audio_and_default_is_not_inferred() {
        let (wire, receiver) = wire();
        let mut controller = Controller::default();
        let voices = vec!["sol".to_owned()];
        assert!(controller.prepare_selected(&wire, 0, &voices, Some("marin".into())).is_err());
        assert!(controller.peer.is_none());
        assert!(controller.since.is_none());
        assert!(receiver.try_recv().is_err());
        controller.prepare_selected(&wire, 0, &voices, Some("sol".into())).unwrap();
        assert_eq!(controller.selected_voice.as_deref(), Some("sol"));
        assert!(controller.peer.is_none());
        controller.prepare_selected(&wire, 0, &voices, None).unwrap();
        assert!(controller.selected_voice.is_none());
        assert!(controller.peer.is_none());
    }

    #[test]
    fn failed_stop_keeps_deadline_and_unconfirmed_shutdown_requires_connection_reset() {
        let (wire, receiver) = wire();
        let mut controller = Controller { starting: Some("voice-start".into()), since: Some(Instant::now()), ..Default::default() };
        let mut output = Vec::new();
        controller.stop(&mut output, &wire, "thread").unwrap();
        controller.fail(&wire, "expired".into()).unwrap();
        assert!(controller.stopping.is_some());
        assert!(controller.since.is_some());
        assert!(controller.prepare(&wire, 0).is_err());
        let bytes = output.len();
        controller.since = Instant::now().checked_sub(Duration::from_secs(66));
        controller.poll(&mut output, &wire, "thread").unwrap();
        assert_eq!(output.len(), bytes, "shutdown timeout must not replay a request");
        assert!(controller.starting.is_none());
        assert!(controller.stopping.is_none());
        assert!(controller.prepare(&wire, 0).unwrap_err().contains("reconnect Codex"));
        Controller::default().prepare(&wire, 0).unwrap();
        assert!(receiver.try_iter().count() >= 3);
    }

    #[test]
    fn startup_timeout_gives_remote_shutdown_its_own_bounded_deadline() {
        let (wire, _receiver) = wire();
        let mut controller =
            Controller { starting: Some("unacknowledged-start".into()), since: Instant::now().checked_sub(Duration::from_secs(66)), ..Default::default() };
        let mut output = Vec::new();
        controller.poll(&mut output, &wire, "thread").unwrap();
        assert!(controller.stopping.is_some());
        assert!(controller.since.unwrap().elapsed() < Duration::from_secs(1));
        controller.poll(&mut output, &wire, "thread").unwrap();
        assert!(controller.stopping.is_some(), "startup age must not prematurely expire shutdown");
        assert_eq!(String::from_utf8(output).unwrap().lines().count(), 1);
    }

    #[test]
    fn stopping_active_voice_sends_remote_stop_without_a_startup_timer() {
        let (wire, _receiver) = wire();
        let mut controller = Controller { active: true, ..Default::default() };
        let mut output = Vec::new();
        controller.stop(&mut output, &wire, "same-thread").unwrap();
        let request: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(request["method"], "thread/realtime/stop");
        assert_eq!(request["params"]["threadId"], "same-thread");
        assert!(controller.stopping.is_some());
        assert!(controller.since.is_some());
        let deadline = controller.since;
        controller.stop(&mut output, &wire, "same-thread").unwrap();
        assert_eq!(controller.since, deadline, "repeated stop must not postpone cleanup");
        assert_eq!(String::from_utf8(output).unwrap().lines().count(), 1);
    }

    #[test]
    fn stop_reply_waits_for_close_before_retry_on_same_conversation() {
        let (wire, _receiver) = wire();
        let mut controller = Controller { active: true, starting: Some("unacknowledged-start".into()), ..Default::default() };
        let mut output = Vec::new();
        controller.stop(&mut output, &wire, "same-thread").unwrap();
        controller.fail(&wire, "expired".into()).unwrap();
        let id = controller.stopping.clone().unwrap();
        controller.message(&mut output, &wire, "same-thread", &json!({"id":id,"result":{}})).unwrap();
        assert!(controller.stopping.is_some());
        assert!(controller.since.is_some());
        assert!(controller.prepare(&wire, 0).is_err());
        controller.message(&mut output, &wire, "same-thread", &json!({"method":"thread/realtime/closed","params":{"threadId":"same-thread"}})).unwrap();
        assert!(controller.stopping.is_none());
        assert!(controller.since.is_none());
        controller.prepare(&wire, 0).unwrap();
        controller.starting = Some("new-start".into());
        let since = controller.since;
        controller.message(&mut output, &wire, "same-thread", &json!({"id":id,"result":{}})).unwrap();
        assert_eq!(controller.starting.as_deref(), Some("new-start"));
        assert_eq!(controller.since, since, "late stop reply must not erase a new startup deadline");
        assert_eq!(String::from_utf8(output).unwrap().matches("same-thread").count(), 1);
    }
}

#[cfg(test)]
mod live_test {
    use super::*;
    /// Authenticated provider test with generated speech. Never opens microphone or speakers.
    #[test]
    #[ignore = "requires signed-in Codex, FILMCRAFT_CONSOLE_TEST_CLI and FILMCRAFT_VOICE_TEST_PCM (8 kHz mono PCM16LE)"]
    fn codex_voice_round_trip_uses_generated_speech_without_devices() {
        let path = std::env::var_os("FILMCRAFT_VOICE_TEST_PCM").expect("set generated raw PCM fixture path");
        let mut bytes = Vec::new();
        std::fs::File::open(path).unwrap().take(256 * 1024 + 1).read_to_end(&mut bytes).unwrap();
        assert!(!bytes.is_empty() && bytes.len() <= 256 * 1024 && bytes.len() % 2 == 0);
        let samples: Vec<f32> = bytes.as_chunks::<2>().0.iter().map(|b| f32::from(i16::from_le_bytes([b[0], b[1]])) / 32768.0).collect();
        let cli = std::env::var("FILMCRAFT_CONSOLE_TEST_CLI").expect("set CLI path");
        let token = "cd".repeat(32);
        let mut child = super::super::app_server(&token).spawn().unwrap();
        let mut writer = child.stdin.take().unwrap();
        let reader = child.stdout.take().unwrap();
        let process = Arc::new(Process::default());
        *process.child.lock().unwrap() = Some(child);
        struct Cleanup(Arc<Process>);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                self.0.reap();
            }
        }
        let _cleanup = Cleanup(process);
        let (tx, messages) = mpsc::sync_channel(32);
        let read_thread = std::thread::spawn(move || {
            let mut reader = BufReader::new(reader);
            loop {
                let v = super::super::read_message(&mut reader);
                let failed = v.is_err();
                if tx.send(v).is_err() || failed {
                    break;
                }
            }
        });
        let server = serde_json::json!({"command":cli,"args":["mcp","--controller-read-only","--demo"],"enabled":true,"required":true});
        let (thread, _, _) = super::super::handshake(&mut writer, &messages, server).unwrap();
        let peer = Peer::start(Input::Fixture { samples, lifetime: Duration::from_secs(60) }).unwrap();
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut transcript = String::new();
        let mut received = 0u64;
        let mut sent = 0u64;
        let mut failure = None;
        let mut types = std::collections::BTreeSet::new();
        let mut methods = std::collections::BTreeSet::new();
        let mut inspections = 0usize;
        let mut inspection_sources = Vec::new();
        let mut last_inspection = None;
        let mut completed = false;
        let require_handoff = std::env::var_os("FILMCRAFT_VOICE_TEST_PROJECT").is_some();
        while Instant::now() < deadline {
            for _ in 0..64 {
                match peer.poll() {
                    Ok(Some(Event::Offer(sdp))) => {
                        super::super::send(&mut writer,&serde_json::json!({"id":"voice-live-test","method":"thread/realtime/start","params":{"threadId":thread,"version":"v3","outputModality":"audio","transport":{"type":"webrtc","sdp":sdp},"prompt":"This is a FilmCraft voice test. Wait for spoken input, then respond briefly. Use Codex for project questions and use its FilmCraft inspection tools to ground the answer. Never edit the project."}})).unwrap();
                    }
                    Ok(Some(Event::InputStats(_))) => {}
                    Ok(Some(Event::Stats { sent: s, received: r })) => {
                        sent = s;
                        received = r;
                    }
                    Ok(Some(Event::Data(value))) => {
                        if let Some(kind) = value.get("type").and_then(Value::as_str) {
                            types.insert(kind.to_owned());
                        }
                        if matches!(value.get("type").and_then(Value::as_str), Some("turn.delta" | "turn.created" | "input_transcript.added")) {
                            fn shape(v: &Value, prefix: &str, depth: usize, out: &mut Vec<String>) {
                                if depth > 3 || out.len() > 64 {
                                    return;
                                }
                                if let Some(map) = v.as_object() {
                                    for (k, v) in map.iter().take(16) {
                                        let path = format!("{prefix}.{k}");
                                        out.push(format!(
                                            "{path}:{}",
                                            if v.is_string() {
                                                "str"
                                            } else if v.is_object() {
                                                "object"
                                            } else if v.is_array() {
                                                "array"
                                            } else {
                                                "scalar"
                                            }
                                        ));
                                        shape(v, &path, depth + 1, out);
                                    }
                                }
                            }
                            let mut fields = Vec::new();
                            shape(&value, "event", 0, &mut fields);
                            eprintln!("fixture voice structure {fields:?}");
                        }
                        for key in ["delta", "transcript", "text"] {
                            if let Some(text) = value.get(key).and_then(Value::as_str)
                                && transcript.len() < 16 * 1024
                            {
                                transcript.push_str(text);
                            }
                        }
                    }
                    Ok(Some(Event::Failed(error))) => {
                        failure = Some(error);
                        break;
                    }
                    Ok(Some(Event::Closed)) | Err(_) => {
                        failure = Some("voice peer closed".into());
                        break;
                    }
                    Ok(Some(Event::Ready)) => {}
                    Ok(None) => break,
                }
            }
            for _ in 0..64 {
                let message = match messages.try_recv() {
                    Ok(Ok(message)) => message,
                    Ok(Err(error)) => {
                        failure = Some(error);
                        break;
                    }
                    Err(_) => break,
                };
                let p = message.get("params").unwrap_or(&Value::Null);
                if let Some(method) = message.get("method").and_then(Value::as_str) {
                    methods.insert(method.to_owned());
                }
                if message.get("method").and_then(Value::as_str) == Some("item/completed")
                    && p.pointer("/item/type").and_then(Value::as_str) == Some("mcpToolCall")
                    && p.pointer("/item/status").and_then(Value::as_str) == Some("completed")
                {
                    inspections = inspections.saturating_add(1);
                    inspection_sources.push(json!({"sameThread":p.get("threadId").and_then(Value::as_str)==Some(thread.as_str()),"server":p.pointer("/item/server"),"tool":p.pointer("/item/tool")}));
                    last_inspection = Some(Instant::now());
                }

                if message.get("method").and_then(Value::as_str) == Some("turn/completed") {
                    completed = true;
                }
                match message.get("method").and_then(Value::as_str) {
                    Some("thread/realtime/sdp") => peer.answer(p.get("sdp").and_then(Value::as_str).unwrap().into()).unwrap(),
                    Some("thread/realtime/error") => {
                        failure = Some("Codex realtime service reported an error".into());
                    }
                    Some("thread/realtime/transcript/done") => {
                        if let Some(text) = p.get("text").and_then(Value::as_str) {
                            transcript.push_str(text);
                        }
                    }
                    _ => {}
                }
                if message.get("id").is_some() && message.get("method").is_some() {
                    super::super::send(
                        &mut writer,
                        &serde_json::json!({"id":message["id"],"error":{"code":-32601,"message":"Voice transport test refuses tool calls"}}),
                    )
                    .unwrap();
                }
                if message.get("id").and_then(Value::as_str) == Some("voice-live-test") && message.get("error").is_some() {
                    failure = Some("Codex rejected WebRTC voice startup".into());
                }
            }
            if failure.is_some()
                || received > 0
                    && sent > 8000
                    && if require_handoff {
                        inspections > 0 && completed && last_inspection.is_some_and(|at| at.elapsed() > Duration::from_secs(2))
                    } else {
                        transcript.to_lowercase().contains("hello")
                    }
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        peer.stop();
        super::super::send(&mut writer, &serde_json::json!({"id":"voice-stop-test","method":"thread/realtime/stop","params":{"threadId":thread}})).unwrap();
        drop(peer);
        drop(writer);
        _cleanup.0.reap();
        drop(messages);
        read_thread.join().unwrap();
        eprintln!(
            "voice test sent_frames={sent} received_frames={received} inspections={inspections} sources={inspection_sources:?} completed={completed} app_server_methods={methods:?} event_types={types:?} generated_fixture_transcript={transcript:?}"
        );
        assert!(failure.is_none(), "{failure:?}");
        assert!(received > 0, "No speech audio received");
        if require_handoff {
            assert!(inspections > 0, "Voice did not delegate a project inspection to Codex");
        } else {
            assert!(transcript.to_lowercase().contains("hello"), "Generated speech was not transcribed");
        }
    }
}
