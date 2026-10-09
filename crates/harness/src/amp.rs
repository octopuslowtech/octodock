use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

use zeron_proto::{
    AgentEvent, DoneStatus, HarnessId, Model, ReasoningLevel, RunRequest, SteeringMode, ToolCall,
};

use crate::claude::wire::ImageBlock;
use crate::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use crate::{Harness, HarnessError, RunControls, Signal, send_signal, shutdown_child};

const MODES: [(&str, &str); 4] = [
    ("low", "Low"),
    ("medium", "Medium"),
    ("high", "High"),
    ("ultra", "Ultra"),
];

pub struct AmpHarness {
    executable: Option<PathBuf>,
    kill_grace: Duration,
}

impl Default for AmpHarness {
    fn default() -> Self {
        Self {
            executable: None,
            kill_grace: Duration::from_secs(3),
        }
    }
}

impl AmpHarness {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_executable(mut self, path: impl Into<PathBuf>) -> Self {
        self.executable = Some(path.into());
        self
    }

    fn resolve_executable(&self) -> Result<PathBuf, HarnessError> {
        if let Some(path) = &self.executable {
            return crate::executable::validate_native_override(path);
        }
        let extra = crate::executable::home_dir()
            .map(|home| vec![home.join(".local/bin/amp"), home.join(".amp/bin/amp")])
            .unwrap_or_default();
        crate::executable::find_on_paths("amp", extra).ok_or_else(|| {
            HarnessError::NotInstalled(
                "amp (searched PATH, the login shell's PATH, ~/.local/bin and ~/.amp/bin)".into(),
            )
        })
    }

    fn build_command(&self, exe: &PathBuf, request: &RunRequest) -> Command {
        let mut cmd = Command::new(exe);
        crate::compose_child_path(&mut cmd, exe);
        if let Some(thread) = &request.resume {
            cmd.args(["threads", "continue", thread]);
        }
        cmd.args([
            "--execute",
            "--stream-json",
            "--stream-json-thinking",
            "--stream-json-input",
            "--no-archive-after-execute",
        ]);
        if let Some(mode) = &request.model {
            cmd.args(["--mode", mode]);
        }
        if request.auto_approve {
            cmd.arg("--dangerously-allow-all");
        }
        if let Some(mcp) = &request.mcp {
            cmd.arg("--mcp-config");
            cmd.arg(
                json!({ &mcp.name: { "command": mcp.command, "args": mcp.args, "env": mcp.env } })
                    .to_string(),
            );
        }
        if !request.cwd.is_empty() {
            cmd.current_dir(&request.cwd);
        }
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        cmd
    }
}

#[async_trait]
impl Harness for AmpHarness {
    fn id(&self) -> HarnessId {
        HarnessId::Amp
    }
    fn display_name(&self) -> &str {
        "Amp"
    }
    fn supports_steering(&self) -> bool {
        true
    }
    fn steering_mode(&self) -> SteeringMode {
        SteeringMode::StepBoundary
    }
    fn reasoning_levels(&self) -> &[ReasoningLevel] {
        &[]
    }
    fn installed(&self) -> bool {
        self.resolve_executable().is_ok()
    }
    fn executable_path(&self) -> Option<PathBuf> {
        self.resolve_executable().ok()
    }
    fn deterministic_turn_end(&self) -> bool {
        true
    }
    fn fallback_models(&self) -> Vec<Model> {
        MODES
            .iter()
            .map(|(id, label)| Model {
                id: (*id).into(),
                label: (*label).into(),
                description: None,
                reasoning_levels: Vec::new(),
                options: Vec::new(),
            })
            .collect()
    }
    async fn models(&self) -> Result<Vec<Model>, HarnessError> {
        self.resolve_executable()?;
        Ok(self.fallback_models())
    }

    async fn run(
        &self,
        request: RunRequest,
        controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        let exe = self.resolve_executable()?;
        let mut child = self.build_command(&exe, &request).spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                HarnessError::NotInstalled(crate::executable::binary_hint(&exe))
            } else {
                HarnessError::Io(e)
            }
        })?;
        let mut stdin = child.stdin.take();
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| HarnessError::Protocol("amp child has no stdout".into()))?;
        let stderr_tail = crate::StderrTail::default();
        if let Some(stderr) = child.stderr.take() {
            let tail = stderr_tail.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::debug!(target: "zeron_harness::amp", "stderr: {line}");
                    tail.push(&line);
                }
            });
        }

        let images = crate::claude::load_image_blocks(&request.attachments).await;
        write_line(&mut stdin, &user_line(&request.prompt, &images, false)).await;

        let (event_tx, event_rx) = mpsc::channel::<Result<AgentEvent, HarnessError>>(256);
        tokio::spawn(run_session(Session {
            norm: Normalizer::new(),
            child,
            stdout: BufReader::new(stdout).lines(),
            stdin,
            event_tx,
            controls,
            kill_grace: self.kill_grace,
            stderr_tail,
        }));

        Ok(futures::stream::unfold(event_rx, |mut rx| async move {
            rx.recv().await.map(|ev| (ev, rx))
        })
        .boxed())
    }
}

