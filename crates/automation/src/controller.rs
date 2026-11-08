//! Preserved bounded controller profiles; the general MCP server stays in the parent module.
use super::*;
use std::time::{Duration, Instant};

const MAX_CONTROLLER_LINE: u64 = 1 << 20;

async fn controller_line<R: tokio::io::AsyncRead + Unpin>(reader: &mut tokio::io::BufReader<R>) -> std::io::Result<Option<String>> {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt};
    let mut bytes = Vec::new();
    let n = (&mut *reader).take(MAX_CONTROLLER_LINE + 1).read_until(b'\n', &mut bytes).await?;
    if n == 0 {
        return Ok(None);
    }
    if n as u64 > MAX_CONTROLLER_LINE {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "controller request exceeds 1 MiB"));
    }
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
    }
    if bytes.last() == Some(&b'\r') {
        bytes.pop();
    }
    String::from_utf8(bytes).map(Some).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

/// A least-privilege MCP surface for an AI controller that is still in its read-only stage.
/// It deliberately has no generic command runner, batch editor, import tool, or UI input tools.
#[derive(Clone)]
pub struct FilmcraftControllerMcp {
    inner: FilmcraftMcp,
    tool_router: ToolRouter<Self>,
}

/// Separate opt-in profile for previewed marker edits, clip moves, and audio placement.
#[derive(Clone)]
pub struct FilmcraftMarkerControllerMcp {
    inner: FilmcraftMcp,
    plans: Arc<Mutex<std::collections::HashMap<String, (Instant, MarkerPlan)>>>,
    move_plans: Arc<Mutex<std::collections::HashMap<String, (Instant, ClipMovePlan)>>>,
    delete_plans: Arc<Mutex<std::collections::HashMap<String, (Instant, ClipDeletePlan)>>>,
    audio_plans: Arc<Mutex<std::collections::HashMap<String, (Instant, AudioPlacePlan)>>>,
    audio_mute_plans: Arc<Mutex<std::collections::HashMap<String, (Instant, AudioTrackMutePlan)>>>,
    transition_plans: Arc<Mutex<std::collections::HashMap<String, (Instant, TransitionAddPlan)>>>,
    nest_plans: Arc<Mutex<std::collections::HashMap<String, (Instant, NestClipPlan)>>>,
    performance_plans: Arc<Mutex<std::collections::HashMap<String, (Instant, PerformanceMatchPlan)>>>,
    color_plans: Arc<Mutex<std::collections::HashMap<String, (Instant, ColorPresetPlan)>>>,
    effect_plans: Arc<Mutex<std::collections::HashMap<String, (Instant, VideoEffectPlan)>>>,
    param_plans: Arc<Mutex<std::collections::HashMap<String, (Instant, EffectParamPlan)>>>,
    remove_plans: Arc<Mutex<std::collections::HashMap<String, (Instant, RemoveEffectPlan)>>>,
    tool_router: ToolRouter<Self>,
}

#[derive(Clone, Debug)]
struct ClipMovePlan {
    sequence_id: u64,
    revision: u64,
    moves: Vec<crate::clip_move_plan::ClipMove>,
    action: &'static str,
    reference_video: Option<(u64, i64)>,
}

#[derive(Clone, Debug)]
struct ClipDeletePlan {
    sequence_id: u64,
    revision: u64,
    clip_id: u64,
    affected: Vec<crate::clip_delete_plan::DeletedClip>,
}

#[derive(Clone, Debug)]
struct AudioPlacePlan {
    sequence_id: u64,
    revision: u64,
    placement: crate::audio_place_plan::AudioPlacement,
}

#[derive(Clone, Debug)]
struct AudioTrackMutePlan {
    sequence_id: u64,
    revision: u64,
    target: crate::audio_track_mute_plan::AudioTrackMute,
}

#[derive(Clone, Debug)]
struct TransitionAddPlan {
    sequence_id: u64,
    revision: u64,
    target: crate::transition_add_plan::TransitionAdd,
}

#[derive(Clone, Debug)]
struct NestClipPlan {
    sequence_id: u64,
    revision: u64,
    target: crate::nest_clip_plan::NestTarget,
}

#[derive(Clone, Debug)]
struct PerformanceMatchPlan {
    sequence_id: u64,
    revision: u64,
    target: crate::performance_match_plan::MatchTarget,
    analysis: Value,
    song_before: Value,
}

#[derive(Clone, Debug)]
struct ColorPresetPlan {
    sequence_id: u64,
    revision: u64,
    target: crate::color_preset_plan::ColorPresetTarget,
    before_effects: Vec<Value>,
}

#[derive(Clone, Debug)]
struct VideoEffectPlan {
    sequence_id: u64,
    revision: u64,
    target: crate::video_effect_plan::VideoEffectTarget,
    effect: Value,
    before_effects: Vec<Value>,
}

#[derive(Clone, Debug)]
struct EffectParamPlan {
    sequence_id: u64,
    revision: u64,
    target: crate::effect_param_plan::EffectParamTarget,
    resolved_value: Value,
    before_effects: Vec<Value>,
}

#[derive(Clone, Debug)]
struct RemoveEffectPlan {
    sequence_id: u64,
    revision: u64,
    target: crate::video_effect_plan::RemoveEffectTarget,
    before_effects: Vec<Value>,
}

#[derive(Clone, Debug)]
enum MarkerPlan {
    Add {
        sequence_id: u64,
        revision: u64,
        time: i64,
        name: String,
        comment: String,
    },
    Edit {
        sequence_id: u64,
        revision: u64,
        marker_id: u64,
        name: Option<String>,
        comment: Option<String>,
        before_name: String,
        before_comment: String,
        start: i64,
    },
}

pub(super) async fn serve_restricted<S: ServerHandler>(
    server: S,
    input: impl tokio::io::AsyncRead + Unpin + Send + 'static,
    output: impl tokio::io::AsyncWrite + Unpin + Send + 'static,
) -> Result<(), AutomationError> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let (mut to_service, service_in) = tokio::io::duplex(1 << 20);
    let (service_out, from_service) = tokio::io::duplex(1 << 20);
    let (err_tx, mut err_rx) = tokio::sync::mpsc::channel::<String>(16);
    let reader = tokio::spawn(async move {
        let mut lines = BufReader::new(input);
        while let Ok(Some(line)) = controller_line(&mut lines).await {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<Value>(&line) {
                Ok(_) => {
                    if to_service.write_all(format!("{line}\n").as_bytes()).await.is_err() {
                        break;
                    }
                }
                Err(e) => {
                    let reply = json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": format!("parse error: {e}")}});
                    if err_tx.send(reply.to_string()).await.is_err() {
                        break;
                    }
                }
            }
        }
        let _ = to_service.shutdown().await;
    });
    let writer = tokio::spawn(async move {
        let mut output = output;
        let mut replies = BufReader::new(from_service).lines();
        loop {
            let line = tokio::select! {
                line = replies.next_line() => match line { Ok(Some(line)) => line, _ => break },
                Some(error) = err_rx.recv() => error,
            };
            if output.write_all(format!("{line}\n").as_bytes()).await.is_err() || output.flush().await.is_err() {
                break;
            }
        }
    });
    let running = match server.serve((service_in, service_out)).await {
        Ok(running) => running,
        Err(e) => {
            reader.abort();
            writer.abort();
            return Err(AutomationError::Other(format!("MCP init: {e}")));
        }
    };
    let result = running.waiting().await.map_err(|e| AutomationError::Other(e.to_string())).map(|_| ());
    reader.abort();
    let _ = writer.await;
    result
}

impl FilmcraftControllerMcp {
    pub fn headless(session: Session) -> Self {
        Self { inner: FilmcraftMcp::headless(session), tool_router: Self::tool_router() }
    }

    pub fn bridge(addr: &str) -> Result<Self, AutomationError> {
        Ok(Self { inner: FilmcraftMcp::bridge(addr)?, tool_router: Self::tool_router() })
    }

    /// Serve the restricted controller profile over MCP stdio.
    pub async fn serve_stdio(self) -> Result<(), AutomationError> {
        self.serve_io(tokio::io::stdin(), tokio::io::stdout()).await
    }

    /// Serve the restricted profile over a line stream, preserving JSON-RPC parse errors.
    pub async fn serve_io(
        self,
        input: impl tokio::io::AsyncRead + Unpin + Send + 'static,
        output: impl tokio::io::AsyncWrite + Unpin + Send + 'static,
    ) -> Result<(), AutomationError> {
        serve_restricted(self, input, output).await
    }

    fn unknown_arg(&self, tool: &str, args: Option<&rmcp::model::JsonObject>) -> Option<String> {
        let def = self.tool_router.get(tool)?;
        let props = def.input_schema.get("properties").and_then(Value::as_object).cloned().unwrap_or_default();
        let bad = args?.keys().find(|k| !props.contains_key(*k))?;
        let accepted: Vec<&str> = props.keys().map(String::as_str).collect();
        let accepted = if accepted.is_empty() { "no arguments".to_string() } else { accepted.join(", ") };
        Some(format!("unknown argument \"{bad}\" for {tool}; expected: {accepted}"))
    }
}

impl FilmcraftMarkerControllerMcp {
    pub fn headless(session: Session) -> Self {
        Self {
            inner: FilmcraftMcp::headless(session),
            plans: Arc::default(),
            move_plans: Arc::default(),
            delete_plans: Arc::default(),
            audio_plans: Arc::default(),
            audio_mute_plans: Arc::default(),
            transition_plans: Arc::default(),
            nest_plans: Arc::default(),
            performance_plans: Arc::default(),
            color_plans: Arc::default(),
            effect_plans: Arc::default(),
            param_plans: Arc::default(),
            remove_plans: Arc::default(),
            tool_router: Self::tool_router(),
        }
    }

    pub fn bridge(addr: &str) -> Result<Self, AutomationError> {
        Ok(Self {
            inner: FilmcraftMcp::bridge(addr)?,
            plans: Arc::default(),
            move_plans: Arc::default(),
            delete_plans: Arc::default(),
            audio_plans: Arc::default(),
            audio_mute_plans: Arc::default(),
            transition_plans: Arc::default(),
            nest_plans: Arc::default(),
            performance_plans: Arc::default(),
            color_plans: Arc::default(),
            effect_plans: Arc::default(),
            param_plans: Arc::default(),
            remove_plans: Arc::default(),
            tool_router: Self::tool_router(),
        })
    }

    pub async fn serve_stdio(self) -> Result<(), AutomationError> {
        self.serve_io(tokio::io::stdin(), tokio::io::stdout()).await
    }

