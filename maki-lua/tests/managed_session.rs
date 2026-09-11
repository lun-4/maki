use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use maki_agent::tools::{ToolContext, ToolRegistry};
use maki_agent::{
    ActorBackend, AgentEvent, AgentInput, AgentLimits, AgentManagerHandle, AgentMode,
    BackendResult, ControlWork, DoneReason, GraphLifecycle, History, TurnContext, TurnOutcome,
    WorkKind,
};
use maki_lua::PluginHost;
use maki_providers::provider::{BoxFuture, Provider};
use maki_providers::{
    AgentError, Message, Model, ModelInfo, ProviderEvent, RequestOptions, StreamResponse,
};
use maki_storage::id::SessionRef;
use serde_json::{Value, json};

mod common;

const TOOL_NAME: &str = "managed_session";
const TIMEOUT_TOOL_NAME: &str = "managed_session_timeout";
const TIMEOUT_ERROR: &str = "session prompt timed out after 1s";
const CORRELATION: &str = "managed-root";
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(1);

const PLUGIN_SRC: &str = r#"
maki.api.register_tool({
  name = "managed_session_timeout",
  description = "time out a managed child prompt",
  schema = { type = "object", properties = {}, additionalProperties = false },
  audiences = { "main" },
  handler = function(_, ctx)
    local session, err = maki.agent.session(ctx, {
      name = "managed-timeout-child",
      inherit_provider = true,
    })
    if not session then
      return { llm_output = err, is_error = true }
    end
    local result, prompt_err = session:prompt("wait", { timeout = 1 })
    if result ~= nil or prompt_err ~= "session prompt timed out after 1s" then
      return { llm_output = "unexpected timeout result", is_error = true }
    end
    return "ok"
  end,
})

maki.api.register_tool({
  name = "managed_session",
  description = "create and close a managed child",
  schema = { type = "object", properties = {}, additionalProperties = false },
  audiences = { "main" },
  handler = function(_, ctx)
    local session, err = maki.agent.session(ctx, {
      name = "managed-child",
      inherit_provider = true,
    })
    if not session then
      return { llm_output = err, is_error = true }
    end
    local result, prompt_err = session:prompt("reply")
    session:close()
    if not result then
      return { llm_output = prompt_err, is_error = true }
    end
    return "ok"
  end,
})
"#;

struct LuaToolBackend {
    registry: Arc<ToolRegistry>,
    context: ToolContext,
    completed: flume::Sender<Result<(), String>>,
    tool_name: &'static str,
}

impl ActorBackend for LuaToolBackend {
    fn run_turn<'a>(
        &'a mut self,
        _: &'a mut History,
        context: TurnContext,
        _: AgentInput,
        _: WorkKind,
    ) -> Pin<Box<dyn Future<Output = BackendResult> + Send + 'a>> {
        Box::pin(async move {
            self.context.managed_turn = context.managed_turn;
            let invocation = self
                .registry
                .get(self.tool_name)
                .unwrap()
                .tool
                .parse(&json!({}))
                .unwrap();
            let result = invocation.execute(&self.context).await.output.map(|_| ());
            let _ = self.completed.send(result.clone());
            if result.is_err() {
                return BackendResult::SetupFailed {
                    agent_id: context.agent_id,
                    turn_id: context.turn_id.unwrap(),
                };
            }
            BackendResult::EnteredRun(TurnOutcome::Completed {
                agent_id: context.agent_id,
                turn_id: context.turn_id.unwrap(),
                usage: Default::default(),
                num_turns: 1,
                reason: DoneReason::EndTurn,
            })
        })
    }

    fn run_control<'a>(
        &'a mut self,
        _: &'a mut History,
        _: TurnContext,
        _: &'a ControlWork,
    ) -> Pin<Box<dyn Future<Output = BackendResult> + Send + 'a>> {
        Box::pin(async { BackendResult::ControlDone })
    }

    fn run_compact<'a>(
        &'a mut self,
        _: &'a mut History,
        _: TurnContext,
    ) -> Pin<Box<dyn Future<Output = BackendResult> + Send + 'a>> {
        Box::pin(async { BackendResult::CompactDone })
    }
}

struct PendingProvider {
    dropped: flume::Sender<()>,
}

struct DropSignal(flume::Sender<()>);

impl Drop for DropSignal {
    fn drop(&mut self) {
        let _ = self.0.send(());
    }
}

impl Provider for PendingProvider {
    fn stream_message<'a>(
        &'a self,
        _: &'a Model,
        _: &'a [Message],
        _: &'a str,
        _: &'a Value,
        _: &'a flume::Sender<ProviderEvent>,
        _: RequestOptions,
        _: Option<&'a SessionRef>,
    ) -> BoxFuture<'a, Result<StreamResponse, AgentError>> {
        Box::pin(async move {
            let _signal = DropSignal(self.dropped.clone());
            std::future::pending::<()>().await;
            unreachable!()
        })
    }

    fn list_models(&self) -> BoxFuture<'_, Result<Vec<ModelInfo>, AgentError>> {
        Box::pin(async { Ok(Vec::new()) })
    }
}