fn user_line(text: &str, images: &[ImageBlock], steer: bool) -> String {
    let mut content = vec![json!({ "type": "text", "text": text })];
    content.extend(images.iter().map(|image| {
        json!({
            "type": "image",
            "source": { "type": "base64", "media_type": image.media_type, "data": image.data },
        })
    }));
    let mut line = json!({ "type": "user", "message": { "role": "user", "content": content } });
    if steer {
        line["steer"] = Value::Bool(true);
    }
    line.to_string()
}

async fn write_line(stdin: &mut Option<ChildStdin>, line: &str) {
    let Some(pipe) = stdin else {
        return;
    };
    let write = async {
        pipe.write_all(line.as_bytes()).await?;
        pipe.write_all(b"\n").await?;
        pipe.flush().await
    };
    if let Err(e) = write.await {
        tracing::debug!(target: "zeron_harness::amp", "stdin write failed (tolerated): {e}");
        *stdin = None;
    }
}

fn first_str(input: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| input.get(*key).and_then(Value::as_str))
        .map(str::to_owned)
}

fn decode_tool_use(name: &str, input: &Value) -> ToolCall {
    let field = |keys: &[&str]| first_str(input, keys);
    let raw = || (!input.is_null()).then(|| input.clone());
    let path = || field(&["path", "file_path"]).unwrap_or_default();
    match name {
        "shell_command" | "Bash" => ToolCall::Exec {
            command: field(&["command", "cmd"]).unwrap_or_default(),
        },
        "Read" | "read_file" => ToolCall::ReadFile { path: path() },
        "create_file" => ToolCall::WriteFile {
            path: path(),
            content: field(&["content"]),
        },
        "edit_file" => ToolCall::EditFile {
            path: path(),
            old_string: field(&["old_str", "old_string"]),
            new_string: field(&["new_str", "new_string"]),
        },
        "apply_patch" => ToolCall::ApplyPatch {
            path: field(&["path"]),
        },
        "Grep" => ToolCall::Search {
            pattern: field(&["pattern"]).unwrap_or_default(),
            path: field(&["path"]),
        },
        "glob" => ToolCall::Glob {
            pattern: field(&["filePattern", "pattern"]).unwrap_or_default(),
        },
        "read_web_page" => ToolCall::WebFetch {
            url: field(&["url"]).unwrap_or_default(),
            prompt: field(&["objective", "prompt"]),
        },
        "web_search" => ToolCall::WebSearch {
            query: field(&["query", "objective"]).unwrap_or_default(),
        },
        "Task" => ToolCall::Unknown {
            name: match field(&["description"]) {
                Some(description) if !description.is_empty() => format!("Agent: {description}"),
                _ => "Agent".into(),
            },
            input: raw(),
        },
        _ => match name.strip_prefix("mcp__").and_then(|r| r.split_once("__")) {
            Some((server, tool)) => ToolCall::Mcp {
                server: server.into(),
                tool: tool.into(),
                input: raw(),
            },
            None => ToolCall::Unknown {
                name: name.into(),
                input: raw(),
            },
        },
    }
}

fn tag(parent: &str, event: AgentEvent) -> AgentEvent {
    AgentEvent::Subagent {
        parent_tool_use_id: parent.to_owned(),
        event: Box::new(event),
    }
}

