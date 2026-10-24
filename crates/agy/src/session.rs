//! One ACP session: an agy conversation and the process running it.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};
use serde_json::{Value, json};

use crate::Bridge;
use crate::agy::{Agy, Killer, ModelList};
use crate::options::Settings;
use crate::translate::{self, Step, Translator, TurnResult};

/// How long agy may take to start and report its conversation.
const START_TIMEOUT: Duration = Duration::from_secs(60);

pub struct Session {
    /// agy's conversation id, which is also the ACP session id.
    pub id: String,
    cwd: PathBuf,
    program: PathBuf,
    models: Arc<ModelList>,
    settings: Mutex<Settings>,
    /// The agy process; `None` until the next turn after a cancel, an idle
    /// stop or a failure.
    process: Mutex<Option<Agy>>,
    /// Stops the process of the turn in progress (`session/cancel`).
    running: Mutex<Option<Killer>>,
    cancelled: AtomicBool,
    last_used: Mutex<Instant>,
}

impl Session {
    /// Starts agy and waits for it to name the conversation.
    pub fn start(
        program: PathBuf,
        cwd: PathBuf,
        resume: Option<&str>,
        models: Arc<ModelList>,
    ) -> Result<Self> {
        let settings = Settings::default();
        // The default model needs no model list, so these flags hold for the
        // first turn unless the user picks something else.
        let agy = Agy::spawn(&program, &cwd, resume, settings.flags(&[], false))?;
        let id = match agy.events.recv_timeout(START_TIMEOUT) {
            Ok(event) if event.get("event").and_then(Value::as_str) == Some("init") => event
                .get("conversation_id")
                .and_then(Value::as_str)
                .context("agy did not name its conversation")?
                .to_string(),
            Ok(_) => bail!("agy did not start a conversation"),
            Err(_) => bail!("{}", agy.failure()),
        };
        if resume.is_some_and(|wanted| wanted != id) {
            bail!(
                "agy could not resume conversation {}",
                resume.unwrap_or_default()
            );
        }
        Ok(Self {
            id,
            cwd,
            program,
            models,
            settings: Mutex::new(settings),
            process: Mutex::new(Some(agy)),
            running: Mutex::new(None),
            cancelled: AtomicBool::new(false),
            last_used: Mutex::new(Instant::now()),
        })
    }

    /// The options; models are listed once [`ModelList`] has them.
    pub fn config_options(&self) -> Value {
        let models = self.models.loaded().unwrap_or_default();
        self.settings.lock().unwrap().config_options(models)
    }

    /// Takes effect on the next turn (agy reads these flags at start).
    pub fn set_option(&self, config: &str, value: &str) -> Result<()> {
        let models = self.models.get();
        self.settings.lock().unwrap().set(models, config, value)
    }

    /// Runs one prompt to the end of the turn.
    pub fn prompt(&self, bridge: &Bridge, prompt: &Value) -> Result<Value> {
        self.cancelled.store(false, Ordering::SeqCst);
        let mut turn = self.run_turn(bridge, translate::prompt_content(prompt), false)?;

        // Headless agy refused something it needed permission for: ask, and
        // continue the conversation once if allowed.
        let mode = self.settings.lock().unwrap().mode;
        if !turn.refused.is_empty() && mode.asks_when_refused() && !self.is_cancelled() {
            let (id, title) = turn.failed_tool.clone().unwrap_or_else(|| {
                (
                    "permission".into(),
                    format!("Allow {}", turn.refused.join(", ")),
                )
            });
            // Headless agy has no way to allow one action: continuing runs
            // the rest of the turn with every permission. Said in the title,
            // and never a read, so nothing approves it without the user.
            let title =
                format!("{title} (agy then runs the rest of this turn with every permission)");
            let tool_call = json!({
                "toolCallId": id,
                "title": title,
                "kind": "other",
                "status": "pending",
            });
            if bridge.ask_permission(&self.id, tool_call)? && !self.is_cancelled() {
                let text = format!(
                    "The user granted permission for: {}. Continue where you left off.",
                    turn.refused.join(", ")
                );
                turn = self.run_turn(bridge, json!([{ "type": "text", "text": text }]), true)?;
            }
        }
        if self.is_cancelled() {
            return Ok(json!({ "stopReason": "cancelled" }));
        }
        if let Some(error) = turn.error {
            bail!("{error}");
        }
        Ok(json!({ "stopReason": "end_turn" }))
    }

    /// Sends `content` and relays events until agy ends the turn.
    fn run_turn(
        &self,
        bridge: &Bridge,
        content: Value,
        skip_permissions: bool,
    ) -> Result<TurnResult> {
        let models = self.models.get();
        let flags = self
            .settings
            .lock()
            .unwrap()
            .flags(models, skip_permissions);
        let mut process = self.process.lock().unwrap();
        *self.last_used.lock().unwrap() = Instant::now();
        if process.as_ref().is_none_or(|agy| agy.flags != flags) {
            // Stop the old process first: two must not share a conversation.
            *process = None;
            *process = Some(Agy::spawn(&self.program, &self.cwd, Some(&self.id), flags)?);
        }
        let agy = process.as_mut().context("agy is not running")?;
        *self.running.lock().unwrap() = Some(agy.killer());
        if self.is_cancelled() {
            agy.killer().kill();
        }

        let mut translator = Translator::new(&self.id, &self.cwd);
        let sent = agy.send(content);
        let result = loop {
            if sent.is_err() {
                break None;
            }
            match agy.events.recv() {
                Ok(event) => match translator.event(&event) {
                    Step::Updates(updates) => {
                        for update in updates {
                            bridge.notify("session/update", update);
                        }
                    }
                    Step::Done(result) => break Some(result),
                },
                // agy exited (or was stopped by a cancel).
                Err(_) => break None,
            }
        };
        *self.running.lock().unwrap() = None;
        *self.last_used.lock().unwrap() = Instant::now();
        match result {
            Some(result) => Ok(result),
            None => {
                let failure = agy.failure();
                *process = None;
                if self.is_cancelled() {
                    Ok(TurnResult::default())
                } else {
                    bail!("{failure}")
                }
            }
        }
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    /// Stops the turn in progress; the next turn resumes the conversation.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        if let Some(killer) = self.running.lock().unwrap().as_ref() {
            killer.kill();
        }
    }

    /// Stops agy, whether or not a turn is running.
    pub fn stop(&self) {
        self.cancel();
        if let Ok(mut process) = self.process.try_lock() {
            *process = None;
        }
    }

    /// Stops the process if no turn has used it for `idle`.
    pub fn stop_if_idle(&self, idle: Duration) {
        if self.last_used.lock().unwrap().elapsed() < idle {
            return;
        }
        // A running turn holds the lock; leave it alone.
        if let Ok(mut process) = self.process.try_lock() {
            *process = None;
        }
    }
}