fn input() -> AgentInput {
    AgentInput {
        message: "create child".into(),
        mode: AgentMode::Build,
        images: Vec::new(),
        preamble: Vec::new(),
        thinking: Default::default(),
        fast: false,
        workflow: false,
        prompt: None,
    }
}

#[test]
fn managed_session_uses_root_authority_and_closes_its_node() {
    smol::block_on(async {
        let registry = Arc::new(ToolRegistry::new());
        let _host = PluginHost::new(Arc::clone(&registry)).unwrap();
        _host.load_source("managed-session", PLUGIN_SRC).unwrap();
        let (context, events, _cancel) = common::ctx_with_canned_provider();
        let manager = AgentManagerHandle::new(AgentLimits::default()).unwrap();
        let (completed_tx, completed_rx) = flume::bounded(1);
        let root = manager
            .create_root(
                Vec::new(),
                None,
                Box::new(LuaToolBackend {
                    registry,
                    context,
                    completed: completed_tx,
                    tool_name: TOOL_NAME,
                }),
            )
            .unwrap();
        let ticket = root
            .actor()
            .unwrap()
            .admit_turn(input(), None, CORRELATION.into())
            .unwrap();

        assert_eq!(
            completed_rx.recv_async().await.unwrap(),
            Ok(()),
            "managed Lua tool failed"
        );
        assert!(matches!(ticket.wait().await, TurnOutcome::Completed { .. }));
        let nodes = manager.snapshot();
        assert_eq!(nodes.len(), 2);
        let root_node = nodes
            .iter()
            .find(|node| node.agent_id == root.id())
            .unwrap();
        let child = nodes
            .iter()
            .find(|node| node.parent_id == Some(root.id()))
            .unwrap();
        assert_eq!(root_node.graph_lifecycle, GraphLifecycle::Live);
        assert_eq!(root_node.children, vec![child.agent_id]);
        assert_eq!(child.root_id, root.id());
        assert_eq!(child.depth, 1);
        assert_eq!(child.graph_lifecycle, GraphLifecycle::Closed);
        let envelope = events
            .try_iter()
            .find(|envelope| matches!(envelope.event, AgentEvent::SubagentHistory { .. }))
            .expect("managed child history envelope");
        assert_eq!(envelope.subagent.unwrap().agent_id, child.agent_id);

        let report = manager.shutdown(SHUTDOWN_TIMEOUT).await;
        assert!(report.timed_out.is_empty());
    });
}

#[test]
fn managed_prompt_timeout_returns_pair_closes_child_and_resumes_parent() {
    smol::block_on(async {
        let registry = Arc::new(ToolRegistry::new());
        let _host = PluginHost::new(Arc::clone(&registry)).unwrap();
        _host.load_source("managed-session", PLUGIN_SRC).unwrap();
        let (dropped_tx, dropped_rx) = flume::bounded(1);
        let provider = Arc::new(PendingProvider {
            dropped: dropped_tx,
        });
        let (mut context, _events, _cancel) = common::ctx_with_provider(provider);
        let manager = AgentManagerHandle::new(AgentLimits {
            max_concurrent_agent_turns: 1,
            ..AgentLimits::default()
        })
        .unwrap();
        let (completed_tx, completed_rx) = flume::bounded(1);
        let root = manager
            .create_root(
                Vec::new(),
                None,
                Box::new(LuaToolBackend {
                    registry,
                    context: context.clone(),
                    completed: completed_tx,
                    tool_name: TIMEOUT_TOOL_NAME,
                }),
            )
            .unwrap();
        context.managed_turn = None;
        let ticket = root
            .actor()
            .unwrap()
            .admit_turn(input(), None, CORRELATION.into())
            .unwrap();

        assert_eq!(
            completed_rx.recv_async().await.unwrap(),
            Ok(()),
            "{TIMEOUT_ERROR}"
        );
        assert!(matches!(ticket.wait().await, TurnOutcome::Completed { .. }));
        dropped_rx
            .recv_async()
            .await
            .expect("timed-out managed provider request remained alive");
        let child = manager
            .snapshot()
            .into_iter()
            .find(|node| node.parent_id == Some(root.id()))
            .expect("managed timeout child");
        assert_eq!(child.graph_lifecycle, GraphLifecycle::Closed);

        let report = manager.shutdown(SHUTDOWN_TIMEOUT).await;
        assert!(report.timed_out.is_empty());
    });
}