fn blocks(frame: &Value) -> &[Value] {
    frame["message"]["content"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
}

fn opt_string(value: &Value) -> Option<String> {
    value.as_str().map(str::to_owned)
}

struct Normalizer {
    session_id: Option<String>,
    message_id: String,
    saw_init: bool,
    spawns: HashSet<String>,
    prompt_echoed: bool,
    pending_steers: usize,
    turn_ended: bool,
    last_error: Option<String>,
}

impl Normalizer {
    fn new() -> Self {
        Self {
            session_id: None,
            message_id: uuid::Uuid::new_v4().to_string(),
            saw_init: false,
            spawns: HashSet::new(),
            prompt_echoed: false,
            pending_steers: 0,
            turn_ended: false,
            last_error: None,
        }
    }

    fn rotate(&mut self) -> (String, String) {
        let next = uuid::Uuid::new_v4().to_string();
        let prev = std::mem::replace(&mut self.message_id, next.clone());
        (prev, next)
    }

    fn normalize(&mut self, frame: &Value, interrupted: bool) -> Vec<AgentEvent> {
        let parent = frame["parent_tool_use_id"].as_str();
        match frame["type"].as_str().unwrap_or("") {
            "system" => self.system(frame),
            "assistant" => self.assistant(frame, parent),
            "user" => self.user(frame, parent),
            "result" => self.result(frame, interrupted),
            _ => Vec::new(),
        }
    }

    fn system(&mut self, frame: &Value) -> Vec<AgentEvent> {
        if frame["subtype"] != "init" {
            if let Some(error) = opt_string(&frame["error"]) {
                self.last_error = Some(error);
                self.turn_ended = true;
            }
            return Vec::new();
        }
        if self.saw_init {
            return Vec::new();
        }
        self.saw_init = true;
        let session_id = opt_string(&frame["session_id"]).unwrap_or_default();
        self.session_id = Some(session_id.clone());
        vec![AgentEvent::SessionStarted {
            harness: HarnessId::Amp,
            model: opt_string(&frame["agent_mode"]).unwrap_or_default(),
            tools: serde_json::from_value(frame["tools"].clone()).unwrap_or_default(),
            cwd: opt_string(&frame["cwd"]).unwrap_or_default(),
            session_id,
            assistant_message_id: self.message_id.clone(),
        }]
    }

    fn assistant(&mut self, frame: &Value, parent: Option<&str>) -> Vec<AgentEvent> {
        let mut out = Vec::new();
        for block in blocks(frame) {
            match block["type"].as_str().unwrap_or("") {
                "thinking" => {
                    if let Some(text) = opt_string(&block["thinking"]).filter(|t| !t.is_empty()) {
                        out.push(AgentEvent::ReasoningDelta { text });
                    }
                }
                "text" => {
                    if let Some(text) = opt_string(&block["text"]).filter(|t| !t.is_empty()) {
                        out.push(AgentEvent::TextDelta {
                            text: if parent.is_some() {
                                format!("{}\n\n", text.trim_end())
                            } else {
                                text
                            },
                        });
                    }
                }
                "tool_use" => {
                    let id = opt_string(&block["id"]).unwrap_or_default();
                    let call =
                        decode_tool_use(block["name"].as_str().unwrap_or(""), &block["input"]);
                    let spawn = parent.is_none() && call.is_subagent_spawn();
                    out.push(AgentEvent::ToolCall {
                        id: id.clone(),
                        call,
                    });
                    if spawn {
                        if let Some(text) =
                            opt_string(&block["input"]["prompt"]).filter(|p| !p.trim().is_empty())
                        {
                            out.push(tag(&id, AgentEvent::UserMessage { text }));
                        }
                        self.spawns.insert(id);
                    }
                }
                _ => {}
            }
        }
        if let Some(parent) = parent {
            return out.into_iter().map(|event| tag(parent, event)).collect();
        }
        let usage = &frame["message"]["usage"];
        let fields = [
            "input_tokens",
            "cache_read_input_tokens",
            "cache_creation_input_tokens",
        ];
        if fields.iter().any(|key| usage[*key].is_u64()) {
            out.push(AgentEvent::ContextUsage {
                tokens: Some(
                    fields
                        .iter()
                        .filter_map(|key| usage[*key].as_u64())
                        .fold(0, u64::saturating_add),
                ),
                window: usage["max_tokens"].as_u64().filter(|n| *n > 0),
            });
        }
        let (prev, _) = self.rotate();
        out.push(AgentEvent::AssistantMessageCompleted {
            assistant_message_id: prev,
        });
        self.turn_ended = frame["message"]["stop_reason"] != "tool_use";
        out
    }

    fn user(&mut self, frame: &Value, parent: Option<&str>) -> Vec<AgentEvent> {
        let mut out = Vec::new();
        let mut has_text = false;
        for block in blocks(frame) {
            match block["type"].as_str().unwrap_or("") {
                "tool_result" => {
                    let id = opt_string(&block["tool_use_id"]).unwrap_or_default();
                    let is_error = block["is_error"].as_bool().unwrap_or(false);
                    let settled = parent.is_none() && self.spawns.remove(&id);
                    out.push(AgentEvent::ToolResult {
                        id: id.clone(),
                        is_error,
                        output: None,
                        diff: None,
                    });
                    if settled {
                        out.push(tag(
                            &id,
                            AgentEvent::Done {
                                status: if is_error {
                                    DoneStatus::Errored
                                } else {
                                    DoneStatus::Completed
                                },
                                result: None,
                                error: None,
                                session_id: None,
                            },
                        ));
                    }
                }
                "text" => has_text = true,
                _ => {}
            }
        }
        if let Some(parent) = parent {
            return out.into_iter().map(|event| tag(parent, event)).collect();
        }
        if has_text {
            self.turn_ended = false;
            if !self.prompt_echoed {
                self.prompt_echoed = true;
            } else if self.pending_steers > 0 {
                self.pending_steers -= 1;
                let (prev, next) = self.rotate();
                out.push(AgentEvent::Steered {
                    assistant_message_id: Some(prev),
                    next_assistant_message_id: Some(next),
                });
            }
        }
        out
    }

    fn result(&mut self, frame: &Value, interrupted: bool) -> Vec<AgentEvent> {
        let session_id = opt_string(&frame["session_id"]).or_else(|| self.session_id.clone());
        let success = frame["subtype"] == "success";
        let mut out = Vec::new();
        let usage = &frame["usage"];
        if usage.is_object() {
            out.push(AgentEvent::Usage {
                input_tokens: usage["input_tokens"].as_u64().unwrap_or(0),
                output_tokens: usage["output_tokens"].as_u64().unwrap_or(0),
            });
        }
        out.push(AgentEvent::Done {
            status: if interrupted {
                DoneStatus::Interrupted
            } else if success {
                DoneStatus::Completed
            } else {
                DoneStatus::Errored
            },
            result: opt_string(&frame["result"]).filter(|_| success),
            error: (!success).then(|| {
                opt_string(&frame["error"])
                    .or_else(|| self.last_error.take())
                    .unwrap_or_else(|| "Amp run failed".into())
            }),
            session_id,
        });
        out
    }
}

struct Session {
    norm: Normalizer,
    child: Child,
    stdout: tokio::io::Lines<BufReader<ChildStdout>>,
    stdin: Option<ChildStdin>,
    event_tx: mpsc::Sender<Result<AgentEvent, HarnessError>>,
    controls: RunControls,
    kill_grace: Duration,
    stderr_tail: crate::StderrTail,
}

async fn run_session(session: Session) {
    let Session {
        mut norm,
        mut child,
        mut stdout,
        mut stdin,
        event_tx,
        controls,
        kill_grace,
        stderr_tail,
    } = session;
    let RunControls {
        realtime: _,
        execution_lease: _execution_lease,
        request_input: _,
        mut steering,
        interrupt,
        turn: _,
    } = controls;

    let mut steering_open = true;
    let mut interrupted = false;
    let mut any_done = false;
    let mut escalation: Option<tokio::task::JoinHandle<()>> = None;

    'main: loop {
        tokio::select! {
            line = stdout.next_line() => match line {
                Ok(Some(line)) => {
                    let Ok(frame) = serde_json::from_str::<Value>(line.trim()) else {
                        continue;
                    };
                    for event in norm.normalize(&frame, interrupted) {
                        any_done |= matches!(event, AgentEvent::Done { .. });
                        if event_tx.send(Ok(event)).await.is_err() {
                            break 'main;
                        }
                    }
                    if norm.turn_ended && norm.pending_steers == 0 {
                        steering_open = false;
                        stdin = None;
                    }
                }
                Ok(None) => break 'main,
                Err(e) => {
                    let _ = event_tx.send(Err(HarnessError::Io(e))).await;
                    break 'main;
                }
            },

            steer = steering.recv(), if steering_open && !interrupted => match steer {
                Some(message) => {
                    norm.pending_steers += 1;
                    write_line(&mut stdin, &user_line(&message.prompt, &[], true)).await;
                }
                None => {
                    steering_open = false;
                    stdin = None;
                }
            },

            _ = interrupt.cancelled(), if !interrupted => {
                interrupted = true;
                stdin = None;
                if let Some(target) = crate::process::signal_target(&child) {
                    send_signal(&target, Signal::Term);
                    escalation = Some(tokio::spawn(async move {
                        tokio::time::sleep(kill_grace).await;
                        send_signal(&target, Signal::Kill);
                    }));
                }
            },

            _ = event_tx.closed() => break 'main,
        }
    }

    if !any_done && !event_tx.is_closed() {
        let done = if interrupted {
            AgentEvent::Done {
                status: DoneStatus::Interrupted,
                result: None,
                error: None,
                session_id: norm.session_id.clone(),
            }
        } else {
            let status = child.try_wait().ok().flatten();
            AgentEvent::Done {
                status: DoneStatus::Errored,
                result: None,
                error: Some(
                    norm.last_error
                        .take()
                        .unwrap_or_else(|| crate::crash_message("amp", status, &stderr_tail)),
                ),
                session_id: norm.session_id.clone(),
            }
        };
        let _ = event_tx.send(Ok(done)).await;
    }

    shutdown_child(&mut child, kill_grace).await;
    if let Some(handle) = escalation {
        handle.abort();
    }
}
