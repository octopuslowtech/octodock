#![cfg(unix)]

use std::path::PathBuf;
use std::time::Duration;

use futures::StreamExt;
use tokio::sync::{mpsc, oneshot};

use zeron_harness::{AmpHarness, CancellationToken, Harness, RunControls, SteerMessage};
use zeron_proto::{AgentEvent, DoneStatus, HarnessId, RunRequest, SandboxLevel, ToolCall};

fn harness() -> AmpHarness {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake-amp.sh");
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
    AmpHarness::new().with_executable(path)
}

fn request(prompt: &str) -> RunRequest {
    RunRequest {
        mcp: None,
        prompt: prompt.into(),
        harness: None,
        model: None,
        reasoning: None,
        model_options: serde_json::Map::new(),
        cwd: String::new(),
        sandbox: SandboxLevel::DangerFullAccess,
        auto_approve: false,
        attachments: Vec::new(),
        worktree: None,
        resume: None,
    }
}

fn controls() -> (RunControls, mpsc::Sender<SteerMessage>, CancellationToken) {
    let (steer_tx, steer_rx) = mpsc::channel(8);
    let token = CancellationToken::new();
    let controls = RunControls {
        realtime: None,
        execution_lease: None,
        request_input: Box::new(|_| oneshot::channel().1),
        steering: steer_rx,
        interrupt: token.clone(),
        turn: Default::default(),
    };
    (controls, steer_tx, token)
}

async fn run_to_end(req: RunRequest, controls: RunControls) -> Vec<AgentEvent> {
    let stream = harness().run(req, controls).await.expect("run starts");
    tokio::time::timeout(
        Duration::from_secs(10),
        stream.map(|r| r.expect("stream event")).collect::<Vec<_>>(),
    )
    .await
    .expect("run finished in time")
}

fn dones(events: &[AgentEvent]) -> Vec<&AgentEvent> {
    events
        .iter()
        .filter(|e| matches!(e, AgentEvent::Done { .. }))
        .collect()
}

#[tokio::test]
async fn happy_path_normalizes_and_settles_on_the_result_frame() {
    let (controls, _steer, _token) = controls();
    let events = run_to_end(request("scenario:happy"), controls).await;

    assert!(matches!(
        &events[0],
        AgentEvent::SessionStarted { harness: HarnessId::Amp, model, session_id, .. }
            if model == "low" && session_id == "T-1"
    ));
    assert!(events.contains(&AgentEvent::ToolCall {
        id: "TU-1".into(),
        call: ToolCall::Exec {
            command: "echo hi".into()
        },
    }));
    assert!(events.contains(&AgentEvent::ToolResult {
        id: "TU-1".into(),
        is_error: false,
        output: None,
        diff: None,
    }));
    assert!(events.contains(&AgentEvent::TextDelta {
        text: "pong".into()
    }));
    assert!(events.contains(&AgentEvent::ContextUsage {
        tokens: Some(6),
        window: Some(1000),
    }));
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, AgentEvent::ReasoningDelta { .. })),
        "empty thinking blocks stay silent: {events:?}"
    );
    assert_eq!(
        dones(&events),
        vec![&AgentEvent::Done {
            status: DoneStatus::Completed,
            result: Some("pong".into()),
            error: None,
            session_id: Some("T-1".into()),
        }]
    );
}

#[tokio::test]
async fn steer_is_written_with_the_steer_flag_and_confirmed_by_its_echo() {
    let (controls, steer, _token) = controls();
    steer
        .send(SteerMessage {
            prompt: "change course".into(),
            message_id: None,
            attachments: Vec::new(),
            config: None,
        })
        .await
        .unwrap();
    let events = run_to_end(request("scenario:steer"), controls).await;

    let steered = events
        .iter()
        .position(|e| matches!(e, AgentEvent::Steered { .. }))
        .expect("steer confirmed");
    let text = events
        .iter()
        .position(|e| matches!(e, AgentEvent::TextDelta { text } if text == "steered"))
        .expect("post-steer text");
    assert!(steered < text, "{events:?}");
    assert_eq!(dones(&events).len(), 1);
}

#[tokio::test]
async fn resume_mode_and_auto_approve_ride_the_command_line() {
    let (controls, _steer, _token) = controls();
    let mut req = request("scenario:args");
    req.resume = Some("T-9".into());
    req.model = Some("high".into());
    req.auto_approve = true;
    let events = run_to_end(req, controls).await;

    let args = events
        .iter()
        .find_map(|e| match e {
            AgentEvent::TextDelta { text } => Some(text.as_str()),
            _ => None,
        })
        .expect("args echoed");
    assert!(args.starts_with("threads continue T-9 --execute"), "{args}");
    assert!(args.contains("--stream-json-input"), "{args}");
    assert!(args.contains("--mode high"), "{args}");
    assert!(args.contains("--dangerously-allow-all"), "{args}");
}

#[tokio::test]
async fn interrupt_terminates_the_cli_and_settles_interrupted() {
    let (controls, _steer, token) = controls();
    let mut stream = harness()
        .run(request("scenario:hang"), controls)
        .await
        .expect("run starts");
    let first = stream.next().await.expect("init").expect("event");
    assert!(matches!(first, AgentEvent::SessionStarted { .. }));
    token.cancel();
    let rest = tokio::time::timeout(
        Duration::from_secs(10),
        stream.map(|r| r.expect("stream event")).collect::<Vec<_>>(),
    )
    .await
    .expect("interrupt settles");
    assert_eq!(
        rest,
        vec![AgentEvent::Done {
            status: DoneStatus::Interrupted,
            result: None,
            error: None,
            session_id: Some("T-1".into()),
        }]
    );
}

#[tokio::test]
async fn failed_result_settles_errored_with_the_cli_error() {
    let (controls, _steer, _token) = controls();
    let events = run_to_end(request("scenario:error"), controls).await;
    assert_eq!(
        dones(&events),
        vec![&AgentEvent::Done {
            status: DoneStatus::Errored,
            result: None,
            error: Some("boom".into()),
            session_id: Some("T-1".into()),
        }]
    );
}

async fn live_turn(prompt: &str, resume: Option<String>) -> (Vec<String>, Option<String>) {
    let (controls, _steer, _token) = controls();
    let mut req = request(prompt);
    req.model = Some("low".into());
    req.auto_approve = true;
    req.resume = resume;
    let stream = AmpHarness::new()
        .run(req, controls)
        .await
        .expect("run starts");
    let events = tokio::time::timeout(
        Duration::from_secs(120),
        stream.map(|r| r.expect("stream event")).collect::<Vec<_>>(),
    )
    .await
    .expect("run finished in time");
    let texts = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::TextDelta { text } => Some(text.clone()),
            _ => None,
        })
        .collect();
    match dones(&events)[..] {
        [
            AgentEvent::Done {
                status: DoneStatus::Completed,
                session_id,
                ..
            },
        ] => (texts, session_id.clone()),
        _ => panic!("unexpected end: {events:?}"),
    }
}

#[tokio::test]
#[ignore = "spends Amp credits against the real CLI"]
async fn live_smoke_runs_and_resumes_a_thread() {
    let (first, thread) = live_turn("Reply with exactly: pong", None).await;
    assert_eq!(first, vec!["pong".to_string()]);
    let (second, resumed) = live_turn("Reply with exactly: pong2", thread.clone()).await;
    assert_eq!(second, vec!["pong2".to_string()]);
    assert_eq!(resumed, thread);
}
