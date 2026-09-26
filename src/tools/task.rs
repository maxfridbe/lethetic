use serde_json::json;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::icons;
use crate::client::StreamEvent;
use crate::config::Config;
use crate::headless;
use crate::tools::{FunctionDefinition, Tool, ToolExecution};

pub fn get_definition() -> Tool {
    Tool {
        tool_type: "function".to_string(),
        function: FunctionDefinition {
            name: "task".to_string(),
            description: "Spawn a sub-agent to handle a self-contained task autonomously. \
                The sub-agent has access to all tools (except task and ask_the_user) and runs \
                until it produces a final response. Use this to delegate complex, isolated \
                sub-problems — e.g. 'investigate and summarize all usages of X', \
                'refactor module Y and verify it compiles', 'research topic Z and write a report'. \
                The sub-agent cannot ask the user questions; make the prompt self-contained."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "prompt": {
                        "type": "string",
                        "description": "The full, self-contained task description for the sub-agent"
                    },
                    "description": {
                        "type": "string",
                        "description": "Short label shown in the UI (3-5 words)"
                    },
                    "tool_call_id": {
                        "type": "string",
                        "description": "A unique, descriptive string identifier for this call"
                    }
                },
                "required": ["prompt", "description", "tool_call_id"]
            }),
        },
    }
}

pub fn get_ui_description(arguments: &serde_json::Value) -> String {
    if let Some(desc) = arguments["description"].as_str() {
        return format!("{} Sub-agent: {}", icons::COMMAND, desc);
    }
    format!("{} Running sub-agent task", icons::COMMAND)
}

fn child_accounting_hook(
    parent_hook: Option<crate::client::RequestStartedHook>,
) -> Option<crate::client::RequestStartedHook> {
    parent_hook.map(|parent_hook| {
        std::sync::Arc::new(
            move |checkpoint: &crate::client::ProviderRequestCheckpoint| {
                let mut accounting_only = checkpoint.clone();
                accounting_only.transcript = None;
                parent_hook(&accounting_only)
            },
        ) as crate::client::RequestStartedHook
    })
}

pub async fn execute(
    prompt: &str,
    cwd: &str,
    cancellation_token: CancellationToken,
    tx: mpsc::UnboundedSender<StreamEvent>,
    client: &reqwest::Client,
    config: &Config,
) -> String {
    execute_with_hook(prompt, cwd, cancellation_token, tx, client, config, None).await
}

pub async fn execute_with_hook(
    prompt: &str,
    cwd: &str,
    cancellation_token: CancellationToken,
    tx: mpsc::UnboundedSender<StreamEvent>,
    client: &reqwest::Client,
    config: &Config,
    request_hook: Option<crate::client::RequestStartedHook>,
) -> String {
    execute_classified_with_hook(
        prompt,
        cwd,
        cancellation_token,
        tx,
        client,
        config,
        request_hook,
    )
    .await
    .output
}

pub(super) async fn execute_classified_with_hook(
    prompt: &str,
    cwd: &str,
    cancellation_token: CancellationToken,
    tx: mpsc::UnboundedSender<StreamEvent>,
    client: &reqwest::Client,
    config: &Config,
    request_hook: Option<crate::client::RequestStartedHook>,
) -> ToolExecution {
    let result = execute_with_hook_and_timeouts(
        prompt,
        cancellation_token,
        tx,
        client,
        config,
        request_hook,
        SubagentTimeouts::default(),
    )
    .await;
    ToolExecution {
        output: result.output,
        cwd: cwd.to_string(),
        is_error: result.is_error,
        provenance: crate::tools::ToolOutputProvenance::OrdinaryHost,
    }
}

#[derive(Clone, Copy)]
struct SubagentTimeouts {
    execution: std::time::Duration,
    containment: std::time::Duration,
}

impl Default for SubagentTimeouts {
    fn default() -> Self {
        Self {
            execution: std::time::Duration::from_secs(300),
            containment: std::time::Duration::from_secs(30),
        }
    }
}

struct SubagentExecution {
    output: String,
    is_error: bool,
}

impl SubagentExecution {
    fn success(output: impl Into<String>) -> Self {
        Self {
            output: output.into(),
            is_error: false,
        }
    }

    fn error(output: impl Into<String>) -> Self {
        Self {
            output: output.into(),
            is_error: true,
        }
    }
}

fn classify_finished_subagent(result: Result<headless::AgentRun, String>) -> SubagentExecution {
    match result {
        Ok(result) if result.text.trim().is_empty() => {
            SubagentExecution::success("Sub-agent completed but produced no output.")
        }
        Ok(result) => SubagentExecution::success(result.text),
        Err(error) => SubagentExecution::error(format!("Sub-agent failed: {error}")),
    }
}

