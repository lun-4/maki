use std::sync::Arc;

use maki_agent::actor::AgentActorHandle;
use maki_agent::{AgentId, AgentManagerHandle, CancelMap, CancelTrigger, TurnCancellationReason};

use super::AgentCommand;
use super::shared_queue::correlation;

/// Routes commands from the UI to the actor's cancellation APIs.
///
/// `AgentCommand::Cancel { run_id }` maps to the root actor's run_id-correlated
/// cancel, which targets only that run's active/queued/pre-admission work.
/// `CancelAll` cancels the manager's entire root subtree, compatibility
/// subagents outside that graph, and startup so an early command aborts MCP
/// readiness.
pub(super) fn spawn_command_router(
    cmd_rx: flume::Receiver<AgentCommand>,
    actor: Arc<AgentActorHandle>,
    manager: AgentManagerHandle,
    root_id: AgentId,
    subagent_cancels: Arc<CancelMap<String>>,
    init_trigger: CancelTrigger,
) {
    // `CancelTrigger` is single-fire and not `Clone`: one startup trigger is
    // consumed by the first CancelAll (aborting MCP readiness) while later
    // CancelAll calls still cancel managed and compatibility subagents.
    let mut init_trigger = Some(init_trigger);
    smol::spawn(async move {
        while let Ok(cmd) = cmd_rx.recv_async().await {
            match cmd {
                AgentCommand::Cancel { run_id } => {
                    actor.cancel_correlation(&correlation(run_id), TurnCancellationReason::User);
                }
                AgentCommand::CancelAll => {
                    if let Some(trigger) = init_trigger.take() {
                        trigger.cancel();
                    }
                    let _ = manager.cancel_subtree(root_id);
                    subagent_cancels.cancel_all();
                }
                AgentCommand::CancelSubagent { tool_use_id } => {
                    subagent_cancels.cancel_or_precancel(tool_use_id);
                }
            }
        }
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::pin::Pin;
    use std::time::Duration;

    use maki_agent::{
        ActorBackend, ActorLifecycle, AgentInput, AgentLimits, AgentMetadata, AgentMode,
        BackendResult, ControlWork, History, TurnContext, TurnOutcome, WorkKind,
    };
    use maki_providers::TokenUsage;

    use super::*;

    struct CancellableBackend {
        current: Option<flume::Sender<maki_agent::CurrentManagedTurn>>,
        entered: Option<flume::Sender<()>>,
    }

    impl ActorBackend for CancellableBackend {
        fn run_turn<'a>(
            &'a mut self,
            _: &'a mut History,
            context: TurnContext,
            _: AgentInput,
            _: WorkKind,
        ) -> Pin<Box<dyn Future<Output = BackendResult> + Send + 'a>> {
            Box::pin(async move {
                if let Some(current) = &self.current {
                    current.send(context.managed_turn.clone().unwrap()).unwrap();
                }
                if let Some(entered) = &self.entered {
                    entered.send(()).unwrap();
                }
                let reason = context.cancel_reason.cancelled().await;
                BackendResult::EnteredRun(TurnOutcome::Cancelled {
                    agent_id: context.agent_id,
                    turn_id: context.turn_id.unwrap(),
                    usage: TokenUsage::default(),
                    num_turns: 0,
                    reason,
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

    fn input() -> AgentInput {
        AgentInput {
            message: "test".into(),
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
    fn cancel_all_cancels_active_and_queued_managed_child_turns() {
        smol::block_on(async {
            let manager = AgentManagerHandle::new(AgentLimits::default()).unwrap();
            let (current_tx, current_rx) = flume::bounded(1);
            let root = manager
                .create_root(
                    Vec::new(),
                    None,
                    Box::new(CancellableBackend {
                        current: Some(current_tx),
                        entered: None,
                    }),
                )
                .unwrap();
            let root_actor = root.actor().unwrap();
            let root_ticket = root_actor.admit_turn(input(), None, "root".into()).unwrap();
            let current = current_rx.recv_async().await.unwrap();

            let (entered_tx, entered_rx) = flume::bounded(1);
            let child = manager
                .spawn_child(
                    &current,
                    AgentMetadata::default(),
                    Vec::new(),
                    None,
                    Box::new(CancellableBackend {
                        current: None,
                        entered: Some(entered_tx),
                    }),
                )
                .unwrap();
            let child_actor = child.actor().unwrap();
            let active = child_actor
                .admit_turn(input(), None, "active".into())
                .unwrap();
            entered_rx.recv_async().await.unwrap();
            let queued = child_actor
                .admit_turn(input(), None, "queued".into())
                .unwrap();

            let (cmd_tx, cmd_rx) = flume::unbounded();
            let (init_trigger, init_cancel) = maki_agent::CancelToken::new();
            spawn_command_router(
                cmd_rx,
                Arc::new(root_actor.clone()),
                manager.clone(),
                root.id(),
                Arc::new(CancelMap::new()),
                init_trigger,
            );
            cmd_tx.send_async(AgentCommand::CancelAll).await.unwrap();
            init_cancel.cancelled().await;

            for outcome in [active.wait().await, queued.wait().await] {
                assert!(matches!(
                    outcome,
                    TurnOutcome::Cancelled {
                        reason: TurnCancellationReason::User,
                        ..
                    }
                ));
            }
            assert_eq!(child_actor.snapshot().lifecycle, ActorLifecycle::Open);
            assert!(matches!(
                root_ticket.wait().await,
                TurnOutcome::Cancelled {
                    reason: TurnCancellationReason::User,
                    ..
                }
            ));
            assert_eq!(root_actor.snapshot().lifecycle, ActorLifecycle::Open);

            drop(cmd_tx);
            let report = manager.shutdown(Duration::from_secs(1)).await;
            assert!(report.timed_out.is_empty());
        });
    }
}