    pub async fn serve_io(
        self,
        input: impl tokio::io::AsyncRead + Unpin + Send + 'static,
        output: impl tokio::io::AsyncWrite + Unpin + Send + 'static,
    ) -> Result<(), AutomationError> {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let (mut to_service, service_in) = tokio::io::duplex(1 << 20);
        let (service_out, from_service) = tokio::io::duplex(1 << 20);
        let (err_tx, mut err_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let reader = tokio::spawn(async move {
            let mut lines = BufReader::new(input);
            while let Ok(Some(line)) = controller_line(&mut lines).await {
                if line.trim().is_empty() {
                    continue;
                }
                match serde_json::from_str::<Value>(&line) {
                    Ok(_) => {
                        if to_service.write_all(format!("{line}\n").as_bytes()).await.is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        let _ = err_tx.send(json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":format!("parse error: {e}")}}).to_string());
                    }
                }
            }
            let _ = to_service.shutdown().await;
        });
        let writer = tokio::spawn(async move {
            let mut output = output;
            let mut replies = BufReader::new(from_service).lines();
            loop {
                let line = tokio::select! { l = replies.next_line() => match l { Ok(Some(l)) => l, _ => break }, Some(e) = err_rx.recv() => e };
                if output.write_all(format!("{line}\n").as_bytes()).await.is_err() || output.flush().await.is_err() {
                    break;
                }
            }
        });
        let running = match self.serve((service_in, service_out)).await {
            Ok(r) => r,
            Err(e) => {
                reader.abort();
                writer.abort();
                return Err(AutomationError::Other(format!("MCP init: {e}")));
            }
        };
        let result = running.waiting().await.map_err(|e| AutomationError::Other(e.to_string())).map(|_| ());
        reader.abort();
        let _ = writer.await;
        result
    }

    async fn snapshot(&self) -> Result<(u64, u64, i64, Value), String> {
        let project = self.inner.run("project.inspect", json!({})).await.map_err(|e| e.to_string())?;
        let sequence = self.inner.run("sequence.inspect", json!({})).await.map_err(|e| e.to_string())?;
        let revision = project["revision"].as_u64().ok_or("project revision unavailable")?;
        let sequence_id = sequence["id"].as_u64().ok_or("active sequence id unavailable")?;
        let duration = sequence["duration"].as_i64().ok_or("sequence duration unavailable")?;
        Ok((revision, sequence_id, duration, sequence))
    }

    fn stash_plan(&self, plan: MarkerPlan) -> Result<String, String> {
        let mut random = [0u8; 32];
        getrandom::fill(&mut random).map_err(|e| format!("could not create a secure plan token: {e}"))?;
        let token = format!("marker-plan-{}", random.iter().map(|b| format!("{b:02x}")).collect::<String>());
        let mut plans = self.plans.lock().unwrap_or_else(PoisonError::into_inner);
        plans.retain(|_, (created, _)| created.elapsed() < Duration::from_secs(300));
        if plans.len() >= 64
            && let Some(oldest) = plans.iter().min_by_key(|(_, (created, _))| *created).map(|(k, _)| k.clone())
        {
            plans.remove(&oldest);
        }
        plans.insert(token.clone(), (Instant::now(), plan));
        Ok(token)
    }

    fn take_plan(&self, token: &str) -> Result<MarkerPlan, String> {
        let entry = self.plans.lock().unwrap_or_else(PoisonError::into_inner).remove(token).ok_or("unknown, expired, or already used plan token")?;
        if entry.0.elapsed() >= Duration::from_secs(300) {
            return Err("plan expired; preview again".into());
        }
        Ok(entry.1)
    }

    fn stash_move_plan(&self, plan: ClipMovePlan) -> Result<String, String> {
        let mut random = [0u8; 32];
        getrandom::fill(&mut random).map_err(|e| format!("could not create a secure plan token: {e}"))?;
        let token = format!("clip-move-plan-{}", random.iter().map(|b| format!("{b:02x}")).collect::<String>());
        let mut plans = self.move_plans.lock().unwrap_or_else(PoisonError::into_inner);
        plans.retain(|_, (created, _)| created.elapsed() < Duration::from_secs(300));
        if plans.len() >= 64
            && let Some(oldest) = plans.iter().min_by_key(|(_, (created, _))| *created).map(|(k, _)| k.clone())
        {
            plans.remove(&oldest);
        }
        plans.insert(token.clone(), (Instant::now(), plan));
        Ok(token)
    }

    fn take_move_plan(&self, token: &str) -> Result<ClipMovePlan, String> {
        let entry = self.move_plans.lock().unwrap_or_else(PoisonError::into_inner).remove(token).ok_or("unknown, expired, or already used clip move token")?;
        if entry.0.elapsed() >= Duration::from_secs(300) {
            return Err("plan expired; preview again".into());
        }
        Ok(entry.1)
    }

    fn stash_delete_plan(&self, plan: ClipDeletePlan) -> Result<String, String> {
        let mut random = [0u8; 32];
        getrandom::fill(&mut random).map_err(|e| format!("could not create a secure plan token: {e}"))?;
        let token = format!("clip-delete-plan-{}", random.iter().map(|b| format!("{b:02x}")).collect::<String>());
        let mut plans = self.delete_plans.lock().unwrap_or_else(PoisonError::into_inner);
        plans.retain(|_, (created, _)| created.elapsed() < Duration::from_secs(300));
        if plans.len() >= 64
            && let Some(oldest) = plans.iter().min_by_key(|(_, (created, _))| *created).map(|(k, _)| k.clone())
        {
            plans.remove(&oldest);
        }
        plans.insert(token.clone(), (Instant::now(), plan));
        Ok(token)
    }

    fn take_delete_plan(&self, token: &str) -> Result<ClipDeletePlan, String> {
        let entry =
            self.delete_plans.lock().unwrap_or_else(PoisonError::into_inner).remove(token).ok_or("unknown, expired, or already used clip delete token")?;
        if entry.0.elapsed() >= Duration::from_secs(300) {
            return Err("plan expired; preview again".into());
        }
        Ok(entry.1)
    }

    fn stash_audio_plan(&self, plan: AudioPlacePlan) -> Result<String, String> {
        let mut random = [0u8; 32];
        getrandom::fill(&mut random).map_err(|e| format!("could not create a secure plan token: {e}"))?;
        let token = format!("audio-place-plan-{}", random.iter().map(|b| format!("{b:02x}")).collect::<String>());
        let mut plans = self.audio_plans.lock().unwrap_or_else(PoisonError::into_inner);
        plans.retain(|_, (created, _)| created.elapsed() < Duration::from_secs(300));
        if plans.len() >= 64
            && let Some(oldest) = plans.iter().min_by_key(|(_, (created, _))| *created).map(|(k, _)| k.clone())
        {
            plans.remove(&oldest);
        }
        plans.insert(token.clone(), (Instant::now(), plan));
        Ok(token)
    }

    fn take_audio_plan(&self, token: &str) -> Result<AudioPlacePlan, String> {
        let entry =
            self.audio_plans.lock().unwrap_or_else(PoisonError::into_inner).remove(token).ok_or("unknown, expired, or already used audio placement token")?;
        if entry.0.elapsed() >= Duration::from_secs(300) {
            return Err("plan expired; preview again".into());
        }
        Ok(entry.1)
    }

    fn stash_audio_mute_plan(&self, plan: AudioTrackMutePlan) -> Result<String, String> {
        let mut random = [0u8; 32];
        getrandom::fill(&mut random).map_err(|e| format!("could not create a secure plan token: {e}"))?;
        let token = format!("audio-mute-plan-{}", random.iter().map(|b| format!("{b:02x}")).collect::<String>());
        let mut plans = self.audio_mute_plans.lock().unwrap_or_else(PoisonError::into_inner);
        plans.retain(|_, (created, _)| created.elapsed() < Duration::from_secs(300));
        if plans.len() >= 64
            && let Some(oldest) = plans.iter().min_by_key(|(_, (created, _))| *created).map(|(k, _)| k.clone())
        {
            plans.remove(&oldest);
        }
        plans.insert(token.clone(), (Instant::now(), plan));
        Ok(token)
    }

    fn take_audio_mute_plan(&self, token: &str) -> Result<AudioTrackMutePlan, String> {
        let entry = self
            .audio_mute_plans
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(token)
            .ok_or("unknown, expired, or already used audio track mute token")?;
        if entry.0.elapsed() >= Duration::from_secs(300) {
            return Err("plan expired; preview again".into());
        }
        Ok(entry.1)
    }

    fn stash_transition_plan(&self, plan: TransitionAddPlan) -> Result<String, String> {
        let mut random = [0u8; 32];
        getrandom::fill(&mut random).map_err(|e| format!("could not create a secure plan token: {e}"))?;
        let token = format!("transition-add-plan-{}", random.iter().map(|b| format!("{b:02x}")).collect::<String>());
        let mut plans = self.transition_plans.lock().unwrap_or_else(PoisonError::into_inner);
        plans.retain(|_, (created, _)| created.elapsed() < Duration::from_secs(300));
        if plans.len() >= 64
            && let Some(oldest) = plans.iter().min_by_key(|(_, (created, _))| *created).map(|(k, _)| k.clone())
        {
            plans.remove(&oldest);
        }
        plans.insert(token.clone(), (Instant::now(), plan));
        Ok(token)
    }

    fn take_transition_plan(&self, token: &str) -> Result<TransitionAddPlan, String> {
        let entry =
            self.transition_plans.lock().unwrap_or_else(PoisonError::into_inner).remove(token).ok_or("unknown, expired, or already used transition token")?;
        if entry.0.elapsed() >= Duration::from_secs(300) {
            return Err("plan expired; preview again".into());
        }
        Ok(entry.1)
    }

    fn stash_nest_plan(&self, plan: NestClipPlan) -> Result<String, String> {
        let mut random = [0u8; 32];
        getrandom::fill(&mut random).map_err(|e| format!("could not create a secure plan token: {e}"))?;
        let token = format!("nest-clip-plan-{}", random.iter().map(|b| format!("{b:02x}")).collect::<String>());
        let mut plans = self.nest_plans.lock().unwrap_or_else(PoisonError::into_inner);
        plans.retain(|_, (created, _)| created.elapsed() < Duration::from_secs(300));
        if plans.len() >= 64
            && let Some(oldest) = plans.iter().min_by_key(|(_, (created, _))| *created).map(|(k, _)| k.clone())
        {
            plans.remove(&oldest);
        }
        plans.insert(token.clone(), (Instant::now(), plan));
        Ok(token)
    }

    fn take_nest_plan(&self, token: &str) -> Result<NestClipPlan, String> {
        let entry = self.nest_plans.lock().unwrap_or_else(PoisonError::into_inner).remove(token).ok_or("unknown, expired, or already used nesting token")?;
        if entry.0.elapsed() >= Duration::from_secs(300) {
            return Err("plan expired; preview again".into());
        }
        Ok(entry.1)
    }

    fn stash_performance_plan(&self, plan: PerformanceMatchPlan) -> Result<String, String> {
        let mut random = [0u8; 32];
        getrandom::fill(&mut random).map_err(|e| format!("could not create a secure plan token: {e}"))?;
        let token = format!("performance-match-plan-{}", random.iter().map(|b| format!("{b:02x}")).collect::<String>());
        let mut plans = self.performance_plans.lock().unwrap_or_else(PoisonError::into_inner);
        plans.retain(|_, (created, _)| created.elapsed() < Duration::from_secs(300));
        if plans.len() >= 64
            && let Some(oldest) = plans.iter().min_by_key(|(_, (created, _))| *created).map(|(k, _)| k.clone())
        {
            plans.remove(&oldest);
        }
        plans.insert(token.clone(), (Instant::now(), plan));
        Ok(token)
    }

    fn take_performance_plan(&self, token: &str) -> Result<PerformanceMatchPlan, String> {
        let entry = self
            .performance_plans
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(token)
            .ok_or("unknown, expired, or already used performance match token")?;
        if entry.0.elapsed() >= Duration::from_secs(300) {
            return Err("plan expired; preview again".into());
        }
        Ok(entry.1)
    }

    fn stash_color_plan(&self, plan: ColorPresetPlan) -> Result<String, String> {
        let mut random = [0u8; 32];
        getrandom::fill(&mut random).map_err(|e| format!("could not create a secure plan token: {e}"))?;
        let token = format!("color-preset-plan-{}", random.iter().map(|b| format!("{b:02x}")).collect::<String>());
        let mut plans = self.color_plans.lock().unwrap_or_else(PoisonError::into_inner);
        plans.retain(|_, (created, _)| created.elapsed() < Duration::from_secs(300));
        if plans.len() >= 64
            && let Some(oldest) = plans.iter().min_by_key(|(_, (created, _))| *created).map(|(k, _)| k.clone())
        {
            plans.remove(&oldest);
        }
        plans.insert(token.clone(), (Instant::now(), plan));
        Ok(token)
    }

    fn take_color_plan(&self, token: &str) -> Result<ColorPresetPlan, String> {
        let entry =
            self.color_plans.lock().unwrap_or_else(PoisonError::into_inner).remove(token).ok_or("unknown, expired, or already used color preset token")?;
        if entry.0.elapsed() >= Duration::from_secs(300) {
            return Err("plan expired; preview again".into());
        }
        Ok(entry.1)
    }

    fn stash_effect_plan(&self, plan: VideoEffectPlan) -> Result<String, String> {
        let mut random = [0u8; 32];
        getrandom::fill(&mut random).map_err(|e| format!("could not create a secure plan token: {e}"))?;
        let token = format!("video-effect-plan-{}", random.iter().map(|b| format!("{b:02x}")).collect::<String>());
        let mut plans = self.effect_plans.lock().unwrap_or_else(PoisonError::into_inner);
        plans.retain(|_, (created, _)| created.elapsed() < Duration::from_secs(300));
        if plans.len() >= 64
            && let Some(oldest) = plans.iter().min_by_key(|(_, (created, _))| *created).map(|(k, _)| k.clone())
        {
            plans.remove(&oldest);
        }
        plans.insert(token.clone(), (Instant::now(), plan));
        Ok(token)
    }

    fn take_effect_plan(&self, token: &str) -> Result<VideoEffectPlan, String> {
        let entry =
            self.effect_plans.lock().unwrap_or_else(PoisonError::into_inner).remove(token).ok_or("unknown, expired, or already used video effect token")?;
        if entry.0.elapsed() >= Duration::from_secs(300) {
            return Err("plan expired; preview again".into());
        }
        Ok(entry.1)
    }

    fn stash_param_plan(&self, plan: EffectParamPlan) -> Result<String, String> {
        let mut random = [0u8; 32];
        getrandom::fill(&mut random).map_err(|e| format!("could not create a secure plan token: {e}"))?;
        let token = format!("effect-param-plan-{}", random.iter().map(|b| format!("{b:02x}")).collect::<String>());
        let mut plans = self.param_plans.lock().unwrap_or_else(PoisonError::into_inner);
        plans.retain(|_, (created, _)| created.elapsed() < Duration::from_secs(300));
        if plans.len() >= 64
            && let Some(oldest) = plans.iter().min_by_key(|(_, (created, _))| *created).map(|(k, _)| k.clone())
        {
            plans.remove(&oldest);
        }
        plans.insert(token.clone(), (Instant::now(), plan));
        Ok(token)
    }

    fn take_param_plan(&self, token: &str) -> Result<EffectParamPlan, String> {
        let entry =
            self.param_plans.lock().unwrap_or_else(PoisonError::into_inner).remove(token).ok_or("unknown, expired, or already used effect parameter token")?;
        if entry.0.elapsed() >= Duration::from_secs(300) {
            return Err("plan expired; preview again".into());
        }
        Ok(entry.1)
    }

    fn stash_remove_plan(&self, plan: RemoveEffectPlan) -> Result<String, String> {
        let mut random = [0u8; 32];
        getrandom::fill(&mut random).map_err(|e| format!("could not create a secure plan token: {e}"))?;
        let token = format!("effect-remove-plan-{}", random.iter().map(|b| format!("{b:02x}")).collect::<String>());
        let mut plans = self.remove_plans.lock().unwrap_or_else(PoisonError::into_inner);
        plans.retain(|_, (created, _)| created.elapsed() < Duration::from_secs(300));
        if plans.len() >= 64
            && let Some(oldest) = plans.iter().min_by_key(|(_, (created, _))| *created).map(|(k, _)| k.clone())
        {
            plans.remove(&oldest);
        }
        plans.insert(token.clone(), (Instant::now(), plan));
        Ok(token)
    }

    fn take_remove_plan(&self, token: &str) -> Result<RemoveEffectPlan, String> {
        let entry =
            self.remove_plans.lock().unwrap_or_else(PoisonError::into_inner).remove(token).ok_or("unknown, expired, or already used effect removal token")?;
        if entry.0.elapsed() >= Duration::from_secs(300) {
            return Err("plan expired; preview again".into());
        }
        Ok(entry.1)
    }

    fn unknown_arg(&self, tool: &str, args: Option<&rmcp::model::JsonObject>) -> Option<String> {
        let def = self.tool_router.get(tool)?;
        let props = def.input_schema.get("properties").and_then(Value::as_object).cloned().unwrap_or_default();
        let bad = args?.keys().find(|k| !props.contains_key(*k))?;
        Some(format!(
            "unknown argument \"{bad}\" for {tool}; expected: {}",
            if props.is_empty() { "no arguments".to_string() } else { props.keys().map(String::as_str).collect::<Vec<_>>().join(", ") }
        ))
    }
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct CurrentFrameParams {
    /// Longest side of the PNG (default 960). The controller profile always renders the current playhead.
    #[serde(default)]
    pub max_side: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MarkerAddPreviewParams {
    pub sequence_id: u64,
    pub revision: u64,
    pub seconds: f64,
    pub name: String,
    #[serde(default)]
    pub comment: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MarkerEditPreviewParams {
    pub sequence_id: u64,
    pub revision: u64,
    pub marker_id: u64,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub comment: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MarkerCommitParams {
    pub plan_token: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ClipMovePreviewParams {
    pub sequence_id: u64,
    pub revision: u64,
    pub clip_id: u64,
    pub seconds: f64,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ClipDeletePreviewParams {
    pub sequence_id: u64,
    pub revision: u64,
    pub clip_id: u64,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AudioAlignPreviewParams {
    pub sequence_id: u64,
    pub revision: u64,
    pub audio_clip_id: u64,
    pub video_clip_id: u64,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AudioPlacePreviewParams {
    pub sequence_id: u64,
    pub revision: u64,
    pub item_id: u64,
    pub track: String,
    pub seconds: f64,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AudioTrackMutePreviewParams {
    pub sequence_id: u64,
    pub revision: u64,
    /// An explicit audio track label such as A1.
    pub track: String,
    pub muted: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TransitionAddPreviewParams {
    pub sequence_id: u64,
    pub revision: u64,
    pub clip_id: u64,
    pub frames: i64,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct NestClipPreviewParams {
    pub sequence_id: u64,
    pub revision: u64,
    pub clip_id: u64,
    pub name: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct PerformanceMatchPreviewParams {
    pub sequence_id: u64,
    pub revision: u64,
    pub video_clip_id: u64,
    pub song_clip_id: u64,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ColorPresetPreviewParams {
    pub sequence_id: u64,
    pub revision: u64,
    pub clip_id: u64,
    pub name: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct VideoEffectPreviewParams {
    pub sequence_id: u64,
    pub revision: u64,
    pub clip_id: u64,
    pub name: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct EffectParamPreviewParams {
    pub sequence_id: u64,
    pub revision: u64,
    pub clip_id: u64,
    pub effect_id: String,
    pub param_id: String,
    pub value: Value,
    #[serde(default)]
    pub clip_time_ticks: Option<i64>,
    #[serde(default)]
    pub edit_existing_keyframe: bool,
    #[serde(default)]
    pub remove_existing_keyframe: bool,
    #[serde(default)]
    pub interpolation: Option<String>,
    #[serde(default)]
    pub to_clip_time_ticks: Option<i64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RemoveEffectPreviewParams {
    pub sequence_id: u64,
    pub revision: u64,
    pub clip_id: u64,
    pub effect_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RequestPlanParams {
    /// A marker request, such as "Add a marker at the playhead called Opening".
    pub request: String,
}

impl FilmcraftMcp {
    async fn plan_request(&self, request: &str) -> Result<Value, String> {
        let before = self.run("project.inspect", json!({})).await.map_err(|e| e.to_string())?;
        let sequence = self.run("sequence.inspect", json!({})).await.map_err(|e| e.to_string())?;
        let after = self.run("project.inspect", json!({})).await.map_err(|e| e.to_string())?;
        for field in ["revision", "activeSequence", "name", "path"] {
            if before[field] != after[field] {
                return Err("project changed while planning; inspect again".into());
            }
        }
        crate::request_plan::plan(request, &after, &sequence)
    }

    async fn require_capability(&self, capability: &str) -> Result<(), String> {
        match self.backend.as_ref() {
            Backend::Headless(_) => Ok(()),
            Backend::Bridge(bridge) => {
                let capabilities = bridge
                    .call("controller.capabilities", json!({}))
                    .await
                    .map_err(|_| "development FilmCraft app is older than this controller; quit the app and relaunch the controller".to_string())?;
                if capabilities[capability] == true {
                    Ok(())
                } else {
                    Err(format!("development FilmCraft app does not support {capability}; quit the app and relaunch the controller"))
                }
            }
        }
    }
}

#[tool_router]
impl FilmcraftControllerMcp {
    #[tool(
        title = "Plan a supported edit request",
        description = "Turn a supported marker or clip-move request into a project-grounded, read-only plan with exact targets and arguments for a preview tool. Unsupported or ambiguous requests fail clearly; this tool never edits.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    async fn request_plan(&self, Parameters(p): Parameters<RequestPlanParams>) -> Result<CallToolResult, McpError> {
        Ok(match self.inner.plan_request(&p.request).await {
            Ok(v) => ok_json(&v),
            Err(e) => fail(e),
        })
    }

    #[tool(
        title = "Inspect FilmCraft project",
        description = "Read the project tree, active sequence, project path, dirty state, and revision. This controller profile cannot edit the project.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    async fn project_inspect(&self) -> Result<CallToolResult, McpError> {
        self.inner.project_inspect().await
    }

    #[tool(
        title = "Inspect active sequence",
        description = "Read tracks, clips, stable IDs, exact timeline positions, effects, transitions, markers, and selection for the active sequence.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    async fn sequence_inspect(&self) -> Result<CallToolResult, McpError> {
        self.inner.sequence_inspect().await
    }

    #[tool(
        title = "Inspect FilmCraft document",
        description = "Read the project tree and active sequence together, including project identity and revision.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    async fn doc_inspect(&self) -> Result<CallToolResult, McpError> {
        self.inner.doc_inspect().await
    }

    #[tool(
        title = "Discover FilmCraft commands",
        description = "Read the available FilmCraft command IDs, parameter descriptions, and enabled state. Commands are listed for explanation only; this profile cannot run them.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    async fn command_list(&self, Parameters(p): Parameters<ListParams>) -> Result<CallToolResult, McpError> {
        self.inner.command_list(Parameters(p)).await
    }

    #[tool(
        title = "Preview current frame",
        description = "Render the frame at the existing playhead. This read-only profile cannot move the playhead.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    async fn render_current_frame(&self, Parameters(p): Parameters<CurrentFrameParams>) -> Result<CallToolResult, McpError> {
        self.inner.render_frame(Parameters(RenderParams { seconds: None, max_side: p.max_side })).await
    }

    #[tool(
        title = "Inspect FilmCraft interface state",
        description = "Read the current workspace, playback, selection, and panel state. No UI input is sent.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    async fn ui_inspect(&self) -> Result<CallToolResult, McpError> {
        self.inner.ui_inspect().await
    }
}

const CONTROLLER_INSTRUCTIONS: &str = "FilmCraft read-only controller profile. Inspect project and sequence state, plan supported marker, clip-move, audio-placement, cross-dissolve, audio/video start-alignment, single-clip nesting, performance-matching, and color-preset requests without writing, discover commands, and preview the current frame. This server exposes no project-writing command, batch editor, media importer, or synthetic UI input. Treat object IDs and project revision as the grounding context for any advice; ask the user to make edits through an authorized editing interface.";

const MARKER_CONTROLLER_INSTRUCTIONS: &str = "FilmCraft bounded editing profile. Use request_plan for supported marker, clip-move, audio-placement, explicit audio-track mute, cross-dissolve, audio/video start-alignment, single-clip nesting, performance-matching, color-preset, standard video-effect, static slider/switch/menu effect-parameter, numeric effect-keyframe addition, existing numeric effect-keyframe value, removal, interpolation and moves, and effect-removal requests, then call the named preview tool with its exact arguments. Show the exact preview and obtain explicit user approval before calling the matching commit tool with its one-time token. Plans expire after five minutes and become stale after any project revision or active-sequence change. There is no generic command runner, media importer, or synthetic UI input.";

#[tool_router]
impl FilmcraftMarkerControllerMcp {
    #[tool(
        title = "Plan a supported edit request",
        description = "Turn a supported marker or clip-move request into a project-grounded, read-only plan with exact targets and arguments for a preview tool. This does not edit or create a commit token.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    async fn request_plan(&self, Parameters(p): Parameters<RequestPlanParams>) -> Result<CallToolResult, McpError> {
        Ok(match self.inner.plan_request(&p.request).await {
            Ok(v) => ok_json(&v),
            Err(e) => fail(e),
        })
    }

    #[tool(
        title = "Inspect FilmCraft project",
        description = "Read project tree, active sequence, dirty state and revision.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    async fn project_inspect(&self) -> Result<CallToolResult, McpError> {
        self.inner.project_inspect().await
    }

    #[tool(
        title = "Inspect active sequence",
        description = "Read tracks, clips and markers for the active sequence.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    async fn sequence_inspect(&self) -> Result<CallToolResult, McpError> {
        self.inner.sequence_inspect().await
    }

    #[tool(
        title = "Inspect FilmCraft document",
        description = "Read project tree and active sequence together.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    async fn doc_inspect(&self) -> Result<CallToolResult, McpError> {
        self.inner.doc_inspect().await
    }

    #[tool(
        title = "Discover FilmCraft commands",
        description = "Read available command IDs and parameter descriptions; this profile cannot execute them.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    async fn command_list(&self, Parameters(p): Parameters<ListParams>) -> Result<CallToolResult, McpError> {
        self.inner.command_list(Parameters(p)).await
    }

    #[tool(
        title = "Preview current frame",
        description = "Render the frame at the current playhead.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    async fn render_current_frame(&self, Parameters(p): Parameters<CurrentFrameParams>) -> Result<CallToolResult, McpError> {
        self.inner.render_frame(Parameters(RenderParams { seconds: None, max_side: p.max_side })).await
    }

    #[tool(
        title = "Inspect FilmCraft interface state",
        description = "Read current workspace, playback, selection and panel state.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    async fn ui_inspect(&self) -> Result<CallToolResult, McpError> {
        self.inner.ui_inspect().await
    }

    #[tool(
        title = "List built-in color presets",
        description = "Read names, folders and descriptions of FilmCraft's built-in Lumetri looks.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    async fn color_presets(&self) -> Result<CallToolResult, McpError> {
        wrap(self.inner.run("lumetri.presets", json!({})).await)
    }

    #[tool(
        title = "List standard video effects",
        description = "Read standard video effect names and IDs available for one-clip controller edits.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    async fn video_effects(&self) -> Result<CallToolResult, McpError> {
        let result = async {
            let list = self.inner.run("effects.list", json!({"kind":"Video"})).await.map_err(|e| e.to_string())?;
            crate::video_effect_plan::catalog(&list).map(|effects| json!({"effects":effects}))
        }
        .await;
        Ok(match result {
            Ok(v) => ok_json(&v),
            Err(e) => fail(e),
        })
    }

    #[tool(
        title = "Preview adding a marker",
        description = "Prepare (without applying) one marker at a time in seconds. Supply the current active sequence ID and project revision from inspection. Then present the returned action and exact values for user approval.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = false, open_world_hint = false)
    )]
    async fn marker_add_preview(&self, Parameters(p): Parameters<MarkerAddPreviewParams>) -> Result<CallToolResult, McpError> {
        let result = async {
            if !p.seconds.is_finite() || p.seconds < 0.0 || p.seconds > i64::MAX as f64 / filmcraft_time::TICKS_PER_SECOND as f64 { return Err("seconds must be finite and non-negative".to_string()); }
            if p.name.trim().is_empty() || p.name.chars().count() > 256 { return Err("name must contain 1–256 characters".into()); }
            let comment = p.comment.unwrap_or_default();
            if comment.chars().count() > 2048 { return Err("comment must be at most 2048 characters".into()); }
            let (revision, sequence_id, duration, _) = self.snapshot().await?;
            if revision != p.revision { return Err(format!("stale project revision: supplied {}, current {revision}", p.revision)); }
            if sequence_id != p.sequence_id { return Err(format!("active sequence changed: supplied {}, current {sequence_id}", p.sequence_id)); }
            let time = (p.seconds * filmcraft_time::TICKS_PER_SECOND as f64).round() as i64;
            if time > duration { return Err(format!("marker time exceeds active sequence duration ({duration} ticks)")); }
            let token = self.stash_plan(MarkerPlan::Add { sequence_id, revision, time, name: p.name.clone(), comment: comment.clone() })?;
            Ok(json!({"preview_only":true,"plan_token":token,"expires_in_seconds":300,"action":"add_marker","sequence_id":sequence_id,"revision":revision,"time_ticks":time,"seconds":time as f64 / filmcraft_time::TICKS_PER_SECOND as f64,"name":p.name,"comment":comment}))
        }.await;
        Ok(match result {
            Ok(v) => ok_json(&v),
            Err(e) => fail(e),
        })
    }

    #[tool(
        title = "Preview editing a marker",
        description = "Prepare (without applying) a name and/or comment change to one marker ID. Supply current active sequence ID and project revision. Then present the returned before/after values for user approval.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = false, open_world_hint = false)
    )]
    async fn marker_edit_preview(&self, Parameters(p): Parameters<MarkerEditPreviewParams>) -> Result<CallToolResult, McpError> {
        let result = async {
            if p.name.is_none() && p.comment.is_none() { return Err("supply name and/or comment".into()); }
            if p.name.as_ref().is_some_and(|s| s.trim().is_empty() || s.chars().count() > 256) { return Err("name must contain 1–256 characters".into()); }
            if p.comment.as_ref().is_some_and(|s| s.chars().count() > 2048) { return Err("comment must be at most 2048 characters".into()); }
            let (revision, sequence_id, _, sequence) = self.snapshot().await?;
            if revision != p.revision { return Err(format!("stale project revision: supplied {}, current {revision}", p.revision)); }
            if sequence_id != p.sequence_id { return Err(format!("active sequence changed: supplied {}, current {sequence_id}", p.sequence_id)); }
            let marker = sequence["markers"].as_array().and_then(|a| a.iter().find(|m| m["id"].as_u64() == Some(p.marker_id))).ok_or("marker ID not found in active sequence")?;
            let old_name = marker["name"].as_str().unwrap_or_default().to_string();
            let old_comment = marker["comment"].as_str().unwrap_or_default().to_string();
            let next_name = p.name.clone().unwrap_or_else(|| old_name.clone());
            let next_comment = p.comment.clone().unwrap_or_else(|| old_comment.clone());
            let token = self.stash_plan(MarkerPlan::Edit { sequence_id, revision, marker_id: p.marker_id, name: p.name.clone(), comment: p.comment.clone(), before_name: old_name.clone(), before_comment: old_comment.clone(), start: marker["start"].as_i64().unwrap_or_default() })?;
            Ok(json!({"preview_only":true,"plan_token":token,"expires_in_seconds":300,"action":"edit_marker","sequence_id":sequence_id,"revision":revision,"marker_id":p.marker_id,"before":{"name":old_name,"comment":old_comment},"after":{"name":next_name,"comment":next_comment}}))
        }.await;
        Ok(match result {
            Ok(v) => ok_json(&v),
            Err(e) => fail(e),
        })
    }

    #[tool(
        title = "Commit approved marker plan",
        description = "Apply exactly one previously previewed marker add/name/comment edit. The one-time token is consumed even on failure; revision and sequence identity are checked atomically by the engine, then the result is inspected and verified.",
        annotations(read_only_hint = false, destructive_hint = false, idempotent_hint = false, open_world_hint = false)
    )]
    async fn marker_commit(&self, Parameters(p): Parameters<MarkerCommitParams>) -> Result<CallToolResult, McpError> {
        let result = async {
            let plan = self.take_plan(&p.plan_token)?;
            let (sequence_id, revision, command, params, expected) = match plan {
                MarkerPlan::Add { sequence_id, revision, time, name, comment } => (
                    sequence_id, revision, "markers.add", json!({"target":"program","time":time,"name":name,"comment":comment,"expectedRevision":revision,"sequenceId":sequence_id}), json!({"name":name,"comment":comment,"start":time})
                ),
                MarkerPlan::Edit { sequence_id, revision, marker_id, name, comment, before_name, before_comment, start } => (
                    sequence_id, revision, "markers.edit", json!({"target":"program","marker":marker_id,"name":name,"comment":comment,"expectedRevision":revision,"sequenceId":sequence_id}), json!({"id":marker_id,"name":name.unwrap_or(before_name),"comment":comment.unwrap_or(before_comment),"start":start})
                ),
            };
            let (current_revision, current_sequence, _, _) = self.snapshot().await?;
            if current_revision != revision { return Err(format!("stale project revision: preview {revision}, current {current_revision}")); }
            if current_sequence != sequence_id { return Err(format!("active sequence changed: preview {sequence_id}, current {current_sequence}")); }
            let changed = self.inner.run(command, params).await.map_err(|e| e.to_string())?;
            let marker_id = if command == "markers.add" { changed["marker"].as_u64().ok_or("engine did not return the new marker ID")? } else { expected["id"].as_u64().ok_or("preview missing marker ID")? };
            let (after_revision, after_sequence, _, sequence_after) = self.snapshot().await?;
            if after_sequence != sequence_id { return Err(format!("edit applied but verification found active sequence {after_sequence} instead of {sequence_id}")); }
            let marker = sequence_after["markers"].as_array().and_then(|a| a.iter().find(|m| m["id"].as_u64() == Some(marker_id))).ok_or("edit applied but marker was absent during verification")?;
            if expected["name"].as_str().is_some_and(|s| marker["name"].as_str() != Some(s)) || expected["comment"].as_str().is_some_and(|s| marker["comment"].as_str() != Some(s)) || expected["start"].as_i64().is_some_and(|t| marker["start"].as_i64() != Some(t)) {
                return Err("edit applied but marker values did not match the approved preview".into());
            }
            Ok(json!({"committed":true,"verified":true,"marker_id":marker_id,"sequence_id":sequence_id,"revision_before":revision,"revision_after":after_revision,"marker":marker}))
        }.await;
        Ok(match result {
            Ok(v) => ok_json(&v),
            Err(e) => fail(e),
        })
    }

    #[tool(
        title = "Preview moving one clip and its linked partners",
        description = "Resolve one clip ID and its linked partners, then preview their exact new timeline positions without editing. This first move slice rejects collisions, attached transitions, locked tracks, and negative positions.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = false, open_world_hint = false)
    )]
    async fn clip_move_preview(&self, Parameters(p): Parameters<ClipMovePreviewParams>) -> Result<CallToolResult, McpError> {
        let result = async {
            self.inner.require_capability("guarded_clip_move").await?;
            let (revision, sequence_id, _, sequence) = self.snapshot().await?;
            if revision != p.revision {
                return Err(format!("stale project revision: supplied {}, current {revision}", p.revision));
            }
            if sequence_id != p.sequence_id {
                return Err(format!("active sequence changed: supplied {}, current {sequence_id}", p.sequence_id));
            }
            let moves = crate::clip_move_plan::resolve(&sequence, p.clip_id, p.seconds)?;
            let token = self.stash_move_plan(ClipMovePlan { sequence_id, revision, moves: moves.clone(), action: "move_clip", reference_video: None })?;
            Ok(json!({"preview_only":true,"plan_token":token,"expires_in_seconds":300,"action":"move_clip",
                "sequence_id":sequence_id,"revision":revision,"clip_id":p.clip_id,"affected":moves,"undo":"edit.undo"}))
        }
        .await;
        Ok(match result {
            Ok(v) => ok_json(&v),
            Err(e) => fail(e),
        })
    }

    #[tool(
        title = "Preview aligning audio to video start",
        description = "Preview moving one unlinked audio clip to the exact start of a named video clip. Reject collisions, attached transitions, locked tracks, and already aligned clips.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = false, open_world_hint = false)
    )]
    async fn audio_align_preview(&self, Parameters(p): Parameters<AudioAlignPreviewParams>) -> Result<CallToolResult, McpError> {
        let result = async {
            self.inner.require_capability("guarded_clip_move").await?;
            let (revision, sequence_id, _, sequence) = self.snapshot().await?;
            if revision != p.revision {
                return Err(format!("stale project revision: supplied {}, current {revision}", p.revision));
            }
            if sequence_id != p.sequence_id {
                return Err(format!("active sequence changed: supplied {}, current {sequence_id}", p.sequence_id));
            }
            let target = crate::audio_video_align_plan::resolve(&sequence, p.audio_clip_id, p.video_clip_id)?;
            let token = self.stash_move_plan(ClipMovePlan {
                sequence_id,
                revision,
                moves: target.affected.clone(),
                action: "align_audio_to_video",
                reference_video: Some((p.video_clip_id, target.anchor_ticks)),
            })?;
            Ok(json!({"preview_only":true,"plan_token":token,"expires_in_seconds":300,"action":"align_audio_to_video",
                "sequence_id":sequence_id,"revision":revision,"target":target,"undo":"edit.undo"}))
        }
        .await;
        Ok(match result {
            Ok(v) => ok_json(&v),
            Err(e) => fail(e),
        })
    }

    #[tool(
        title = "Commit approved clip move",
        description = "Apply one previewed clip move and linked partners as one native undo step, with atomic revision and sequence guards, then verify all clip positions. The token is one-time and expires in five minutes.",
        annotations(read_only_hint = false, destructive_hint = false, idempotent_hint = false, open_world_hint = false)
    )]
    async fn clip_move_commit(&self, Parameters(p): Parameters<MarkerCommitParams>) -> Result<CallToolResult, McpError> {
        let result = async {
            let plan = self.take_move_plan(&p.plan_token)?;
            self.inner.require_capability("guarded_clip_move").await?;
            let (revision, sequence_id, _, _) = self.snapshot().await?;
            if revision != plan.revision {
                return Err(format!("stale project revision: preview {}, current {revision}", plan.revision));
            }
            if sequence_id != plan.sequence_id {
                return Err(format!("active sequence changed: preview {}, current {sequence_id}", plan.sequence_id));
            }
            let moves: Vec<Value> = plan.moves.iter().map(|m| json!({"clip":m.clip_id,"track":m.track_id,"time":m.to_ticks})).collect();
            self.inner
                .run(
                    "timeline.move",
                    json!({"moves":moves,"linked":false,"insert":false,
                "expectedRevision":plan.revision,"sequenceId":plan.sequence_id}),
                )
                .await
                .map_err(|e| e.to_string())?;
            let (_, after_sequence_id, _, sequence) = self.snapshot().await?;
            if after_sequence_id != plan.sequence_id {
                return Err("move applied but active sequence changed during verification".into());
            }
            for moved in &plan.moves {
                let found = ["video", "audio"].iter().filter_map(|field| sequence[*field].as_array()).flat_map(|tracks| tracks.iter()).find_map(|track| {
                    track["items"].as_array().and_then(|items| items.iter().find(|item| item["clip"].as_u64() == Some(moved.clip_id)).map(|item| (track, item)))
                });
                let Some((track, item)) = found else {
                    return Err(format!("move applied but clip #{} was absent during verification", moved.clip_id));
                };
                if track["id"].as_u64() != Some(moved.track_id) || item["start"].as_i64() != Some(moved.to_ticks) {
                    return Err(format!("move applied but clip #{} did not reach its previewed position", moved.clip_id));
                }
            }
            if let Some((video_id, anchor)) = plan.reference_video {
                let video = sequence["video"]
                    .as_array()
                    .and_then(|tracks| {
                        tracks
                            .iter()
                            .filter_map(|track| track["items"].as_array())
                            .flat_map(|items| items.iter())
                            .find(|clip| clip["clip"].as_u64() == Some(video_id))
                    })
                    .ok_or("alignment applied but reference video was absent during verification")?;
                if video["start"].as_i64() != Some(anchor) {
                    return Err("alignment applied but reference video moved during verification".into());
                }
            }
            Ok(json!({"committed":true,"verified":true,"action":plan.action,"affected":plan.moves,"undo":"edit.undo"}))
        }
        .await;
        Ok(match result {
            Ok(v) => ok_json(&v),
            Err(e) => fail(e),
        })
    }

    #[tool(
        title = "Preview deleting one clip and linked partners",
        description = "Identify one clip by ID and preview the exact linked clips removed. The timeline gap remains. Reject locked tracks and attached transitions; no edit is made.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = false, open_world_hint = false)
    )]
    async fn clip_delete_preview(&self, Parameters(p): Parameters<ClipDeletePreviewParams>) -> Result<CallToolResult, McpError> {
        let result = async {
            self.inner.require_capability("guarded_clip_delete").await?;
            let (revision, sequence_id, _, sequence) = self.snapshot().await?;
            if revision != p.revision {
                return Err(format!("stale project revision: supplied {}, current {revision}", p.revision));
            }
            if sequence_id != p.sequence_id {
                return Err(format!("active sequence changed: supplied {}, current {sequence_id}", p.sequence_id));
            }
            let affected = crate::clip_delete_plan::resolve(&sequence, p.clip_id)?;
            let token = self.stash_delete_plan(ClipDeletePlan { sequence_id, revision, clip_id: p.clip_id, affected: affected.clone() })?;
            Ok(json!({"preview_only":true,"plan_token":token,"expires_in_seconds":300,
                "action":"delete_clip","sequence_id":sequence_id,"revision":revision,"clip_id":p.clip_id,
                "affected":affected,"gap_preserved":true,"undo":"edit.undo"}))
        }
        .await;
        Ok(match result {
            Ok(v) => ok_json(&v),
            Err(e) => fail(e),
        })
    }

    #[tool(
        title = "Commit approved clip deletion",
        description = "Delete the exact previewed clip and linked partners while preserving the gap, with atomic revision and sequence guards. Verify no other sequence content changed. Token is one-time and expires after five minutes.",
        annotations(read_only_hint = false, destructive_hint = true, idempotent_hint = false, open_world_hint = false)
    )]
    async fn clip_delete_commit(&self, Parameters(p): Parameters<MarkerCommitParams>) -> Result<CallToolResult, McpError> {
        let result = async {
            let plan = self.take_delete_plan(&p.plan_token)?;
            self.inner.require_capability("guarded_clip_delete").await?;
            let (revision, sequence_id, _, before) = self.snapshot().await?;
            if revision != plan.revision {
                return Err(format!("stale project revision: preview {}, current {revision}", plan.revision));
            }
            if sequence_id != plan.sequence_id {
                return Err(format!("active sequence changed: preview {}, current {sequence_id}", plan.sequence_id));
            }
            let current = crate::clip_delete_plan::resolve(&before, plan.clip_id)?;
            if serde_json::to_value(&current).map_err(|e| e.to_string())? != serde_json::to_value(&plan.affected).map_err(|e| e.to_string())? {
                return Err("clip deletion target changed since preview".into());
            }
            let ids: Vec<u64> = plan.affected.iter().map(|clip| clip.clip_id).collect();
            self.inner
                .run(
                    "edit.clear",
                    json!({"controllerClipDelete":true,"clip":plan.clip_id,
                "clips":ids,"expectedRevision":plan.revision,"sequenceId":plan.sequence_id}),
                )
                .await
                .map_err(|e| e.to_string())?;
            let (_, after_sequence_id, _, after) = self.snapshot().await?;
            if after_sequence_id != plan.sequence_id {
                return Err("deletion applied but active sequence changed during verification".into());
            }
            for field in ["video", "audio"] {
                let before_tracks = before[field].as_array().ok_or("sequence tracks unavailable before deletion")?;
                let after_tracks = after[field].as_array().ok_or("sequence tracks unavailable after deletion")?;
                if before_tracks.len() != after_tracks.len() {
                    return Err("deletion changed the track count".into());
                }
                for (old, new) in before_tracks.iter().zip(after_tracks) {
                    let mut expected = old.clone();
                    expected["items"]
                        .as_array_mut()
                        .ok_or("clip list unavailable before deletion")?
                        .retain(|item| !ids.iter().any(|id| item["clip"].as_u64() == Some(*id)));
                    if expected != *new {
                        return Err("deletion changed clips or track content outside the preview".into());
                    }
                }
            }
            if before["markers"] != after["markers"] || before["settings"] != after["settings"] {
                return Err("deletion changed markers or sequence settings".into());
            }
            Ok(json!({"committed":true,"verified":true,"action":"delete_clip",
                "clip_id":plan.clip_id,"affected":plan.affected,"gap_preserved":true,"undo":"edit.undo"}))
        }
        .await;
        Ok(match result {
            Ok(v) => ok_json(&v),
            Err(e) => fail(e),
        })
    }

    #[tool(
        title = "Preview placing one audio item",
        description = "Resolve an audio-only project item, an audio track, and a timeline time. Reject overlaps and locked tracks. The preview does not edit.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = false, open_world_hint = false)
    )]
    async fn audio_place_preview(&self, Parameters(p): Parameters<AudioPlacePreviewParams>) -> Result<CallToolResult, McpError> {
        let result = async {
            self.inner.require_capability("guarded_audio_place").await?;
            let project = self.inner.run("project.inspect", json!({})).await.map_err(|e| e.to_string())?;
            let (revision, sequence_id, _, sequence) = self.snapshot().await?;
            if revision != p.revision || project["revision"].as_u64() != Some(revision) {
                return Err(format!("stale project revision: supplied {}, current {revision}", p.revision));
            }
            if sequence_id != p.sequence_id || project["activeSequence"].as_u64() != Some(sequence_id) {
                return Err(format!("active sequence changed: supplied {}, current {sequence_id}", p.sequence_id));
            }
            let placement = crate::audio_place_plan::resolve(&project, &sequence, p.item_id, &p.track, p.seconds)?;
            let token = self.stash_audio_plan(AudioPlacePlan { sequence_id, revision, placement: placement.clone() })?;
            Ok(json!({"preview_only":true,"plan_token":token,"expires_in_seconds":300,"action":"place_audio",
                "sequence_id":sequence_id,"revision":revision,"target":placement,"undo":"edit.undo"}))
        }
        .await;
        Ok(match result {
            Ok(v) => ok_json(&v),
            Err(e) => fail(e),
        })
    }

    #[tool(
        title = "Commit approved audio placement",
        description = "Place one previewed audio-only item as one native undo step with atomic revision and sequence guards, then verify its track, source, time, and duration. The token is one-time and expires in five minutes.",
        annotations(read_only_hint = false, destructive_hint = false, idempotent_hint = false, open_world_hint = false)
    )]
    async fn audio_place_commit(&self, Parameters(p): Parameters<MarkerCommitParams>) -> Result<CallToolResult, McpError> {
        let result = async {
            let plan = self.take_audio_plan(&p.plan_token)?;
            self.inner.require_capability("guarded_audio_place").await?;
            let (revision, sequence_id, _, _) = self.snapshot().await?;
            if revision != plan.revision {
                return Err(format!("stale project revision: preview {}, current {revision}", plan.revision));
            }
            if sequence_id != plan.sequence_id {
                return Err(format!("active sequence changed: preview {}, current {sequence_id}", plan.sequence_id));
            }
            let target = &plan.placement;
            let placed = self
                .inner
                .run(
                    "timeline.place",
                    json!({"item":target.item_id,"track":target.track_id,
                "time":target.time_ticks,"sourceIn":0,"duration":target.duration_ticks,"insert":false,
                "controllerSingleAudioClip":true,"expectedRevision":plan.revision,"sequenceId":plan.sequence_id}),
                )
                .await
                .map_err(|e| e.to_string())?;
            let ids = placed["clips"].as_array().ok_or("placement returned no clip list")?;
            if ids.len() != 1 {
                return Err("placement returned an unexpected number of clips; inspect the sequence".into());
            }
            let clip_id = ids[0].as_u64().ok_or("placement returned an invalid clip ID")?;
            let (_, after_sequence_id, _, sequence) = self.snapshot().await?;
            if after_sequence_id != plan.sequence_id {
                return Err("placement applied but active sequence changed during verification".into());
            }
            let track = sequence["audio"]
                .as_array()
                .and_then(|tracks| tracks.iter().find(|track| track["id"].as_u64() == Some(target.track_id)))
                .ok_or("placement applied but audio track was absent during verification")?;
            let clip = track["items"]
                .as_array()
                .and_then(|items| items.iter().find(|item| item["clip"].as_u64() == Some(clip_id)))
                .ok_or("placement applied but clip was absent during verification")?;
            if clip["item"].as_u64() != Some(target.item_id)
                || clip["start"].as_i64() != Some(target.time_ticks)
                || clip["duration"].as_i64() != Some(target.duration_ticks)
            {
                return Err("placement applied but clip did not match the approved preview".into());
            }
            Ok(json!({"committed":true,"verified":true,"action":"place_audio","clip_id":clip_id,
                "target":target,"undo":"edit.undo"}))
        }
        .await;
        Ok(match result {
            Ok(v) => ok_json(&v),
            Err(e) => fail(e),
        })
    }

    #[tool(
        title = "Preview audio track mute",
        description = "Preview muting or unmuting one explicit audio track, including its current state and number of clips. No edit is made.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = false, open_world_hint = false)
    )]
    async fn audio_track_mute_preview(&self, Parameters(p): Parameters<AudioTrackMutePreviewParams>) -> Result<CallToolResult, McpError> {
        let result = async {
            self.inner.require_capability("guarded_audio_track_mute").await?;
            let (revision, sequence_id, _, sequence) = self.snapshot().await?;
            if revision != p.revision {
                return Err(format!("stale project revision: supplied {}, current {revision}", p.revision));
            }
            if sequence_id != p.sequence_id {
                return Err(format!("active sequence changed: supplied {}, current {sequence_id}", p.sequence_id));
            }
            let target = crate::audio_track_mute_plan::resolve(&sequence, &p.track, p.muted)?;
            let token = self.stash_audio_mute_plan(AudioTrackMutePlan { sequence_id, revision, target: target.clone() })?;
            Ok(json!({"preview_only":true,"plan_token":token,"expires_in_seconds":300,
                "action":"set_audio_track_mute","sequence_id":sequence_id,"revision":revision,
                "target":target,"undo":"edit.undo"}))
        }
        .await;
        Ok(match result {
            Ok(v) => ok_json(&v),
            Err(e) => fail(e),
        })
    }

    #[tool(
        title = "Commit approved audio track mute",
        description = "Set the approved audio track mute state in one native undo step with atomic revision, sequence and prior-state checks, then verify the exact track. Token expires after five minutes and is one-time.",
        annotations(read_only_hint = false, destructive_hint = false, idempotent_hint = false, open_world_hint = false)
    )]
    async fn audio_track_mute_commit(&self, Parameters(p): Parameters<MarkerCommitParams>) -> Result<CallToolResult, McpError> {
        let result = async {
            let plan = self.take_audio_mute_plan(&p.plan_token)?;
            self.inner.require_capability("guarded_audio_track_mute").await?;
            let (revision, sequence_id, _, _) = self.snapshot().await?;
            if revision != plan.revision {
                return Err(format!("stale project revision: preview {}, current {revision}", plan.revision));
            }
            if sequence_id != plan.sequence_id {
                return Err(format!("active sequence changed: preview {}, current {sequence_id}", plan.sequence_id));
            }
            let target = &plan.target;
            self.inner
                .run(
                    "timeline.setTrack",
                    json!({"track":target.track_id,"muted":target.after_muted,
                "controllerAudioTrackMute":true,"expectedMuted":target.before_muted,
                "expectedRevision":plan.revision,"sequenceId":plan.sequence_id}),
                )
                .await
                .map_err(|e| e.to_string())?;
            let (_, after_sequence_id, _, sequence) = self.snapshot().await?;
            if after_sequence_id != plan.sequence_id {
                return Err("track mute applied but active sequence changed during verification".into());
            }
            let track = sequence["audio"]
                .as_array()
                .and_then(|tracks| tracks.iter().find(|track| track["id"].as_u64() == Some(target.track_id)))
                .ok_or("track mute applied but audio track was absent during verification")?;
            if track["muted"].as_bool() != Some(target.after_muted)
                || track["name"].as_str() != Some(target.track_name.as_str())
                || track["items"].as_array().map(Vec::len) != Some(target.clip_count)
            {
                return Err("track mute applied but track did not match the approved preview".into());
            }
            Ok(json!({"committed":true,"verified":true,"action":"set_audio_track_mute",
                "target":target,"undo":"edit.undo"}))
        }
        .await;
        Ok(match result {
            Ok(v) => ok_json(&v),
            Err(e) => fail(e),
        })
    }

    #[tool(
        title = "Preview a cross dissolve",
        description = "Resolve a video clip's outgoing cut and preview one cross dissolve of 2–60 even frames without editing. Reject a locked track, nonadjacent clips, or an overlapping transition.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = false, open_world_hint = false)
    )]
    async fn transition_add_preview(&self, Parameters(p): Parameters<TransitionAddPreviewParams>) -> Result<CallToolResult, McpError> {
        let result = async {
            self.inner.require_capability("guarded_transition_add").await?;
            let (revision, sequence_id, _, sequence) = self.snapshot().await?;
            if revision != p.revision {
                return Err(format!("stale project revision: supplied {}, current {revision}", p.revision));
            }
            if sequence_id != p.sequence_id {
                return Err(format!("active sequence changed: supplied {}, current {sequence_id}", p.sequence_id));
            }
            let target = crate::transition_add_plan::resolve(&sequence, p.clip_id, p.frames)?;
            let token = self.stash_transition_plan(TransitionAddPlan { sequence_id, revision, target: target.clone() })?;
            Ok(json!({"preview_only":true,"plan_token":token,"expires_in_seconds":300,"action":"add_cross_dissolve",
                "sequence_id":sequence_id,"revision":revision,"target":target,"undo":"edit.undo"}))
        }
        .await;
        Ok(match result {
            Ok(v) => ok_json(&v),
            Err(e) => fail(e),
        })
    }

    #[tool(
        title = "Commit approved cross dissolve",
        description = "Apply the previewed cross dissolve with atomic revision and sequence guards, then verify the effect, clips, time, and duration. The token is one-time and expires in five minutes.",
        annotations(read_only_hint = false, destructive_hint = false, idempotent_hint = false, open_world_hint = false)
    )]
    async fn transition_add_commit(&self, Parameters(p): Parameters<MarkerCommitParams>) -> Result<CallToolResult, McpError> {
        let result = async {
            let plan = self.take_transition_plan(&p.plan_token)?;
            self.inner.require_capability("guarded_transition_add").await?;
            let (revision, sequence_id, _, _) = self.snapshot().await?;
            if revision != plan.revision {
                return Err(format!("stale project revision: preview {}, current {revision}", plan.revision));
            }
            if sequence_id != plan.sequence_id {
                return Err(format!("active sequence changed: preview {}, current {sequence_id}", plan.sequence_id));
            }
            let target = &plan.target;
            let applied = self
                .inner
                .run(
                    "sequence.applyVideoTransition",
                    json!({"clip":target.clip_id,"edge":"out",
                "effect":target.effect,"frames":target.frames,"expectedRevision":plan.revision,"sequenceId":plan.sequence_id}),
                )
                .await
                .map_err(|e| e.to_string())?;
            let transition_id = applied["transition"].as_u64().ok_or("transition command returned no ID")?;
            let (_, after_sequence_id, _, sequence) = self.snapshot().await?;
            if after_sequence_id != plan.sequence_id {
                return Err("transition applied but active sequence changed during verification".into());
            }
            let track = sequence["video"]
                .as_array()
                .and_then(|tracks| tracks.iter().find(|track| track["id"].as_u64() == Some(target.track_id)))
                .ok_or("transition applied but video track was absent during verification")?;
            let transition = track["transitions"]
                .as_array()
                .and_then(|items| items.iter().find(|item| item["id"].as_u64() == Some(transition_id)))
                .ok_or("transition applied but was absent during verification")?;
            if transition["effect"].as_str() != Some(target.effect)
                || transition["from"].as_u64() != Some(target.clip_id)
                || transition["to"].as_u64() != Some(target.next_clip_id)
                || transition["start"].as_i64() != Some(target.start_ticks)
                || transition["duration"].as_i64() != Some(target.duration_ticks)
            {
                return Err("transition applied but did not match the approved preview".into());
            }
            Ok(json!({"committed":true,"verified":true,"action":"add_cross_dissolve",
                "transition_id":transition_id,"target":target,"undo":"edit.undo"}))
        }
        .await;
        Ok(match result {
            Ok(v) => ok_json(&v),
            Err(e) => fail(e),
        })
    }

    #[tool(
        title = "Preview nesting one video clip",
        description = "Preview replacing one unlinked video clip with a named nested sequence. Reject an attached transition, locked track, sequence source, or duplicate project name. The preview does not edit.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = false, open_world_hint = false)
    )]
    async fn nest_clip_preview(&self, Parameters(p): Parameters<NestClipPreviewParams>) -> Result<CallToolResult, McpError> {
        let result = async {
            self.inner.require_capability("guarded_single_nest").await?;
            let project = self.inner.run("project.inspect", json!({})).await.map_err(|e| e.to_string())?;
            let (revision, sequence_id, _, sequence) = self.snapshot().await?;
            if revision != p.revision || project["revision"].as_u64() != Some(revision) {
                return Err(format!("stale project revision: supplied {}, current {revision}", p.revision));
            }
            if sequence_id != p.sequence_id || project["activeSequence"].as_u64() != Some(sequence_id) {
                return Err(format!("active sequence changed: supplied {}, current {sequence_id}", p.sequence_id));
            }
            let target = crate::nest_clip_plan::resolve(&project, &sequence, p.clip_id, &p.name)?;
            let token = self.stash_nest_plan(NestClipPlan { sequence_id, revision, target: target.clone() })?;
            Ok(json!({"preview_only":true,"plan_token":token,"expires_in_seconds":300,"action":"nest_clip",
                "sequence_id":sequence_id,"revision":revision,"target":target,"undo":"edit.undo"}))
        }
        .await;
        Ok(match result {
            Ok(v) => ok_json(&v),
            Err(e) => fail(e),
        })
    }

    #[tool(
        title = "Commit approved video clip nest",
        description = "Nest one previewed unlinked video clip as a native undo step with atomic revision and sequence guards, then verify the parent replacement and nested contents. The token is one-time and expires in five minutes.",
        annotations(read_only_hint = false, destructive_hint = false, idempotent_hint = false, open_world_hint = false)
    )]
    async fn nest_clip_commit(&self, Parameters(p): Parameters<MarkerCommitParams>) -> Result<CallToolResult, McpError> {
        let result = async {
            let plan = self.take_nest_plan(&p.plan_token)?;
            self.inner.require_capability("guarded_single_nest").await?;
            let (revision, sequence_id, _, _) = self.snapshot().await?;
            if revision != plan.revision {
                return Err(format!("stale project revision: preview {}, current {revision}", plan.revision));
            }
            if sequence_id != plan.sequence_id {
                return Err(format!("active sequence changed: preview {}, current {sequence_id}", plan.sequence_id));
            }
            let target = &plan.target;
            let applied = self
                .inner
                .run(
                    "clip.nest",
                    json!({"name":target.nested_name,"clips":[target.clip_id],
                "controllerSingleNest":true,"expectedRevision":plan.revision,"sequenceId":plan.sequence_id}),
                )
                .await
                .map_err(|e| e.to_string())?;
            let nested_id = applied["sequence"].as_u64().ok_or("nest command returned no sequence ID")?;
            let (_, after_sequence_id, _, parent) = self.snapshot().await?;
            if after_sequence_id != plan.sequence_id {
                return Err("nest applied but active sequence changed during verification".into());
            }
            let track = parent["video"]
                .as_array()
                .and_then(|tracks| tracks.iter().find(|track| track["id"].as_u64() == Some(target.track_id)))
                .ok_or("nest applied but video track was absent during verification")?;
            let items = track["items"].as_array().ok_or("nest applied but track clips were unavailable")?;
            if items.iter().any(|clip| clip["clip"].as_u64() == Some(target.clip_id)) {
                return Err("nest applied but original clip remained in parent sequence".into());
            }
            let replacement =
                items.iter().find(|clip| clip["item"].as_u64() == Some(nested_id)).ok_or("nest applied but replacement clip was absent during verification")?;
            if replacement["start"].as_i64() != Some(target.start_ticks) || replacement["duration"].as_i64() != Some(target.duration_ticks) {
                return Err("nest applied but replacement clip did not match the preview".into());
            }
            let replacement_clip_id = replacement["clip"].as_u64().ok_or("replacement clip ID unavailable")?;
            let nested = self.inner.run("sequence.inspect", json!({"item":nested_id})).await.map_err(|e| e.to_string())?;
            if nested["id"].as_u64() != Some(nested_id) || nested["name"].as_str() != Some(target.nested_name.as_str()) {
                return Err("nest applied but nested sequence identity did not match the preview".into());
            }
            let inside = nested["video"]
                .as_array()
                .and_then(|tracks| {
                    tracks
                        .iter()
                        .filter_map(|track| track["items"].as_array())
                        .flat_map(|items| items.iter())
                        .find(|clip| clip["clip"].as_u64() == Some(target.clip_id))
                })
                .ok_or("nest applied but original clip was absent from nested sequence")?;
            if inside["item"].as_u64() != Some(target.item_id)
                || inside["start"].as_i64() != Some(0)
                || inside["duration"].as_i64() != Some(target.duration_ticks)
            {
                return Err("nest applied but nested clip did not match the preview".into());
            }
            Ok(json!({"committed":true,"verified":true,"action":"nest_clip","nested_sequence_id":nested_id,
                "replacement_clip_id":replacement_clip_id,"target":target,"undo":"edit.undo"}))
        }
        .await;
        Ok(match result {
            Ok(v) => ok_json(&v),
            Err(e) => fail(e),
        })
    }

    #[tool(
        title = "Preview performance matching",
        description = "Analyze one named live video clip against a separate studio song clip, each 2–30 seconds. Return exact anchors, similarity, affected clips, and a one-time token without editing.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = false, open_world_hint = false)
    )]
    async fn performance_match_preview(&self, Parameters(p): Parameters<PerformanceMatchPreviewParams>) -> Result<CallToolResult, McpError> {
        let result = async {
            self.inner.require_capability("guarded_performance_match").await?;
            let (revision, sequence_id, _, sequence) = self.snapshot().await?;
            if revision != p.revision {
                return Err(format!("stale project revision: supplied {}, current {revision}", p.revision));
            }
            if sequence_id != p.sequence_id {
                return Err(format!("active sequence changed: supplied {}, current {sequence_id}", p.sequence_id));
            }
            let target = crate::performance_match_plan::resolve(&sequence, p.video_clip_id, p.song_clip_id)?;
            let song_before = sequence["audio"]
                .as_array()
                .and_then(|tracks| tracks.iter().find(|track| track["id"].as_u64() == Some(target.song_track_id)))
                .and_then(|track| track["items"].as_array())
                .and_then(|items| items.iter().find(|item| item["clip"].as_u64() == Some(target.song_clip_id)))
                .cloned()
                .ok_or("studio song unavailable")?;
            let analysis = self.inner.run("clip.matchPerformance", json!({"clips":[p.video_clip_id,p.song_clip_id]})).await.map_err(|e| e.to_string())?;
            if analysis["sequence"].as_u64() != Some(sequence_id)
                || analysis["video"]["id"].as_u64() != Some(target.video_clip_id)
                || analysis["song"]["id"].as_u64() != Some(target.song_clip_id)
            {
                return Err("analysis returned different source clips or sequence".into());
            }
            let points = analysis["points"].as_array().ok_or("analysis returned no timing points")?;
            if !(2..=128).contains(&points.len()) {
                return Err("analysis returned an unsupported number of timing points".into());
            }
            let similarity =
                analysis["similarity"].as_f64().filter(|v| v.is_finite() && (0.0..=1.0).contains(v)).ok_or("analysis returned invalid similarity")?;
            if points.first().and_then(|p| p["song"].as_i64()) != Some(0)
                || points.first().and_then(|p| p["live"].as_i64()) != Some(0)
                || points.last().and_then(|p| p["song"].as_i64()) != Some(target.song_duration_ticks)
                || points.last().and_then(|p| p["live"].as_i64()) != Some(target.video_duration_ticks)
            {
                return Err("analysis timing endpoints do not match the source clips".into());
            }
            let frame_num = sequence["settings"]["frame_rate"]["num"].as_i64().filter(|v| *v > 0).ok_or("sequence frame rate unavailable")?;
            let frame_den = sequence["settings"]["frame_rate"]["den"].as_i64().filter(|v| *v > 0).ok_or("sequence frame rate unavailable")?;
            for pair in points.windows(2) {
                let a = pair.first().ok_or("invalid timing points")?;
                let b = pair.get(1).ok_or("invalid timing points")?;
                let song_delta = b["song"].as_i64().and_then(|v| v.checked_sub(a["song"].as_i64()?)).ok_or("invalid song interval")?;
                let live_delta = b["live"].as_i64().and_then(|v| v.checked_sub(a["live"].as_i64()?)).ok_or("invalid live interval")?;
                if song_delta <= 0
                    || live_delta <= 0
                    || i128::from(song_delta) * i128::from(frame_num) < i128::from(filmcraft_time::TICKS_PER_SECOND) * i128::from(frame_den)
                    || !(0.5..=2.0).contains(&(live_delta as f64 / song_delta as f64))
                {
                    return Err("analysis requires a section outside 50–200% speed; trim the clips more closely".into());
                }
            }
            let (after_revision, after_sequence, _, _) = self.snapshot().await?;
            if after_revision != revision || after_sequence != sequence_id {
                return Err("project changed during analysis; preview again".into());
            }
            let token =
                self.stash_performance_plan(PerformanceMatchPlan { sequence_id, revision, target: target.clone(), analysis: analysis.clone(), song_before })?;
            Ok(json!({"preview_only":true,"plan_token":token,"expires_in_seconds":300,"action":"match_performance",
                "sequence_id":sequence_id,"revision":revision,"target":target,"analysis":{
                    "similarity":similarity,"points":points,"piece_count":points.len()-1,
                    "video_replacement_start_ticks":target.song_start_ticks,
                    "video_replacement_duration_ticks":target.song_duration_ticks},"undo":"edit.undo"}))
        }
        .await;
        Ok(match result {
            Ok(v) => ok_json(&v),
            Err(e) => fail(e),
        })
    }

    #[tool(
        title = "Commit approved performance match",
        description = "Apply the exact analyzed timing points as one native undo step, with atomic revision and sequence guards. Verify replacement video pieces, unchanged studio song, and muted linked live audio. The token is one-time.",
        annotations(read_only_hint = false, destructive_hint = false, idempotent_hint = false, open_world_hint = false)
    )]
    async fn performance_match_commit(&self, Parameters(p): Parameters<MarkerCommitParams>) -> Result<CallToolResult, McpError> {
        let result = async {
            let plan = self.take_performance_plan(&p.plan_token)?;
            self.inner.require_capability("guarded_performance_match").await?;
            let (revision, sequence_id, _, _) = self.snapshot().await?;
            if revision != plan.revision {
                return Err(format!("stale project revision: preview {}, current {revision}", plan.revision));
            }
            if sequence_id != plan.sequence_id {
                return Err(format!("active sequence changed: preview {}, current {sequence_id}", plan.sequence_id));
            }
            let applied = self
                .inner
                .run(
                    "clip.matchPerformance.apply",
                    json!({"plan":plan.analysis,
                "muteLive":plan.target.mute_live,"expectedRevision":plan.revision,"sequenceId":plan.sequence_id}),
                )
                .await
                .map_err(|e| e.to_string())?;
            let ids = applied["clips"].as_array().ok_or("performance match returned no clips")?;
            let points = plan.analysis["points"].as_array().ok_or("saved timing points unavailable")?;
            if ids.len() != points.len().saturating_sub(1) {
                return Err("performance match returned unexpected clip count".into());
            }
            let (_, after_sequence_id, _, sequence) = self.snapshot().await?;
            if after_sequence_id != plan.sequence_id {
                return Err("performance match applied but active sequence changed during verification".into());
            }
            let track = sequence["video"]
                .as_array()
                .and_then(|tracks| tracks.iter().find(|track| track["id"].as_u64() == Some(plan.target.video_track_id)))
                .ok_or("video track missing after performance match")?;
            let video_items = track["items"].as_array().ok_or("video clips unavailable after performance match")?;
            if video_items.iter().any(|item| item["clip"].as_u64() == Some(plan.target.video_clip_id)) {
                return Err("original live video remained after performance match".into());
            }
            for (index, pair) in points.windows(2).enumerate() {
                let id = ids.get(index).and_then(Value::as_u64).ok_or("invalid replacement clip ID")?;
                let item = video_items.iter().find(|item| item["clip"].as_u64() == Some(id)).ok_or("replacement video clip missing")?;
                let a = pair.first().ok_or("saved timing point unavailable")?;
                let b = pair.get(1).ok_or("saved timing point unavailable")?;
                let song_start = a["song"].as_i64().ok_or("saved song point invalid")?;
                let song_end = b["song"].as_i64().ok_or("saved song point invalid")?;
                let live_start = a["live"].as_i64().ok_or("saved live point invalid")?;
                let live_end = b["live"].as_i64().ok_or("saved live point invalid")?;
                let expected_start = plan.target.song_start_ticks.checked_add(song_start).ok_or("replacement start overflow")?;
                let expected_source =
                    plan.analysis["video"]["source_in"].as_i64().and_then(|v| v.checked_add(live_start)).ok_or("replacement source overflow")?;
                let speed = (live_end - live_start) as f64 / (song_end - song_start) as f64;
                if item["item"].as_u64() != Some(plan.target.video_item_id)
                    || item["start"].as_i64() != Some(expected_start)
                    || item["duration"].as_i64() != Some(song_end - song_start)
                    || item["sourceIn"].as_i64() != Some(expected_source)
                    || item["speed"].as_f64().is_none_or(|v| (v - speed).abs() > 1e-8)
                {
                    return Err("replacement video differs from the approved timing points".into());
                }
            }
            let audio_tracks = sequence["audio"].as_array().ok_or("audio tracks missing after performance match")?;
            let song_after = audio_tracks
                .iter()
                .find(|track| track["id"].as_u64() == Some(plan.target.song_track_id))
                .and_then(|track| track["items"].as_array())
                .and_then(|items| items.iter().find(|item| item["clip"].as_u64() == Some(plan.target.song_clip_id)))
                .ok_or("studio song missing after performance match")?;
            if song_after != &plan.song_before {
                return Err("studio song changed during performance match".into());
            }
            for id in &plan.target.linked_live_audio_ids {
                let item = audio_tracks
                    .iter()
                    .filter_map(|track| track["items"].as_array())
                    .flat_map(|items| items.iter())
                    .find(|item| item["clip"].as_u64() == Some(*id))
                    .ok_or("linked live audio missing after performance match")?;
                if item["enabled"].as_bool() != Some(false) || !item["link"].is_null() {
                    return Err("linked live audio was not muted and unlinked".into());
                }
            }
            Ok(json!({"committed":true,"verified":true,"action":"match_performance","target":plan.target,
                "clip_ids":ids,"similarity":plan.analysis["similarity"],"undo":"edit.undo"}))
        }
        .await;
        Ok(match result {
            Ok(v) => ok_json(&v),
            Err(e) => fail(e),
        })
    }

    #[tool(
        title = "Preview applying a color preset",
        description = "Resolve one video clip and one built-in Lumetri color preset, then preview the existing effects and preset description without editing.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = false, open_world_hint = false)
    )]
    async fn color_preset_preview(&self, Parameters(p): Parameters<ColorPresetPreviewParams>) -> Result<CallToolResult, McpError> {
        let result = async {
            self.inner.require_capability("guarded_color_preset").await?;
            let (revision, sequence_id, _, sequence) = self.snapshot().await?;
            if revision != p.revision {
                return Err(format!("stale project revision: supplied {}, current {revision}", p.revision));
            }
            if sequence_id != p.sequence_id {
                return Err(format!("active sequence changed: supplied {}, current {sequence_id}", p.sequence_id));
            }
            let target = crate::color_preset_plan::resolve(&sequence, p.clip_id, &p.name)?;
            let preset_list = self.inner.run("lumetri.presets", json!({})).await.map_err(|e| e.to_string())?;
            let preset = preset_list["presets"]
                .as_array()
                .and_then(|presets| presets.iter().find(|preset| preset["name"].as_str().is_some_and(|name| name.eq_ignore_ascii_case(&p.name))))
                .cloned()
                .ok_or_else(|| format!("unknown built-in color preset {:?}", p.name))?;
            let before_effects = sequence["video"]
                .as_array()
                .and_then(|tracks| tracks.iter().find(|track| track["id"].as_u64() == Some(target.track_id)))
                .and_then(|track| track["items"].as_array())
                .and_then(|items| items.iter().find(|item| item["clip"].as_u64() == Some(target.clip_id)))
                .and_then(|clip| clip["effects"].as_array())
                .cloned()
                .ok_or("clip effects unavailable")?;
            let (after_revision, after_sequence_id, _, _) = self.snapshot().await?;
            if after_revision != revision || after_sequence_id != sequence_id {
                return Err("project changed during preview; preview again".into());
            }
            let token = self.stash_color_plan(ColorPresetPlan { sequence_id, revision, target: target.clone(), before_effects })?;
            Ok(json!({"preview_only":true,"plan_token":token,"expires_in_seconds":300,"action":"apply_color_preset",
                "sequence_id":sequence_id,"revision":revision,"target":target,"preset":preset,"undo":"edit.undo"}))
        }
        .await;
        Ok(match result {
            Ok(v) => ok_json(&v),
            Err(e) => fail(e),
        })
    }

    #[tool(
        title = "Commit approved color preset",
        description = "Apply one previewed Lumetri preset to one video clip with atomic revision and sequence guards. Verify that exactly one preset effect was added and other effects stayed unchanged. The token is one-time.",
        annotations(read_only_hint = false, destructive_hint = false, idempotent_hint = false, open_world_hint = false)
    )]
    async fn color_preset_commit(&self, Parameters(p): Parameters<MarkerCommitParams>) -> Result<CallToolResult, McpError> {
        let result = async {
            let plan = self.take_color_plan(&p.plan_token)?;
            self.inner.require_capability("guarded_color_preset").await?;
            let (revision, sequence_id, _, _) = self.snapshot().await?;
            if revision != plan.revision {
                return Err(format!("stale project revision: preview {}, current {revision}", plan.revision));
            }
            if sequence_id != plan.sequence_id {
                return Err(format!("active sequence changed: preview {}, current {sequence_id}", plan.sequence_id));
            }
            let applied = self
                .inner
                .run(
                    "lumetri.applyPreset",
                    json!({"name":plan.target.preset_name,
                "clips":[plan.target.clip_id],"controllerSingleVideoClip":true,
                "expectedRevision":plan.revision,"sequenceId":plan.sequence_id}),
                )
                .await
                .map_err(|e| e.to_string())?;
            if applied["clips"].as_u64() != Some(1)
                || applied["preset"].as_str().is_none_or(|name| !name.eq_ignore_ascii_case(&plan.target.preset_name))
                || applied["effect"]["effect"].as_str() != Some("lumetri")
            {
                return Err("color command returned an unexpected result".into());
            }
            let (_, after_sequence_id, _, sequence) = self.snapshot().await?;
            if after_sequence_id != plan.sequence_id {
                return Err("color preset applied but active sequence changed during verification".into());
            }
            let track = sequence["video"]
                .as_array()
                .and_then(|tracks| tracks.iter().find(|track| track["id"].as_u64() == Some(plan.target.track_id)))
                .ok_or("video track missing after color preset")?;
            let clip = track["items"]
                .as_array()
                .and_then(|items| items.iter().find(|clip| clip["clip"].as_u64() == Some(plan.target.clip_id)))
                .ok_or("video clip missing after color preset")?;
            if clip["item"].as_u64() != Some(plan.target.item_id) {
                return Err("video clip source changed during color preset".into());
            }
            let effects = clip["effects"].as_array().ok_or("video effects missing after color preset")?;
            if effects.len() != plan.before_effects.len() + 1 {
                return Err("color preset changed the effect count unexpectedly".into());
            }
            let expected = &applied["effect"];
            let added_index = effects
                .iter()
                .enumerate()
                .find_map(|(index, effect)| {
                    let matches_expected = effect["effect"] == expected["effect"]
                        && effect["enabled"] == expected["enabled"]
                        && effect["masks"] == expected["masks"]
                        && effect["params"] == expected["params"];
                    if !matches_expected {
                        return None;
                    }
                    let remaining: Vec<Value> =
                        effects.iter().enumerate().filter(|(candidate, _)| *candidate != index).map(|(_, value)| value.clone()).collect();
                    (remaining == plan.before_effects).then_some(index)
                })
                .ok_or("added color effect did not match the approved preset or changed existing effects")?;
            Ok(json!({"committed":true,"verified":true,"action":"apply_color_preset","target":plan.target,
                "effect_index":added_index,"preset":applied["preset"],"undo":"edit.undo"}))
        }
        .await;
        Ok(match result {
            Ok(v) => ok_json(&v),
            Err(e) => fail(e),
        })
    }

    #[tool(
        title = "Preview applying a video effect",
        description = "Resolve one video clip and one standard video effect; preview existing effects without editing.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = false, open_world_hint = false)
    )]
    async fn video_effect_preview(&self, Parameters(p): Parameters<VideoEffectPreviewParams>) -> Result<CallToolResult, McpError> {
        let result = async {
            self.inner.require_capability("guarded_video_effect").await?;
            let (revision, sequence_id, _, sequence) = self.snapshot().await?;
            if revision != p.revision {
                return Err(format!("stale project revision: supplied {}, current {revision}", p.revision));
            }
            if sequence_id != p.sequence_id {
                return Err(format!("active sequence changed: supplied {}, current {sequence_id}", p.sequence_id));
            }
            let target = crate::video_effect_plan::resolve(&sequence, p.clip_id, &p.name)?;
            let list = self.inner.run("effects.list", json!({"kind":"Video"})).await.map_err(|e| e.to_string())?;
            let catalog = crate::video_effect_plan::catalog(&list)?;
            let effect = crate::video_effect_plan::find(&catalog, &p.name)?;
            let before_effects = sequence["video"]
                .as_array()
                .and_then(|tracks| tracks.iter().find(|track| track["id"].as_u64() == Some(target.track_id)))
                .and_then(|track| track["items"].as_array())
                .and_then(|items| items.iter().find(|item| item["clip"].as_u64() == Some(target.clip_id)))
                .and_then(|clip| clip["effects"].as_array())
                .cloned()
                .ok_or("clip effects unavailable")?;
            let (after_revision, after_sequence_id, _, _) = self.snapshot().await?;
            if after_revision != revision || after_sequence_id != sequence_id {
                return Err("project changed during preview; preview again".into());
            }
            let token = self.stash_effect_plan(VideoEffectPlan { sequence_id, revision, target: target.clone(), effect: effect.clone(), before_effects })?;
            Ok(json!({"preview_only":true,"plan_token":token,"expires_in_seconds":300,"action":"apply_video_effect",
                "sequence_id":sequence_id,"revision":revision,"target":target,"effect":effect,"undo":"edit.undo"}))
        }
        .await;
        Ok(match result {
            Ok(v) => ok_json(&v),
            Err(e) => fail(e),
        })
    }

    #[tool(
        title = "Commit approved video effect",
        description = "Add one previewed standard video effect to one video clip with atomic revision and sequence guards and exact post-edit verification. The token is one-time.",
        annotations(read_only_hint = false, destructive_hint = false, idempotent_hint = false, open_world_hint = false)
    )]
    async fn video_effect_commit(&self, Parameters(p): Parameters<MarkerCommitParams>) -> Result<CallToolResult, McpError> {
        let result = async {
            let plan = self.take_effect_plan(&p.plan_token)?;
            self.inner.require_capability("guarded_video_effect").await?;
            let (revision, sequence_id, _, _) = self.snapshot().await?;
            if revision != plan.revision {
                return Err(format!("stale project revision: preview {}, current {revision}", plan.revision));
            }
            if sequence_id != plan.sequence_id {
                return Err(format!("active sequence changed: preview {}, current {sequence_id}", plan.sequence_id));
            }
            let applied = self
                .inner
                .run(
                    "effects.apply",
                    json!({"effect":plan.effect["id"],
                "clips":[plan.target.clip_id],"controllerSingleVideoClip":true,
                "expectedRevision":plan.revision,"sequenceId":plan.sequence_id}),
                )
                .await
                .map_err(|e| e.to_string())?;
            if applied["clips"].as_u64() != Some(1) || applied["effect"]["effect"] != plan.effect["id"] {
                return Err("effect command returned an unexpected result".into());
            }
            let (_, after_sequence_id, _, sequence) = self.snapshot().await?;
            if after_sequence_id != plan.sequence_id {
                return Err("video effect applied but active sequence changed during verification".into());
            }
            let track = sequence["video"]
                .as_array()
                .and_then(|tracks| tracks.iter().find(|track| track["id"].as_u64() == Some(plan.target.track_id)))
                .ok_or("video track missing after effect")?;
            let clip = track["items"]
                .as_array()
                .and_then(|items| items.iter().find(|clip| clip["clip"].as_u64() == Some(plan.target.clip_id)))
                .ok_or("video clip missing after effect")?;
            if clip["item"].as_u64() != Some(plan.target.item_id) {
                return Err("video clip source changed during effect".into());
            }
            let effects = clip["effects"].as_array().ok_or("video effects missing after effect")?;
            if effects.len() != plan.before_effects.len() + 1 {
                return Err("video effect changed the effect count unexpectedly".into());
            }
            let expected = &applied["effect"];
            let added_index = effects
                .iter()
                .enumerate()
                .find_map(|(index, effect)| {
                    let matches_expected = effect["effect"] == expected["effect"]
                        && effect["enabled"] == expected["enabled"]
                        && effect["masks"] == expected["masks"]
                        && effect["params"] == expected["params"];
                    if !matches_expected {
                        return None;
                    }
                    let remaining: Vec<Value> =
                        effects.iter().enumerate().filter(|(candidate, _)| *candidate != index).map(|(_, value)| value.clone()).collect();
                    (remaining == plan.before_effects).then_some(index)
                })
                .ok_or("added video effect did not match preview or changed existing effects")?;
            Ok(json!({"committed":true,"verified":true,"action":"apply_video_effect","target":plan.target,
                "effect_index":added_index,"effect":plan.effect,"undo":"edit.undo"}))
        }
        .await;
        Ok(match result {
            Ok(v) => ok_json(&v),
            Err(e) => fail(e),
        })
    }

    #[tool(
        title = "Preview changing a video effect parameter",
        description = "Preview a static parameter or one numeric keyframe add, value change, removal, interpolation change, or move without editing.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = false, open_world_hint = false)
    )]
    async fn effect_param_preview(&self, Parameters(p): Parameters<EffectParamPreviewParams>) -> Result<CallToolResult, McpError> {
        let result = async {
            let keyframe = p.clip_time_ticks.is_some();
            let set_interpolation = p.interpolation.is_some();
            let move_keyframe = p.to_clip_time_ticks.is_some();
            if [p.edit_existing_keyframe, p.remove_existing_keyframe, set_interpolation, move_keyframe].into_iter().filter(|flag| *flag).count() > 1 {
                return Err("choose one keyframe action".into());
            }
            if p.edit_existing_keyframe && !keyframe {
                return Err("existing keyframe edit requires a clip-relative time".into());
            }
            if p.remove_existing_keyframe && (!keyframe || !p.value.is_null()) {
                return Err("keyframe removal requires a clip-relative time and no new value".into());
            }
            if set_interpolation && (!keyframe || !p.value.is_null()) {
                return Err("keyframe interpolation requires a clip-relative time and no new value".into());
            }
            if move_keyframe && (!keyframe || !p.value.is_null()) {
                return Err("keyframe move requires a source and destination clip-relative time and no new value".into());
            }
            self.inner
                .require_capability(if move_keyframe {
                    "guarded_effect_keyframe_move"
                } else if set_interpolation {
                    "guarded_effect_keyframe_interpolation"
                } else if p.remove_existing_keyframe {
                    "guarded_effect_keyframe_remove"
                } else if p.edit_existing_keyframe {
                    "guarded_effect_keyframe_edit"
                } else if keyframe {
                    "guarded_effect_keyframe"
                } else {
                    "guarded_effect_param_scalar"
                })
                .await?;
            let (revision, sequence_id, _, sequence) = self.snapshot().await?;
            if revision != p.revision {
                return Err(format!("stale project revision: supplied {}, current {revision}", p.revision));
            }
            if sequence_id != p.sequence_id {
                return Err(format!("active sequence changed: supplied {}, current {sequence_id}", p.sequence_id));
            }
            let target = if let Some(to) = p.to_clip_time_ticks {
                crate::effect_param_plan::resolve_move_keyframe(
                    &sequence,
                    p.clip_id,
                    &p.effect_id,
                    &p.param_id,
                    p.clip_time_ticks.ok_or("keyframe source time missing")?,
                    to,
                )?
            } else if let Some(interpolation) = p.interpolation.as_deref() {
                crate::effect_param_plan::resolve_interpolation(
                    &sequence,
                    p.clip_id,
                    &p.effect_id,
                    &p.param_id,
                    p.clip_time_ticks.ok_or("interpolation keyframe time missing")?,
                    interpolation,
                )?
            } else if p.remove_existing_keyframe {
                crate::effect_param_plan::resolve_remove_keyframe(
                    &sequence,
                    p.clip_id,
                    &p.effect_id,
                    &p.param_id,
                    p.clip_time_ticks.ok_or("keyframe removal time missing")?,
                )?
            } else if p.edit_existing_keyframe {
                crate::effect_param_plan::resolve_existing_keyframe(
                    &sequence,
                    p.clip_id,
                    &p.effect_id,
                    &p.param_id,
                    p.value,
                    p.clip_time_ticks.ok_or("existing keyframe time missing")?,
                )?
            } else if let Some(at) = p.clip_time_ticks {
                crate::effect_param_plan::resolve_keyframe(&sequence, p.clip_id, &p.effect_id, &p.param_id, p.value, at)?
            } else {
                crate::effect_param_plan::resolve(&sequence, p.clip_id, &p.effect_id, &p.param_id, p.value)?
            };
            let list = self.inner.run("effects.list", json!({"kind":"Video","detail":true})).await.map_err(|e| e.to_string())?;
            let catalog = crate::video_effect_plan::catalog(&list)?;
            let (parameter, resolved_value) = crate::effect_param_plan::parameter(&catalog, &target)?;
            if keyframe && (parameter["type"].as_str() != Some("float") || parameter["animatable"].as_bool() != Some(true)) {
                return Err("this keyframe action requires an animatable numeric slider".into());
            }
            let before_effects = sequence["video"]
                .as_array()
                .and_then(|tracks| tracks.iter().find(|track| track["id"].as_u64() == Some(target.track_id)))
                .and_then(|track| track["items"].as_array())
                .and_then(|items| items.iter().find(|item| item["clip"].as_u64() == Some(target.clip_id)))
                .and_then(|clip| clip["effects"].as_array())
                .cloned()
                .ok_or("clip effects unavailable")?;
            let (after_revision, after_sequence_id, _, _) = self.snapshot().await?;
            if after_revision != revision || after_sequence_id != sequence_id {
                return Err("project changed during preview; preview again".into());
            }
            let token = self.stash_param_plan(EffectParamPlan {
                sequence_id,
                revision,
                target: target.clone(),
                resolved_value: resolved_value.clone(),
                before_effects,
            })?;
            Ok(json!({"preview_only":true,"plan_token":token,"expires_in_seconds":300,
                "action":if move_keyframe { "move_effect_keyframe" }
                    else if set_interpolation { "set_effect_keyframe_interpolation" }
                    else if p.remove_existing_keyframe { "remove_effect_keyframe" }
                    else if p.edit_existing_keyframe { "change_effect_keyframe" }
                    else if keyframe { "add_effect_keyframe" } else { "set_effect_parameter" },
                "sequence_id":sequence_id,"revision":revision,"target":target,"parameter":parameter,
                "resolved_value":resolved_value,"undo":"edit.undo"}))
        }
        .await;
        Ok(match result {
            Ok(v) => ok_json(&v),
            Err(e) => fail(e),
        })
    }

    #[tool(
        title = "Commit approved video effect parameter",
        description = "Commit one previewed static parameter or numeric keyframe edit with atomic revision and sequence guards, exact readback, and a one-time token.",
        annotations(read_only_hint = false, destructive_hint = false, idempotent_hint = false, open_world_hint = false)
    )]
    async fn effect_param_commit(&self, Parameters(p): Parameters<MarkerCommitParams>) -> Result<CallToolResult, McpError> {
        let result = async {
            let plan = self.take_param_plan(&p.plan_token)?;
            let keyframe = plan.target.clip_time_ticks.is_some();
            let move_keyframe = plan.target.to_clip_time_ticks.is_some();
            let set_interpolation = plan.target.after_interpolation.is_some();
            let remove_keyframe = plan.target.remove_keyframe == Some(true);
            let edit_keyframe = plan.target.before_keyframe_value.is_some() && !remove_keyframe && !set_interpolation && !move_keyframe;
            self.inner
                .require_capability(if move_keyframe {
                    "guarded_effect_keyframe_move"
                } else if set_interpolation {
                    "guarded_effect_keyframe_interpolation"
                } else if remove_keyframe {
                    "guarded_effect_keyframe_remove"
                } else if edit_keyframe {
                    "guarded_effect_keyframe_edit"
                } else if keyframe {
                    "guarded_effect_keyframe"
                } else {
                    "guarded_effect_param_scalar"
                })
                .await?;
            let (revision, sequence_id, _, _) = self.snapshot().await?;
            if revision != plan.revision {
                return Err(format!("stale project revision: preview {}, current {revision}", plan.revision));
            }
            if sequence_id != plan.sequence_id {
                return Err(format!("active sequence changed: preview {}, current {sequence_id}", plan.sequence_id));
            }
            let mut command = json!({
                "clip":plan.target.clip_id,"effect":plan.target.effect_index,
                "effectId":plan.target.effect_id,"param":plan.target.param_id,
                "expectedRevision":plan.revision,"sequenceId":plan.sequence_id});
            if keyframe {
                if move_keyframe {
                    command["controllerMoveVideoKeyframe"] = json!(true);
                    command["expectedKeyframeValue"] = json!(plan.target.before_keyframe_value);
                    command["expectedInterpolation"] = json!(plan.target.before_interpolation);
                    command["toClipTime"] = json!(plan.target.to_clip_time_ticks);
                    command["expectedToMediaTime"] = json!(plan.target.to_media_time_ticks);
                } else if set_interpolation {
                    command["controllerKeyframeInterpolation"] = json!(true);
                    command["expectedKeyframeValue"] = json!(plan.target.before_keyframe_value);
                    command["expectedInterpolation"] = json!(plan.target.before_interpolation);
                    command["interpolation"] = json!(plan.target.after_interpolation);
                } else if remove_keyframe {
                    command["controllerRemoveVideoKeyframe"] = json!(true);
                    command["expectedKeyframeValue"] = json!(plan.target.before_keyframe_value);
                } else if edit_keyframe {
                    command["controllerExistingVideoKeyframe"] = json!(true);
                    command["expectedKeyframeValue"] = json!(plan.target.before_keyframe_value);
                } else {
                    command["controllerVideoKeyframe"] = json!(true);
                }
                if !remove_keyframe && !set_interpolation && !move_keyframe {
                    command["value"] = plan.resolved_value.clone();
                }
                command["clipTime"] = json!(plan.target.clip_time_ticks);
                command["expectedMediaTime"] = json!(plan.target.media_time_ticks);
            } else {
                command["controllerStaticVideoParam"] = json!(true);
                command["value"] = plan.resolved_value.clone();
            }
            let applied = self.inner.run("effects.setParam", command).await.map_err(|e| e.to_string())?;
            if applied["clip"].as_u64() != Some(plan.target.clip_id)
                || applied["effect_index"].as_u64() != Some(plan.target.effect_index as u64)
                || applied["effect"]["effect"].as_str() != Some(plan.target.effect_id.as_str())
            {
                return Err("parameter command returned an unexpected result".into());
            }
            let (_, after_sequence_id, _, sequence) = self.snapshot().await?;
            if after_sequence_id != plan.sequence_id {
                return Err("parameter changed but active sequence changed during verification".into());
            }
            let track = sequence["video"]
                .as_array()
                .and_then(|tracks| tracks.iter().find(|track| track["id"].as_u64() == Some(plan.target.track_id)))
                .ok_or("video track missing after parameter edit")?;
            let clip = track["items"]
                .as_array()
                .and_then(|items| items.iter().find(|clip| clip["clip"].as_u64() == Some(plan.target.clip_id)))
                .ok_or("video clip missing after parameter edit")?;
            if clip["item"].as_u64() != Some(plan.target.item_id) {
                return Err("video clip source changed during parameter edit".into());
            }
            let effects = clip["effects"].as_array().ok_or("video effects missing after parameter edit")?;
            let mut expected = plan.before_effects.clone();
            let old_effect = expected.get(plan.target.effect_index).ok_or("effect index missing from preview")?;
            if old_effect["effect"] != applied["effect"]["effect"]
                || old_effect["enabled"] != applied["effect"]["enabled"]
                || old_effect["masks"] != applied["effect"]["masks"]
            {
                return Err("effect identity changed during parameter edit".into());
            }
            let mut old_params = old_effect["params"].clone();
            let new_param = &applied["effect"]["params"][plan.target.param_id.as_str()];
            if move_keyframe {
                let from = plan.target.media_time_ticks.ok_or("keyframe source time missing from preview")?;
                let to = plan.target.to_media_time_ticks.ok_or("keyframe destination time missing from preview")?;
                let mut expected_param = old_params[plan.target.param_id.as_str()].clone();
                let details = expected_param["keyframeDetails"].as_array_mut().ok_or("old keyframe details missing")?;
                let index = details.iter().position(|point| point["time"].as_i64() == Some(from)).ok_or("approved keyframe missing from old effect")?;
                if details[index]["value"].as_str() != plan.target.before_keyframe_value.as_deref()
                    || details[index]["interpolation"].as_str() != plan.target.before_interpolation.as_deref()
                    || details.iter().any(|point| point["time"].as_i64() == Some(to))
                {
                    return Err("approved keyframe changed or destination is occupied".into());
                }
                let mut point = details.remove(index);
                point["time"] = json!(to);
                let insertion = details.partition_point(|other| other["time"].as_i64().is_some_and(|time| time < to));
                details.insert(insertion, point);
                if &expected_param != new_param {
                    return Err("keyframe move changed another value or did not persist".into());
                }
            } else if set_interpolation {
                let at = plan.target.media_time_ticks.ok_or("keyframe media time missing from preview")?;
                let mut expected_param = old_params[plan.target.param_id.as_str()].clone();
                let details = expected_param["keyframeDetails"].as_array_mut().ok_or("old keyframe details missing")?;
                let mut matches = details.iter_mut().filter(|point| point["time"].as_i64() == Some(at));
                let point = matches.next().ok_or("approved keyframe missing from old effect")?;
                if point["value"].as_str() != plan.target.before_keyframe_value.as_deref()
                    || point["interpolation"].as_str() != plan.target.before_interpolation.as_deref()
                {
                    return Err("approved keyframe changed before interpolation edit".into());
                }
                point["interpolation"] = json!(plan.target.after_interpolation);
                if matches.next().is_some() || &expected_param != new_param {
                    return Err("keyframe interpolation changed another value or did not persist".into());
                }
            } else if remove_keyframe {
                let at = plan.target.media_time_ticks.ok_or("keyframe media time missing from preview")?;
                let mut expected_param = old_params[plan.target.param_id.as_str()].clone();
                let count = expected_param["keyframes"].as_u64().ok_or("old keyframe count unavailable")?;
                if count < 2 {
                    return Err("keyframe removal would leave no animation".into());
                }
                let details = expected_param["keyframeDetails"].as_array_mut().ok_or("old keyframe details missing")?;
                if details.len() != count as usize {
                    return Err("old keyframe details did not match count".into());
                }
                let index = details.iter().position(|point| point["time"].as_i64() == Some(at)).ok_or("approved keyframe missing from old effect")?;
                if details[index]["value"].as_str() != plan.target.before_keyframe_value.as_deref() {
                    return Err("approved keyframe's prior value changed".into());
                }
                details.remove(index);
                expected_param["keyframes"] = json!(count - 1);
                if &expected_param != new_param {
                    return Err("keyframe removal changed another value or did not persist".into());
                }
            } else if edit_keyframe {
                let at = plan.target.media_time_ticks.ok_or("keyframe media time missing from preview")?;
                let value = plan.resolved_value.as_f64().ok_or("keyframe numeric value missing from preview")?;
                let old_param = &old_params[plan.target.param_id.as_str()];
                let mut expected_param = old_param.clone();
                let details = expected_param["keyframeDetails"].as_array_mut().ok_or("old keyframe details missing")?;
                let mut matches = details.iter_mut().filter(|point| point["time"].as_i64() == Some(at));
                let point = matches.next().ok_or("approved keyframe missing from old effect")?;
                if point["value"].as_str() != plan.target.before_keyframe_value.as_deref() {
                    return Err("approved keyframe's prior value changed".into());
                }
                point["value"] = json!(format!("Float({value:?})"));
                if matches.next().is_some() || &expected_param != new_param {
                    return Err("keyframe edit changed another value or did not persist".into());
                }
            } else if keyframe {
                let at = plan.target.media_time_ticks.ok_or("keyframe media time missing from preview")?;
                let value = plan.resolved_value.as_f64().ok_or("keyframe numeric value missing from preview")?;
                let mut expected_param = old_params[plan.target.param_id.as_str()].clone();
                let count = expected_param["keyframes"].as_u64().ok_or("old keyframe count unavailable")?;
                if count >= 64 {
                    return Err("too many existing keyframes".into());
                }
                let details = expected_param["keyframeDetails"].as_array_mut().ok_or("old keyframe details missing")?;
                if details.len() != count as usize {
                    return Err("old keyframe details did not match count".into());
                }
                let insert_at = details.partition_point(|point| point["time"].as_i64().is_some_and(|time| time < at));
                if details.get(insert_at).is_some_and(|point| point["time"].as_i64() == Some(at)) {
                    return Err("approved keyframe already existed".into());
                }
                details.insert(
                    insert_at,
                    json!({"time":at,"value":format!("Float({value:?})"),
                    "interpolation":"Linear"}),
                );
                expected_param["keyframes"] = json!(count + 1);
                if &expected_param != new_param {
                    return Err("keyframe did not match the approved time and value".into());
                }
            }
            old_params[plan.target.param_id.as_str()] = new_param.clone();
            if old_params != applied["effect"]["params"] {
                return Err("parameter command changed another parameter".into());
            }
            expected[plan.target.effect_index] = applied["effect"].clone();
            if effects != &expected {
                return Err("parameter edit changed other effect data or did not persist".into());
            }
            Ok(json!({"committed":true,"verified":true,
                "action":if move_keyframe { "move_effect_keyframe" }
                    else if set_interpolation { "set_effect_keyframe_interpolation" }
                    else if remove_keyframe { "remove_effect_keyframe" }
                    else if edit_keyframe { "change_effect_keyframe" }
                    else if keyframe { "add_effect_keyframe" } else { "set_effect_parameter" },"target":plan.target,
                "parameter":applied["effect"]["params"][plan.target.param_id.as_str()],"undo":"edit.undo"}))
        }
        .await;
        Ok(match result {
            Ok(v) => ok_json(&v),
            Err(e) => fail(e),
        })
    }

    #[tool(
        title = "Preview removing a video effect",
        description = "Preview removal of one existing standard video effect instance from one enabled video clip. Show the complete inspected effect without editing.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = false, open_world_hint = false)
    )]
    async fn video_effect_remove_preview(&self, Parameters(p): Parameters<RemoveEffectPreviewParams>) -> Result<CallToolResult, McpError> {
        let result = async {
            self.inner.require_capability("guarded_effect_remove").await?;
            let (revision, sequence_id, _, sequence) = self.snapshot().await?;
            if revision != p.revision {
                return Err(format!("stale project revision: supplied {}, current {revision}", p.revision));
            }
            if sequence_id != p.sequence_id {
                return Err(format!("active sequence changed: supplied {}, current {sequence_id}", p.sequence_id));
            }
            let target = crate::video_effect_plan::resolve_removal(&sequence, p.clip_id, &p.effect_id)?;
            let list = self.inner.run("effects.list", json!({"kind":"Video"})).await.map_err(|e| e.to_string())?;
            let catalog = crate::video_effect_plan::catalog(&list)?;
            let effect = catalog
                .iter()
                .find(|effect| effect["id"].as_str() == Some(target.effect_id.as_str()))
                .cloned()
                .ok_or_else(|| format!("effect {:?} is not a standard video effect", target.effect_id))?;
            let before_effects = sequence["video"]
                .as_array()
                .and_then(|tracks| tracks.iter().find(|track| track["id"].as_u64() == Some(target.track_id)))
                .and_then(|track| track["items"].as_array())
                .and_then(|items| items.iter().find(|item| item["clip"].as_u64() == Some(target.clip_id)))
                .and_then(|clip| clip["effects"].as_array())
                .cloned()
                .ok_or("clip effects unavailable")?;
            let (after_revision, after_sequence_id, _, _) = self.snapshot().await?;
            if after_revision != revision || after_sequence_id != sequence_id {
                return Err("project changed during preview; preview again".into());
            }
            let token = self.stash_remove_plan(RemoveEffectPlan { sequence_id, revision, target: target.clone(), before_effects })?;
            Ok(json!({"preview_only":true,"plan_token":token,"expires_in_seconds":300,"action":"remove_video_effect",
                "sequence_id":sequence_id,"revision":revision,"target":target,"effect":effect,"undo":"edit.undo"}))
        }
        .await;
        Ok(match result {
            Ok(v) => ok_json(&v),
            Err(e) => fail(e),
        })
    }

    #[tool(
        title = "Commit approved video effect removal",
        description = "Remove one previewed standard video effect with atomic revision and sequence guards. Verify that the other effects stayed unchanged. The token is one-time.",
        annotations(read_only_hint = false, destructive_hint = false, idempotent_hint = false, open_world_hint = false)
    )]
    async fn video_effect_remove_commit(&self, Parameters(p): Parameters<MarkerCommitParams>) -> Result<CallToolResult, McpError> {
        let result = async {
            let plan = self.take_remove_plan(&p.plan_token)?;
            self.inner.require_capability("guarded_effect_remove").await?;
            let (revision, sequence_id, _, _) = self.snapshot().await?;
            if revision != plan.revision {
                return Err(format!("stale project revision: preview {}, current {revision}", plan.revision));
            }
            if sequence_id != plan.sequence_id {
                return Err(format!("active sequence changed: preview {}, current {sequence_id}", plan.sequence_id));
            }
            let applied = self
                .inner
                .run(
                    "effects.remove",
                    json!({"clip":plan.target.clip_id,
                "index":plan.target.effect_index,"effectId":plan.target.effect_id,
                "controllerSingleVideoEffect":true,"expectedRevision":plan.revision,"sequenceId":plan.sequence_id}),
                )
                .await
                .map_err(|e| e.to_string())?;
            if applied["clip"].as_u64() != Some(plan.target.clip_id)
                || applied["effect_index"].as_u64() != Some(plan.target.effect_index as u64)
                || applied["removed"] != plan.target.removed_effect
            {
                return Err("effect removal returned an unexpected result".into());
            }
            let (_, after_sequence_id, _, sequence) = self.snapshot().await?;
            if after_sequence_id != plan.sequence_id {
                return Err("effect removed but active sequence changed during verification".into());
            }
            let track = sequence["video"]
                .as_array()
                .and_then(|tracks| tracks.iter().find(|track| track["id"].as_u64() == Some(plan.target.track_id)))
                .ok_or("video track missing after effect removal")?;
            let clip = track["items"]
                .as_array()
                .and_then(|items| items.iter().find(|clip| clip["clip"].as_u64() == Some(plan.target.clip_id)))
                .ok_or("video clip missing after effect removal")?;
            if clip["item"].as_u64() != Some(plan.target.item_id) {
                return Err("video clip source changed during effect removal".into());
            }
            let effects = clip["effects"].as_array().ok_or("video effects missing after effect removal")?;
            let mut expected = plan.before_effects.clone();
            if expected.get(plan.target.effect_index) != Some(&plan.target.removed_effect) {
                return Err("approved effect was absent from preview snapshot".into());
            }
            expected.remove(plan.target.effect_index);
            if effects != &expected {
                return Err("effect removal changed other effect data or did not persist".into());
            }
            Ok(json!({"committed":true,"verified":true,"action":"remove_video_effect","target":plan.target,
                "undo":"edit.undo"}))
        }
        .await;
        Ok(match result {
            Ok(v) => ok_json(&v),
            Err(e) => fail(e),
        })
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for FilmcraftMarkerControllerMcp {
    fn get_info(&self) -> rmcp::model::ServerConfig {
        rmcp::model::ServerConfig::new(rmcp::model::ServerCapabilities::builder().enable_tools().build())
            .with_server_info(rmcp::model::Implementation::new("filmcraft-controller-markers", env!("CARGO_PKG_VERSION")))
            .with_instructions(MARKER_CONTROLLER_INSTRUCTIONS.to_string())
    }

    async fn call_tool(
        &self,
        request: rmcp::model::CallToolRequestParams,
        context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<rmcp::model::CallToolResponse, McpError> {
        if let Some(message) = self.unknown_arg(&request.name, request.arguments.as_ref()) {
            return Err(McpError::invalid_params(message, None));
        }
        let tcc = rmcp::handler::server::tool::ToolCallContext::new(self, request, context);
        self.tool_router.call(tcc).await
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for FilmcraftControllerMcp {
    fn get_info(&self) -> rmcp::model::ServerConfig {
        rmcp::model::ServerConfig::new(rmcp::model::ServerCapabilities::builder().enable_tools().build())
            .with_server_info(rmcp::model::Implementation::new("filmcraft-controller-readonly", env!("CARGO_PKG_VERSION")))
            .with_instructions(CONTROLLER_INSTRUCTIONS.to_string())
    }

    async fn call_tool(
        &self,
        request: rmcp::model::CallToolRequestParams,
        context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<rmcp::model::CallToolResponse, McpError> {
        if let Some(m) = self.unknown_arg(&request.name, request.arguments.as_ref()) {
            return Err(McpError::invalid_params(m, None));
        }
        let tcc = rmcp::handler::server::tool::ToolCallContext::new(self, request, context);
        self.tool_router.call(tcc).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Client {
        tx: tokio::io::DuplexStream,
        rx: tokio::io::Lines<tokio::io::BufReader<tokio::io::DuplexStream>>,
    }

    impl Client {
        fn start_controller(s: Session) -> Self {
            use tokio::io::AsyncBufReadExt;
            let (tx, server_in) = tokio::io::duplex(1 << 22);
            let (server_out, rx) = tokio::io::duplex(1 << 22);
            tokio::spawn(FilmcraftControllerMcp::headless(s).serve_io(server_in, server_out));
            Self { tx, rx: tokio::io::BufReader::new(rx).lines() }
        }
        fn start_marker_controller(s: Session) -> Self {
            use tokio::io::AsyncBufReadExt;
            let (tx, server_in) = tokio::io::duplex(1 << 22);
            let (server_out, rx) = tokio::io::duplex(1 << 22);
            tokio::spawn(FilmcraftMarkerControllerMcp::headless(s).serve_io(server_in, server_out));
            Self { tx, rx: tokio::io::BufReader::new(rx).lines() }
        }
        async fn send(&mut self, line: &str) {
            use tokio::io::AsyncWriteExt;
            self.tx.write_all(format!("{line}\n").as_bytes()).await.unwrap();
        }
        async fn next(&mut self) -> Value {
            serde_json::from_str(&self.rx.next_line().await.unwrap().unwrap()).unwrap()
        }
        async fn ask(&mut self, msg: Value) -> Value {
            self.send(&msg.to_string()).await;
            self.next().await
        }
        async fn init(&mut self) -> Value {
            let r = self.ask(json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}})).await;
            self.send(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}).to_string()).await;
            r
        }
        async fn call(&mut self, id: u64, name: &str, arguments: Value) -> Value {
            self.ask(json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":name,"arguments":arguments}})).await
        }
    }

    fn demo() -> Session {
        let mut s = Session::default();
        s.execute("file.openDemoProject", json!({})).unwrap();
        s
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn controller_profile_exposes_reads_and_blocks_mutating_tools() {
        let mut c = Client::start_controller(demo());
        let init = c.init().await;
        assert_eq!(init["result"]["serverInfo"]["name"], "filmcraft-controller-readonly");
        let tools = c.ask(json!({"jsonrpc":"2.0","id":1,"method":"tools/list"})).await["result"]["tools"].as_array().cloned().unwrap();
        let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
        for want in ["project_inspect", "sequence_inspect", "doc_inspect", "command_list", "render_current_frame", "ui_inspect", "request_plan"] {
            assert!(names.contains(&want), "{want} not listed");
        }
        for forbidden in ["command_run", "command_batch", "media_import", "ui_click", "ui_drag", "ui_key", "ui_type", "ui_control"] {
            assert!(!names.contains(&forbidden), "{forbidden} leaked into the read-only profile");
        }
        for tool in &tools {
            assert_eq!(tool["annotations"]["readOnlyHint"], true, "{} is not marked read-only", tool["name"]);
            assert_eq!(tool["annotations"]["destructiveHint"], false, "{} is marked destructive", tool["name"]);
        }

        let before = c.call(2, "doc_inspect", json!({})).await;
        assert_eq!(before["result"]["isError"], false, "{before}");
        let denied = c.call(3, "command_run", json!({"id":"file.newBin", "params":{"name":"must not appear"}})).await;
        assert!(denied["error"].is_object() || denied["result"]["isError"] == true, "write tool was not rejected: {denied}");
        let preview = c.call(4, "render_current_frame", json!({"max_side":320})).await;
        assert_ne!(preview["result"]["isError"], true, "{preview}");
        let planned = c.call(42, "request_plan", json!({"request":"Add a marker at the playhead called Opening beat"})).await;
        assert_eq!(planned["result"]["isError"], false, "{planned}");
        let plan: Value = serde_json::from_str(planned["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(plan["action"], "add_marker");
        assert_eq!(plan["preview_only"], true);
        let after = c.call(5, "doc_inspect", json!({})).await;
        assert_eq!(after["result"]["content"][0]["text"], before["result"]["content"][0]["text"], "read-only calls changed project state");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn marker_controller_requires_preview_token_and_verifies_add_and_edit() {
        let mut c = Client::start_marker_controller(demo());
        let init = c.init().await;
        assert_eq!(init["result"]["serverInfo"]["name"], "filmcraft-controller-markers");
        let tools = c.ask(json!({"jsonrpc":"2.0","id":1,"method":"tools/list"})).await["result"]["tools"].as_array().cloned().unwrap();
        let names: Vec<&str> = tools.iter().filter_map(|x| x["name"].as_str()).collect();
        for want in [
            "request_plan",
            "marker_add_preview",
            "marker_edit_preview",
            "marker_commit",
            "clip_move_preview",
            "clip_move_commit",
            "clip_delete_preview",
            "clip_delete_commit",
            "audio_place_preview",
            "audio_place_commit",
            "transition_add_preview",
            "transition_add_commit",
            "audio_align_preview",
            "nest_clip_preview",
            "nest_clip_commit",
            "performance_match_preview",
            "performance_match_commit",
            "color_preset_preview",
            "color_preset_commit",
            "color_presets",
            "sequence_inspect",
        ] {
            assert!(names.contains(&want), "{want} absent");
        }
        for denied in ["command_run", "command_batch", "ui_click", "markers_delete"] {
            assert!(!names.contains(&denied), "{denied} leaked into marker profile");
        }
        for t in &tools {
            let write_tool = t["name"].as_str().is_some_and(|name| name.ends_with("_commit"));
            assert_eq!(t["annotations"]["readOnlyHint"], !write_tool, "{} read-only annotation", t["name"]);
        }

        let project = c.call(2, "project_inspect", json!({})).await;
        let sequence = c.call(3, "sequence_inspect", json!({})).await;
        let project: Value = serde_json::from_str(project["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        let sequence: Value = serde_json::from_str(sequence["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        let revision = project["revision"].as_u64().unwrap();
        let sequence_id = sequence["id"].as_u64().unwrap();
        let before_markers = sequence["markers"].as_array().unwrap().len();

        let request = c.call(40, "request_plan", json!({"request":"Add a marker at 0 seconds called Controller Test Marker with comment previewed"})).await;
        assert_eq!(request["result"]["isError"], false, "{request}");
        let request_plan: Value = serde_json::from_str(request["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(request_plan["execution"]["tool"], "marker_add_preview");
        assert_eq!(request_plan["project"]["revision"], revision);
        let preview = c.call(4, "marker_add_preview", request_plan["execution"]["arguments"].clone()).await;
        assert_eq!(preview["result"]["isError"], false, "{preview}");
        let planned: Value = serde_json::from_str(preview["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(planned["preview_only"], true);
        let token = planned["plan_token"].as_str().unwrap().to_string();
        let unchanged = c.call(5, "sequence_inspect", json!({})).await;
        let unchanged: Value = serde_json::from_str(unchanged["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(unchanged["markers"].as_array().unwrap().len(), before_markers, "preview changed markers");

        let committed = c.call(6, "marker_commit", json!({"plan_token":token})).await;
        assert_eq!(committed["result"]["isError"], false, "{committed}");
        let verified: Value = serde_json::from_str(committed["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(verified["verified"], true, "{verified}");
        let marker_id = verified["marker_id"].as_u64().unwrap();
        assert_eq!(verified["marker"]["name"], "Controller Test Marker");
        let replay = c.call(7, "marker_commit", json!({"plan_token":planned["plan_token"]})).await;
        assert_eq!(replay["result"]["isError"], true, "single-use plan token replayed: {replay}");

        let current = c.call(8, "project_inspect", json!({})).await;
        let current: Value = serde_json::from_str(current["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        let edit_preview = c
            .call(
                9,
                "marker_edit_preview",
                json!({"sequence_id":sequence_id,"revision":current["revision"],"marker_id":marker_id,"name":"Approved Marker Name"}),
            )
            .await;
        assert_eq!(edit_preview["result"]["isError"], false, "{edit_preview}");
        let edit_plan: Value = serde_json::from_str(edit_preview["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(edit_plan["before"]["name"], "Controller Test Marker");
        assert_eq!(edit_plan["after"]["name"], "Approved Marker Name");
        let edit = c.call(10, "marker_commit", json!({"plan_token":edit_plan["plan_token"]})).await;
        assert_eq!(edit["result"]["isError"], false, "{edit}");
        let edited: Value = serde_json::from_str(edit["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(edited["verified"], true, "{edited}");
        assert_eq!(edited["marker"]["id"], marker_id);
        assert_eq!(edited["marker"]["name"], "Approved Marker Name");

        let move_request = c.call(11, "request_plan", json!({"request":"Move clip #47 to 30 seconds"})).await;
        assert_eq!(move_request["result"]["isError"], false, "{move_request}");
        let move_request: Value = serde_json::from_str(move_request["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(move_request["execution"]["tool"], "clip_move_preview");
        let move_preview = c.call(12, "clip_move_preview", move_request["execution"]["arguments"].clone()).await;
        assert_eq!(move_preview["result"]["isError"], false, "{move_preview}");
        let move_preview: Value = serde_json::from_str(move_preview["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(move_preview["affected"].as_array().unwrap().len(), 1);
        let before_move = c.call(13, "sequence_inspect", json!({})).await;
        let before_move: Value = serde_json::from_str(before_move["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(before_move["audio"][1]["items"][0]["start"], 0, "preview changed the clip");
        let moved = c.call(14, "clip_move_commit", json!({"plan_token":move_preview["plan_token"]})).await;
        assert_eq!(moved["result"]["isError"], false, "{moved}");
        let moved: Value = serde_json::from_str(moved["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(moved["verified"], true);
        let after_move = c.call(15, "sequence_inspect", json!({})).await;
        let after_move: Value = serde_json::from_str(after_move["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(after_move["audio"][1]["items"][0]["start"], 30 * filmcraft_time::TICKS_PER_SECOND);
        let replay_move = c.call(16, "clip_move_commit", json!({"plan_token":move_preview["plan_token"]})).await;
        assert_eq!(replay_move["result"]["isError"], true);

        let audio_request = c.call(17, "request_plan", json!({"request":"Place audio item #11 on A3 at 30 seconds"})).await;
        assert_eq!(audio_request["result"]["isError"], false, "{audio_request}");
        let audio_request: Value = serde_json::from_str(audio_request["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(audio_request["execution"]["tool"], "audio_place_preview");
        let audio_preview = c.call(18, "audio_place_preview", audio_request["execution"]["arguments"].clone()).await;
        assert_eq!(audio_preview["result"]["isError"], false, "{audio_preview}");
        let audio_preview: Value = serde_json::from_str(audio_preview["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(audio_preview["target"], audio_request["target"]);
        let before_audio = c.call(19, "sequence_inspect", json!({})).await;
        let before_audio: Value = serde_json::from_str(before_audio["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(before_audio["audio"][2]["items"], json!([]));
        let placed = c.call(20, "audio_place_commit", json!({"plan_token":audio_preview["plan_token"]})).await;
        assert_eq!(placed["result"]["isError"], false, "{placed}");
        let placed: Value = serde_json::from_str(placed["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(placed["verified"], true);
        let after_audio = c.call(21, "sequence_inspect", json!({})).await;
        let after_audio: Value = serde_json::from_str(after_audio["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(after_audio["audio"][2]["items"][0]["clip"], placed["clip_id"]);
        assert_eq!(after_audio["audio"][2]["items"][0]["item"], 11);
        let replay_audio = c.call(22, "audio_place_commit", json!({"plan_token":audio_preview["plan_token"]})).await;
        assert_eq!(replay_audio["result"]["isError"], true);

        let transition_request = c.call(23, "request_plan", json!({"request":"Add cross dissolve after clip #32 for 12 frames"})).await;
        assert_eq!(transition_request["result"]["isError"], false, "{transition_request}");
        let transition_request: Value = serde_json::from_str(transition_request["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(transition_request["execution"]["tool"], "transition_add_preview");
        let transition_preview = c.call(24, "transition_add_preview", transition_request["execution"]["arguments"].clone()).await;
        assert_eq!(transition_preview["result"]["isError"], false, "{transition_preview}");
        let transition_preview: Value = serde_json::from_str(transition_preview["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(transition_preview["target"], transition_request["target"]);
        let before_transition = c.call(25, "sequence_inspect", json!({})).await;
        let before_transition: Value = serde_json::from_str(before_transition["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(before_transition["video"][0]["transitions"].as_array().unwrap().len(), 4);
        let applied = c.call(26, "transition_add_commit", json!({"plan_token":transition_preview["plan_token"]})).await;
        assert_eq!(applied["result"]["isError"], false, "{applied}");
        let applied: Value = serde_json::from_str(applied["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(applied["verified"], true);
        let after_transition = c.call(27, "sequence_inspect", json!({})).await;
        let after_transition: Value = serde_json::from_str(after_transition["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(after_transition["video"][0]["transitions"].as_array().unwrap().len(), 5);
        let replay_transition = c.call(28, "transition_add_commit", json!({"plan_token":transition_preview["plan_token"]})).await;
        assert_eq!(replay_transition["result"]["isError"], true);

        let align_request = c.call(29, "request_plan", json!({"request":"Align audio clip #47 to video clip #35"})).await;
        assert_eq!(align_request["result"]["isError"], false, "{align_request}");
        let align_request: Value = serde_json::from_str(align_request["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(align_request["execution"]["tool"], "audio_align_preview");
        let align_preview = c.call(30, "audio_align_preview", align_request["execution"]["arguments"].clone()).await;
        assert_eq!(align_preview["result"]["isError"], false, "{align_preview}");
        let align_preview: Value = serde_json::from_str(align_preview["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(align_preview["target"], align_request["target"]);
        let aligned = c.call(31, "clip_move_commit", json!({"plan_token":align_preview["plan_token"]})).await;
        assert_eq!(aligned["result"]["isError"], false, "{aligned}");
        let aligned: Value = serde_json::from_str(aligned["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(aligned["verified"], true);
        assert_eq!(aligned["action"], "align_audio_to_video");
        let after_align = c.call(32, "sequence_inspect", json!({})).await;
        let after_align: Value = serde_json::from_str(after_align["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(after_align["audio"][1]["items"][0]["start"], after_align["video"][0]["items"][2]["start"]);
        let replay_align = c.call(33, "clip_move_commit", json!({"plan_token":align_preview["plan_token"]})).await;
        assert_eq!(replay_align["result"]["isError"], true);

        let nest_request = c.call(34, "request_plan", json!({"request":"Nest clip #46 as Overlay Nest"})).await;
        assert_eq!(nest_request["result"]["isError"], false, "{nest_request}");
        let nest_request: Value = serde_json::from_str(nest_request["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(nest_request["execution"]["tool"], "nest_clip_preview");
        let nest_preview = c.call(35, "nest_clip_preview", nest_request["execution"]["arguments"].clone()).await;
        assert_eq!(nest_preview["result"]["isError"], false, "{nest_preview}");
        let nest_preview: Value = serde_json::from_str(nest_preview["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(nest_preview["target"], nest_request["target"]);
        let nested = c.call(36, "nest_clip_commit", json!({"plan_token":nest_preview["plan_token"]})).await;
        assert_eq!(nested["result"]["isError"], false, "{nested}");
        let nested: Value = serde_json::from_str(nested["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(nested["verified"], true);
        let after_nest = c.call(37, "sequence_inspect", json!({})).await;
        let after_nest: Value = serde_json::from_str(after_nest["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(after_nest["video"][1]["items"][0]["item"], nested["nested_sequence_id"]);
        let replay_nest = c.call(38, "nest_clip_commit", json!({"plan_token":nest_preview["plan_token"]})).await;
        assert_eq!(replay_nest["result"]["isError"], true);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn performance_match_preview_commit_and_replay_guard() {
        let mut s = demo();
        s.execute("file.newSequence", json!({"name":"Performance controller test","video":1,"audio":2})).unwrap();
        let duration = 4 * filmcraft_time::TICKS_PER_SECOND;
        let video = s.execute("timeline.place", json!({"item":5,"duration":duration})).unwrap();
        let song = s.execute("timeline.place", json!({"item":11,"audioTrack":"A2","time":filmcraft_time::TICKS_PER_SECOND,"duration":duration})).unwrap();
        let video_id = video["clips"][0].as_u64().unwrap();
        let song_id = song["clips"][0].as_u64().unwrap();
        let mut c = Client::start_marker_controller(s);
        c.init().await;
        let before = c.call(1, "sequence_inspect", json!({})).await;
        let before: Value = serde_json::from_str(before["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        let request = format!("Match performance of video clip #{video_id} to song clip #{song_id}");
        let planned = c.call(2, "request_plan", json!({"request":request})).await;
        assert_eq!(planned["result"]["isError"], false, "{planned}");
        let planned: Value = serde_json::from_str(planned["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(planned["execution"]["tool"], "performance_match_preview");
        let preview = c.call(3, "performance_match_preview", planned["execution"]["arguments"].clone()).await;
        assert_eq!(preview["result"]["isError"], false, "{preview}");
        let preview: Value = serde_json::from_str(preview["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(preview["target"], planned["target"]);
        assert_eq!(preview["analysis"]["piece_count"], preview["analysis"]["points"].as_array().unwrap().len() - 1);
        let unchanged = c.call(4, "sequence_inspect", json!({})).await;
        let unchanged: Value = serde_json::from_str(unchanged["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(unchanged, before);
        let commit = c.call(5, "performance_match_commit", json!({"plan_token":preview["plan_token"]})).await;
        assert_eq!(commit["result"]["isError"], false, "{commit}");
        let commit: Value = serde_json::from_str(commit["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(commit["verified"], true);
        let replay = c.call(6, "performance_match_commit", json!({"plan_token":preview["plan_token"]})).await;
        assert_eq!(replay["result"]["isError"], true);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn color_preset_preview_commit_and_replay_guard() {
        let mut c = Client::start_marker_controller(demo());
        c.init().await;
        let before = c.call(1, "sequence_inspect", json!({})).await;
        let before: Value = serde_json::from_str(before["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        let planned = c.call(2, "request_plan", json!({"request":"Apply color preset Golden Dusk to clip #32"})).await;
        assert_eq!(planned["result"]["isError"], false, "{planned}");
        let planned: Value = serde_json::from_str(planned["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(planned["execution"]["tool"], "color_preset_preview");
        let preview = c.call(3, "color_preset_preview", planned["execution"]["arguments"].clone()).await;
        assert_eq!(preview["result"]["isError"], false, "{preview}");
        let preview: Value = serde_json::from_str(preview["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(preview["target"], planned["target"]);
        assert_eq!(preview["preset"]["name"], "Golden Dusk");
        let unchanged = c.call(4, "sequence_inspect", json!({})).await;
        let unchanged: Value = serde_json::from_str(unchanged["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(unchanged, before);
        let commit = c.call(5, "color_preset_commit", json!({"plan_token":preview["plan_token"]})).await;
        assert_eq!(commit["result"]["isError"], false, "{commit}");
        let commit: Value = serde_json::from_str(commit["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(commit["verified"], true);
        let replay = c.call(6, "color_preset_commit", json!({"plan_token":preview["plan_token"]})).await;
        assert_eq!(replay["result"]["isError"], true);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn video_effect_preview_commit_and_replay_guard() {
        let mut c = Client::start_marker_controller(demo());
        c.init().await;
        let before = c.call(1, "sequence_inspect", json!({})).await;
        let before: Value = serde_json::from_str(before["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        let list = c.call(2, "video_effects", json!({})).await;
        let list: Value = serde_json::from_str(list["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert!(list["effects"].as_array().unwrap().iter().any(|e| e["id"] == "gaussian_blur"));
        assert!(!list["effects"].as_array().unwrap().iter().any(|e| e["id"] == "graphic_text"));
        let planned = c.call(3, "request_plan", json!({"request":"Apply video effect Gaussian Blur to clip #32"})).await;
        assert_eq!(planned["result"]["isError"], false, "{planned}");
        let planned: Value = serde_json::from_str(planned["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        let preview = c.call(4, "video_effect_preview", planned["execution"]["arguments"].clone()).await;
        assert_eq!(preview["result"]["isError"], false, "{preview}");
        let preview: Value = serde_json::from_str(preview["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(preview["target"], planned["target"]);
        assert_eq!(preview["effect"]["id"], "gaussian_blur");
        let unchanged = c.call(5, "sequence_inspect", json!({})).await;
        let unchanged: Value = serde_json::from_str(unchanged["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(unchanged, before);
        let commit = c.call(6, "video_effect_commit", json!({"plan_token":preview["plan_token"]})).await;
        assert_eq!(commit["result"]["isError"], false, "{commit}");
        let commit: Value = serde_json::from_str(commit["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(commit["verified"], true);
        let replay = c.call(7, "video_effect_commit", json!({"plan_token":preview["plan_token"]})).await;
        assert_eq!(replay["result"]["isError"], true);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn effect_param_preview_commit_and_range_guard() {
        let mut c = Client::start_marker_controller(demo());
        c.init().await;
        let effect_plan = c.call(1, "request_plan", json!({"request":"Apply video effect Gaussian Blur to clip #32"})).await;
        let effect_plan: Value = serde_json::from_str(effect_plan["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        let effect_preview = c.call(2, "video_effect_preview", effect_plan["execution"]["arguments"].clone()).await;
        let effect_preview: Value = serde_json::from_str(effect_preview["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        let applied = c.call(3, "video_effect_commit", json!({"plan_token":effect_preview["plan_token"]})).await;
        assert_eq!(applied["result"]["isError"], false, "{applied}");
        let before = c.call(4, "sequence_inspect", json!({})).await;
        let before: Value = serde_json::from_str(before["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        let planned = c.call(5, "request_plan", json!({"request":"Set parameter blurriness of effect gaussian_blur on clip #32 to 25"})).await;
        assert_eq!(planned["result"]["isError"], false, "{planned}");
        let planned: Value = serde_json::from_str(planned["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(planned["execution"]["tool"], "effect_param_preview");
        let mut invalid = planned["execution"]["arguments"].clone();
        invalid["value"] = json!(3001);
        let rejected = c.call(6, "effect_param_preview", invalid).await;
        assert_eq!(rejected["result"]["isError"], true);
        let preview = c.call(7, "effect_param_preview", planned["execution"]["arguments"].clone()).await;
        assert_eq!(preview["result"]["isError"], false, "{preview}");
        let preview: Value = serde_json::from_str(preview["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(preview["target"], planned["target"]);
        assert_eq!(preview["parameter"]["max"], 3000.0);
        let unchanged = c.call(8, "sequence_inspect", json!({})).await;
        let unchanged: Value = serde_json::from_str(unchanged["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(unchanged, before);
        let commit = c.call(9, "effect_param_commit", json!({"plan_token":preview["plan_token"]})).await;
        assert_eq!(commit["result"]["isError"], false, "{commit}");
        let commit: Value = serde_json::from_str(commit["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(commit["verified"], true);
        assert_eq!(commit["parameter"]["value"], "Float(25.0)");
        let replay = c.call(10, "effect_param_commit", json!({"plan_token":preview["plan_token"]})).await;
        assert_eq!(replay["result"]["isError"], true);

        let switch_plan = c.call(11, "request_plan", json!({"request":"Set parameter repeat_edge of effect gaussian_blur on clip #32 to true"})).await;
        assert_eq!(switch_plan["result"]["isError"], false, "{switch_plan}");
        let switch_plan: Value = serde_json::from_str(switch_plan["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        let switch_preview = c.call(12, "effect_param_preview", switch_plan["execution"]["arguments"].clone()).await;
        assert_eq!(switch_preview["result"]["isError"], false, "{switch_preview}");
        let switch_preview: Value = serde_json::from_str(switch_preview["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(switch_preview["parameter"]["type"], "bool");
        let switched = c.call(13, "effect_param_commit", json!({"plan_token":switch_preview["plan_token"]})).await;
        assert_eq!(switched["result"]["isError"], false, "{switched}");
        let switched: Value = serde_json::from_str(switched["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(switched["parameter"]["value"], "Bool(true)");

        let choice_plan = c.call(14, "request_plan", json!({"request":"Set parameter dimensions of effect gaussian_blur on clip #32 to Horizontal"})).await;
        assert_eq!(choice_plan["result"]["isError"], false, "{choice_plan}");
        let choice_plan: Value = serde_json::from_str(choice_plan["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        let mut invalid_option = choice_plan["execution"]["arguments"].clone();
        invalid_option["value"] = json!("No such option");
        let rejected = c.call(15, "effect_param_preview", invalid_option).await;
        assert_eq!(rejected["result"]["isError"], true);
        let choice_preview = c.call(16, "effect_param_preview", choice_plan["execution"]["arguments"].clone()).await;
        assert_eq!(choice_preview["result"]["isError"], false, "{choice_preview}");
        let choice_preview: Value = serde_json::from_str(choice_preview["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(choice_preview["parameter"]["type"], "choice");
        assert_eq!(choice_preview["resolved_value"], 1);
        let chosen = c.call(17, "effect_param_commit", json!({"plan_token":choice_preview["plan_token"]})).await;
        assert_eq!(chosen["result"]["isError"], false, "{chosen}");
        let chosen: Value = serde_json::from_str(chosen["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(chosen["parameter"]["value"], "Choice(1)");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn first_effect_keyframe_preview_commit_and_replay_guard() {
        let mut c = Client::start_marker_controller(demo());
        c.init().await;
        let apply = c.call(1, "request_plan", json!({"request":"Apply video effect Gaussian Blur to clip #32"})).await;
        let apply: Value = serde_json::from_str(apply["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        let preview = c.call(2, "video_effect_preview", apply["execution"]["arguments"].clone()).await;
        let preview: Value = serde_json::from_str(preview["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(c.call(3, "video_effect_commit", json!({"plan_token":preview["plan_token"]})).await["result"]["isError"], false);
        let planned = c
            .call(
                4,
                "request_plan",
                json!({"request":
            "Add keyframe for parameter blurriness of effect gaussian_blur on clip #32 at 0 seconds to 50"}),
            )
            .await;
        assert_eq!(planned["result"]["isError"], false, "{planned}");
        let planned: Value = serde_json::from_str(planned["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(planned["action"], "add_effect_keyframe");
        let before = c.call(5, "sequence_inspect", json!({})).await;
        let before: Value = serde_json::from_str(before["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        let preview = c.call(6, "effect_param_preview", planned["execution"]["arguments"].clone()).await;
        assert_eq!(preview["result"]["isError"], false, "{preview}");
        let preview: Value = serde_json::from_str(preview["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(preview["target"], planned["target"]);
        let unchanged = c.call(7, "sequence_inspect", json!({})).await;
        let unchanged: Value = serde_json::from_str(unchanged["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(unchanged, before);
        let committed = c.call(8, "effect_param_commit", json!({"plan_token":preview["plan_token"]})).await;
        assert_eq!(committed["result"]["isError"], false, "{committed}");
        let committed: Value = serde_json::from_str(committed["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(committed["verified"], true);
        assert_eq!(committed["parameter"]["keyframeDetails"][0]["value"], "Float(50.0)");
        assert_eq!(c.call(9, "effect_param_commit", json!({"plan_token":preview["plan_token"]})).await["result"]["isError"], true);

        let planned = c
            .call(
                10,
                "request_plan",
                json!({"request":
            "Change keyframe for parameter blurriness of effect gaussian_blur on clip #32 at 0 seconds to 75"}),
            )
            .await;
        assert_eq!(planned["result"]["isError"], false, "{planned}");
        let planned: Value = serde_json::from_str(planned["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(planned["target"]["before_keyframe_value"], "Float(50.0)");
        let before = c.call(11, "sequence_inspect", json!({})).await;
        let before: Value = serde_json::from_str(before["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        let preview = c.call(12, "effect_param_preview", planned["execution"]["arguments"].clone()).await;
        assert_eq!(preview["result"]["isError"], false, "{preview}");
        let preview: Value = serde_json::from_str(preview["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(preview["action"], "change_effect_keyframe");
        assert_eq!(preview["target"], planned["target"]);
        let unchanged = c.call(13, "sequence_inspect", json!({})).await;
        let unchanged: Value = serde_json::from_str(unchanged["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(unchanged, before);
        let committed = c.call(14, "effect_param_commit", json!({"plan_token":preview["plan_token"]})).await;
        assert_eq!(committed["result"]["isError"], false, "{committed}");
        let committed: Value = serde_json::from_str(committed["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(committed["verified"], true);
        assert_eq!(committed["parameter"]["keyframeDetails"][0]["value"], "Float(75.0)");
        assert_eq!(c.call(15, "effect_param_commit", json!({"plan_token":preview["plan_token"]})).await["result"]["isError"], true);

        let planned = c
            .call(
                16,
                "request_plan",
                json!({"request":
            "Add keyframe for parameter blurriness of effect gaussian_blur on clip #32 at 0.001 seconds to 100"}),
            )
            .await;
        assert_eq!(planned["result"]["isError"], false, "{planned}");
        let planned: Value = serde_json::from_str(planned["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        let mut duplicate = planned["execution"]["arguments"].clone();
        duplicate["clip_time_ticks"] = json!(0);
        assert_eq!(c.call(17, "effect_param_preview", duplicate).await["result"]["isError"], true);
        let preview = c.call(18, "effect_param_preview", planned["execution"]["arguments"].clone()).await;
        assert_eq!(preview["result"]["isError"], false, "{preview}");
        let preview: Value = serde_json::from_str(preview["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(preview["target"], planned["target"]);
        let committed = c.call(19, "effect_param_commit", json!({"plan_token":preview["plan_token"]})).await;
        assert_eq!(committed["result"]["isError"], false, "{committed}");
        let committed: Value = serde_json::from_str(committed["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(committed["parameter"]["keyframes"], 2);
        assert_eq!(committed["parameter"]["keyframeDetails"][0]["value"], "Float(75.0)");
        assert_eq!(committed["parameter"]["keyframeDetails"][1]["value"], "Float(100.0)");

        let planned = c
            .call(
                20,
                "request_plan",
                json!({"request":
            "Remove keyframe for parameter blurriness of effect gaussian_blur on clip #32 at 0.001 seconds"}),
            )
            .await;
        assert_eq!(planned["result"]["isError"], false, "{planned}");
        let planned: Value = serde_json::from_str(planned["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(planned["target"]["before_keyframe_value"], "Float(100.0)");
        let preview = c.call(21, "effect_param_preview", planned["execution"]["arguments"].clone()).await;
        assert_eq!(preview["result"]["isError"], false, "{preview}");
        let preview: Value = serde_json::from_str(preview["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(preview["action"], "remove_effect_keyframe");
        assert_eq!(preview["target"], planned["target"]);
        let committed = c.call(22, "effect_param_commit", json!({"plan_token":preview["plan_token"]})).await;
        assert_eq!(committed["result"]["isError"], false, "{committed}");
        let committed: Value = serde_json::from_str(committed["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(committed["parameter"]["keyframes"], 1);
        assert_eq!(committed["parameter"]["keyframeDetails"][0]["value"], "Float(75.0)");
        assert_eq!(c.call(23, "effect_param_commit", json!({"plan_token":preview["plan_token"]})).await["result"]["isError"], true);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn video_effect_remove_preview_commit_and_replay_guard() {
        let mut c = Client::start_marker_controller(demo());
        c.init().await;
        let apply_plan = c.call(1, "request_plan", json!({"request":"Apply video effect Gaussian Blur to clip #32"})).await;
        let apply_plan: Value = serde_json::from_str(apply_plan["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        let apply_preview = c.call(2, "video_effect_preview", apply_plan["execution"]["arguments"].clone()).await;
        let apply_preview: Value = serde_json::from_str(apply_preview["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        let applied = c.call(3, "video_effect_commit", json!({"plan_token":apply_preview["plan_token"]})).await;
        assert_eq!(applied["result"]["isError"], false, "{applied}");
        let before = c.call(4, "sequence_inspect", json!({})).await;
        let before: Value = serde_json::from_str(before["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        let planned = c.call(5, "request_plan", json!({"request":"Remove video effect gaussian_blur from clip #32"})).await;
        assert_eq!(planned["result"]["isError"], false, "{planned}");
        let planned: Value = serde_json::from_str(planned["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        let preview = c.call(6, "video_effect_remove_preview", planned["execution"]["arguments"].clone()).await;
        assert_eq!(preview["result"]["isError"], false, "{preview}");
        let preview: Value = serde_json::from_str(preview["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(preview["target"], planned["target"]);
        let unchanged = c.call(7, "sequence_inspect", json!({})).await;
        let unchanged: Value = serde_json::from_str(unchanged["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(unchanged, before);
        let removed = c.call(8, "video_effect_remove_commit", json!({"plan_token":preview["plan_token"]})).await;
        assert_eq!(removed["result"]["isError"], false, "{removed}");
        let removed: Value = serde_json::from_str(removed["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(removed["verified"], true);
        let replay = c.call(9, "video_effect_remove_commit", json!({"plan_token":preview["plan_token"]})).await;
        assert_eq!(replay["result"]["isError"], true);
    }
}

#[cfg(test)]
mod line_limit_tests {
    use super::*;
    #[tokio::test]
    async fn oversized_or_invalid_utf8_controller_lines_fail_without_panicking() {
        let oversized = vec![b'x'; MAX_CONTROLLER_LINE as usize + 1];
        let mut reader = tokio::io::BufReader::new(oversized.as_slice());
        assert_eq!(controller_line(&mut reader).await.unwrap_err().kind(), std::io::ErrorKind::InvalidData);
        let mut reader = tokio::io::BufReader::new(&b"\xff\n"[..]);
        assert!(controller_line(&mut reader).await.is_err());
    }
}