async fn execute_with_hook_and_timeouts(
    prompt: &str,
    cancellation_token: CancellationToken,
    tx: mpsc::UnboundedSender<StreamEvent>,
    client: &reqwest::Client,
    config: &Config,
    request_hook: Option<crate::client::RequestStartedHook>,
    timeouts: SubagentTimeouts,
) -> SubagentExecution {
    let _ = tx.send(StreamEvent::ToolProgress(format!(
        "Sub-agent started: {}",
        prompt.chars().take(80).collect::<String>()
    )));

    let child_cancellation = cancellation_token.child_token();
    let mut agent = Box::pin(headless::run_agent_accounted_with_cancellation_and_hook(
        prompt.to_string(),
        client,
        config,
        false,
        Some(tx),
        Some(child_cancellation.clone()),
        child_accounting_hook(request_hook),
    ));
    enum AgentOutcome {
        Finished(Result<headless::AgentRun, String>),
        Cancelled,
        TimedOut,
    }
    let outcome = tokio::select! {
        biased;
        result = &mut agent => AgentOutcome::Finished(result),
        _ = cancellation_token.cancelled() => AgentOutcome::Cancelled,
        _ = tokio::time::sleep(timeouts.execution) => AgentOutcome::TimedOut,
    };
    match outcome {
        AgentOutcome::Finished(result) => classify_finished_subagent(result),
        AgentOutcome::Cancelled | AgentOutcome::TimedOut => {
            child_cancellation.cancel();
            let settled = tokio::time::timeout(timeouts.containment, &mut agent).await;
            let output = match (outcome, settled) {
                (AgentOutcome::Cancelled, Ok(_)) => "Sub-agent cancelled.".to_string(),
                (AgentOutcome::Cancelled, Err(_)) => {
                    "Sub-agent cancellation did not settle within 30 seconds.".to_string()
                }
                (AgentOutcome::TimedOut, Ok(_)) => {
                    "Sub-agent timed out after 5 minutes.".to_string()
                }
                (AgentOutcome::TimedOut, Err(_)) => {
                    "Sub-agent timed out after 5 minutes and cancellation did not settle within 30 seconds."
                        .to_string()
                }
                (AgentOutcome::Finished(_), _) => unreachable!("finished outcome handled above"),
            };
            SubagentExecution::error(output)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(
        request_id: &str,
        in_flight: bool,
        completed: bool,
    ) -> crate::accounting::ProviderRequestAccounting {
        crate::accounting::ProviderRequestAccounting {
            request_id: request_id.to_string(),
            connection_id: "child-connection".to_string(),
            model: "child-model".to_string(),
            usage: crate::accounting::Usage::default(),
            usage_reported: completed,
            estimated_cost: None,
            completed,
            in_flight,
        }
    }

    #[test]
    fn provider_failure_is_a_typed_tool_error_without_losing_local_detail() {
        let raw_error = "quota denied tenant violet req-subagent-42";
        let result = classify_finished_subagent(Err(raw_error.to_string()));
        assert!(result.is_error);
        assert_eq!(result.output, format!("Sub-agent failed: {raw_error}"));
    }

    #[tokio::test]
    async fn cancelled_subagent_forwards_terminal_checkpoint_failure_to_parent() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let config = Config {
            server_url: format!("http://{}/v1", listener.local_addr().unwrap()),
            model: "test-model".to_string(),
            context_size: 4096,
            connection_kind: crate::config::ConnectionKind::OpenAiChatCompletions,
            ..Default::default()
        };
        let hook: crate::client::RequestStartedHook = std::sync::Arc::new(|checkpoint| {
            if checkpoint.request.in_flight {
                Ok(())
            } else {
                Err("simulated sub-agent checkpoint failure".to_string())
            }
        });
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let (tx, mut rx) = mpsc::unbounded_channel();

        let result = execute_classified_with_hook(
            "inspect the workspace",
            ".",
            cancellation,
            tx,
            &reqwest::Client::new(),
            &config,
            Some(hook),
        )
        .await;

        assert!(result.is_error);
        assert!(
            result.output.contains("Sub-agent cancelled"),
            "{}",
            result.output
        );
        let mut started_id = None;
        let mut settlement = None;
        while let Ok(event) = rx.try_recv() {
            match event {
                StreamEvent::RequestStarted(request) => started_id = Some(request.request_id),
                StreamEvent::RequestSettlementFailed {
                    request_id,
                    error,
                    cancellation_requested,
                } => settlement = Some((request_id, error, cancellation_requested)),
                StreamEvent::RequestFinished(request) => panic!(
                    "failed sub-agent checkpoint emitted RequestFinished for {}",
                    request.request_id
                ),
                _ => {}
            }
        }
        let started_id = started_id.expect("sub-agent start was not forwarded");
        let (request_id, error, cancellation_requested) =
            settlement.expect("sub-agent settlement failure was not forwarded");
        assert_eq!(request_id, started_id);
        assert!(cancellation_requested);
        assert!(
            error.contains("provider cancellation accounting checkpoint failed"),
            "{error}"
        );
        assert!(error.contains("simulated sub-agent checkpoint failure"));
        drop(listener);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn timed_out_subagent_keeps_parent_settlement_route_after_private_receiver_drop() {
        use tokio::io::AsyncReadExt as _;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (server_release_tx, server_release_rx) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buffer = [0_u8; 4096];
            loop {
                let read = socket.read(&mut buffer).await.unwrap();
                if read == 0 || buffer[..read].windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                    break;
                }
            }
            server_release_rx.await.unwrap();
            drop(socket);
            true
        });
        let config = Config {
            server_url: format!("http://{address}/v1"),
            model: "test-model".to_string(),
            context_size: 4096,
            connection_kind: crate::config::ConnectionKind::OpenAiChatCompletions,
            ..Default::default()
        };
        let (terminal_entered_tx, terminal_entered_rx) = tokio::sync::oneshot::channel();
        let terminal_entered_tx =
            std::sync::Arc::new(std::sync::Mutex::new(Some(terminal_entered_tx)));
        let release =
            std::sync::Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
        let hook_sender = terminal_entered_tx.clone();
        let hook_release = release.clone();
        let hook: crate::client::RequestStartedHook = std::sync::Arc::new(move |checkpoint| {
            if checkpoint.request.in_flight {
                return Ok(());
            }
            if let Some(sender) = hook_sender.lock().unwrap().take() {
                let _ = sender.send(());
            }
            let (released, condition) = &*hook_release;
            let mut released = released.lock().unwrap();
            while !*released {
                released = condition.wait(released).unwrap();
            }
            Err("simulated late sub-agent checkpoint failure".to_string())
        });
        let (tx, mut rx) = mpsc::unbounded_channel();
        let execute = tokio::spawn(async move {
            execute_with_hook_and_timeouts(
                "inspect the workspace",
                CancellationToken::new(),
                tx,
                &reqwest::Client::new(),
                &config,
                Some(hook),
                SubagentTimeouts {
                    execution: std::time::Duration::from_millis(40),
                    containment: std::time::Duration::from_millis(40),
                },
            )
            .await
        });

        tokio::time::timeout(std::time::Duration::from_secs(2), terminal_entered_rx)
            .await
            .expect("provider did not enter terminal checkpoint after timeout")
            .expect("terminal checkpoint signal disappeared");
        let result = tokio::time::timeout(std::time::Duration::from_secs(2), execute)
            .await
            .expect("sub-agent containment deadline did not return")
            .expect("sub-agent task panicked");
        assert!(result.is_error);
        assert!(
            result.output.contains("cancellation did not settle"),
            "{}",
            result.output
        );

        let (released, condition) = &*release;
        *released.lock().unwrap() = true;
        condition.notify_all();

        let mut active = std::collections::BTreeSet::new();
        let mut started_count = 0_usize;
        let settlement = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                match rx.recv().await.expect("parent settlement route closed") {
                    StreamEvent::RequestStarted(request) => {
                        started_count = started_count.saturating_add(1);
                        active.insert(request.request_id);
                    }
                    StreamEvent::RequestSettlementFailed {
                        request_id,
                        error,
                        cancellation_requested,
                    } => {
                        let was_active = active.remove(&request_id);
                        break (request_id, error, cancellation_requested, was_active);
                    }
                    StreamEvent::RequestFinished(request) => panic!(
                        "failed late sub-agent checkpoint emitted RequestFinished for {}",
                        request.request_id
                    ),
                    _ => {}
                }
            }
        })
        .await
        .expect("late terminal settlement did not reach the parent");
        assert_eq!(
            started_count, 1,
            "parent received a duplicate nested request start"
        );
        assert!(settlement.2);
        assert!(
            settlement.3,
            "settlement ID was not the exact forwarded nested request marker"
        );
        assert!(
            settlement
                .1
                .contains("provider cancellation accounting checkpoint failed"),
            "{}",
            settlement.1
        );
        assert!(
            settlement
                .1
                .contains("simulated late sub-agent checkpoint failure"),
            "{}",
            settlement.1
        );
        assert!(
            active.is_empty(),
            "exact nested request marker remained live"
        );
        server_release_tx.send(()).unwrap();
        assert!(server.await.unwrap());
    }

    #[test]
    fn child_hook_forwards_accounting_but_strips_transcript() {
        let recorded = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded_for_hook = recorded.clone();
        let parent: crate::client::RequestStartedHook = std::sync::Arc::new(move |checkpoint| {
            recorded_for_hook.lock().unwrap().push(checkpoint.clone());
            Ok(())
        });
        let hook = child_accounting_hook(Some(parent)).unwrap();
        let started = request("child-request", true, false);
        let finished = request("child-request", false, true);

        hook(&crate::client::ProviderRequestCheckpoint {
            request: started.clone(),
            transcript: None,
        })
        .unwrap();
        hook(&crate::client::ProviderRequestCheckpoint {
            request: finished.clone(),
            transcript: Some(vec![crate::context::Message {
                role: "assistant".to_string(),
                content: "child-only transcript".to_string(),
                tool_calls: None,
                provider_content: Some(vec![serde_json::json!({
                    "type": "text",
                    "text": "child-only transcript"
                })]),
                tool_result_is_error: false,
            }]),
        })
        .unwrap();

        let recorded = recorded.lock().unwrap();
        assert_eq!(recorded.len(), 2);
        assert_eq!(recorded[0].request, started);
        assert_eq!(recorded[1].request, finished);
        assert!(
            recorded
                .iter()
                .all(|checkpoint| checkpoint.transcript.is_none())
        );
    }
}
