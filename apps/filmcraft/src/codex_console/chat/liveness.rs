//! Task progress, independent of voice traffic and upstream retry notices.
use super::*;

const IDLE_LIMIT: Duration = Duration::from_secs(600);
const CANCEL_GRACE: Duration = Duration::from_secs(30);

impl Turn {
    pub(super) fn note_progress(&mut self, method: &str, params: &Value, now: Instant) {
        if !self.busy || self.cancelled || self.active.is_none() || params.get("turnId").and_then(Value::as_str) != self.active.as_deref() {
            return;
        }
        let item = params.get("itemId").or_else(|| params.pointer("/item/id")).and_then(Value::as_str).is_some_and(|id| !id.is_empty() && id.len() <= 256);
        let delta = params.get("delta").and_then(Value::as_str).is_some_and(|text| !text.is_empty());
        let progress = match method {
            "item/started" | "item/completed" | "item/mcpToolCall/progress" => item,
            "item/agentMessage/delta"
            | "item/reasoning/summaryTextDelta"
            | "item/reasoning/textDelta"
            | "item/commandExecution/outputDelta"
            | "item/plan/delta" => item && delta,
            "turn/diff/updated" => params.get("diff").and_then(Value::as_str).is_some_and(|text| !text.is_empty()),
            "turn/plan/updated" => params.get("plan").and_then(Value::as_array).is_some_and(|plan| !plan.is_empty()),
            _ => false,
        };
        if progress {
            self.progress_at = Some(now);
        }
    }
    pub(super) fn poll_inactivity(&mut self, writer: &mut impl Write, wire: &Wire, thread: &str, now: Instant) -> Result<(), String> {
        if !self.busy {
            return Ok(());
        }
        if let Some(since) = self.idle_cancellation_since {
            if now.saturating_duration_since(since) >= CANCEL_GRACE {
                return Err("Codex did not confirm cancellation of its stalled task. Reconnect, then inspect the project before requesting any further edits; the saved conversation is preserved.".into());
            }
        } else if self.progress_at.is_some_and(|last| now.saturating_duration_since(last) >= IDLE_LIMIT) {
            // Never kill a working turn merely because its total duration reached ten minutes.
            // Cancel an actually silent worker once and wait for its real terminal event.
            self.idle_cancellation_since = Some(now);
            self.command(writer, wire, thread, CommandMessage::Cancel)?;
            wire.emit(ChatEvent::Retrying("No Codex task progress for ten minutes. Cancelling the stalled task; inspect the project before repeating any edits. Voice traffic and retry notices do not count as task progress.".into()))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(now: Instant) -> (Turn, Wire, mpsc::Receiver<ChatEvent>) {
        let (_, commands) = mpsc::channel();
        let (sender, events) = mpsc::sync_channel(64);
        let wire = Wire { process: Arc::new(Process::default()), session: None, commands, events: sender, ctx: egui::Context::default() };
        let turn = Turn { busy: true, active: Some("working-turn".into()), progress_at: Some(now - Duration::from_secs(601)), ..Default::default() };
        (turn, wire, events)
    }
    #[test]
    fn working_task_survives_ten_minutes_when_recent_tool_activity_arrives() {
        let now = Instant::now();
        let (mut turn, wire, events) = fixture(now);
        let mut output = Vec::new();
        turn.message(&mut output, &wire, "thread", json!({"method":"item/completed","params":{"threadId":"thread","turnId":"working-turn","item":{"id":"edit-call","type":"mcpToolCall","server":SERVER,"status":"completed"}}})).unwrap();
        turn.poll_inactivity(&mut output, &wire, "thread", now).unwrap();
        assert!(turn.busy && !turn.cancelled);
        assert!(turn.idle_cancellation_since.is_none());
        assert!(output.is_empty(), "Do not interrupt or replay a working task");
        assert!(events.try_iter().any(|event| matches!(event, ChatEvent::Inspected)));
    }
    #[test]
    fn two_hours_of_real_worker_progress_have_no_absolute_turn_deadline() {
        let base = Instant::now();
        let (mut turn, wire, _events) = fixture(base);
        let mut output = Vec::new();
        for minute in 0..120 {
            let now = base + Duration::from_secs(minute * 60);
            turn.note_progress("item/commandExecution/outputDelta", &json!({"turnId":"working-turn","itemId":"long-tool","delta":"progress"}), now);
            turn.poll_inactivity(&mut output, &wire, "thread", now + Duration::from_secs(59)).unwrap();
        }
        assert!(turn.busy && !turn.cancelled);
        assert!(output.is_empty());
    }
    #[test]
    fn voice_retry_noise_and_other_turns_do_not_hide_a_stalled_worker() {
        let now = Instant::now();
        let (mut turn, wire, _events) = fixture(now);
        let mut output = Vec::new();
        for message in [
            json!({"method":"error","params":{"threadId":"thread","turnId":"working-turn","willRetry":true,"error":{"message":"Reconnecting 2/5","codexErrorInfo":"responseStreamDisconnected"}}}),
            json!({"method":"thread/realtime/transcript","params":{"threadId":"thread","turnId":"working-turn","itemId":"voice","delta":"Still talking"}}),
            json!({"method":"item/agentMessage/delta","params":{"threadId":"other-thread","turnId":"working-turn","itemId":"message","delta":"Unrelated"}}),
            json!({"method":"item/agentMessage/delta","params":{"threadId":"thread","turnId":"other-turn","itemId":"message","delta":"Unrelated"}}),
            json!({"method":"item/agentMessage/delta","params":{"threadId":"thread","turnId":"working-turn","itemId":"message","delta":""}}),
        ] {
            turn.message(&mut output, &wire, "thread", message).unwrap();
        }
        turn.poll_inactivity(&mut output, &wire, "thread", now).unwrap();
        assert!(turn.busy && turn.cancelled, "Cancellation is not a completed edit");
        let first = output.clone();
        turn.poll_inactivity(&mut output, &wire, "thread", now + Duration::from_secs(29)).unwrap();
        assert_eq!(output, first);
        assert_eq!(String::from_utf8(output.clone()).unwrap().matches("turn/interrupt").count(), 1);
        assert!(!String::from_utf8(output).unwrap().contains("turn/start"));
        assert!(turn.poll_inactivity(&mut Vec::new(), &wire, "thread", now + Duration::from_secs(30)).unwrap_err().contains("did not confirm cancellation"));
    }
    #[test]
    fn acknowledged_stall_cancellation_keeps_connection_available_without_replay() {
        let now = Instant::now();
        let (mut turn, wire, events) = fixture(now);
        let mut output = Vec::new();
        turn.poll_inactivity(&mut output, &wire, "thread", now).unwrap();
        turn.message(
            &mut output,
            &wire,
            "thread",
            json!({"method":"turn/completed","params":{"threadId":"thread","turn":{"id":"working-turn","status":"interrupted"}}}),
        )
        .unwrap();
        let before = output.clone();
        turn.poll_inactivity(&mut output, &wire, "thread", now + Duration::from_secs(60)).unwrap();
        assert!(!turn.busy && turn.idle_cancellation_since.is_none());
        assert_eq!(before, output);
        assert!(events.try_iter().any(|event| matches!(event, ChatEvent::Finished)));
        turn.command(&mut output, &wire, "thread", CommandMessage::Prompt("New explicit request: inspect only".into(), None)).unwrap();
        assert!(turn.busy && !turn.cancelled);
    }
}
