use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use event_listener::Event;
use maki_providers::TokenUsage;

use super::{AgentLimits, AgentManagerHandle, AgentMetadata, GraphLifecycle, ManagerError};
use crate::{
    ActorBackend, AgentInput, AgentMode, BackendResult, ControlWork, History, TurnContext,
    TurnOutcome, WorkKind,
};

struct Gate {
    entered: AtomicUsize,
    released: AtomicUsize,
    event: Event,
}

impl Gate {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            entered: AtomicUsize::new(0),
            released: AtomicUsize::new(0),
            event: Event::new(),
        })
    }

    fn release(&self, count: usize) {
        self.released.fetch_add(count, Ordering::Release);
        self.event.notify(usize::MAX);
    }

    async fn enter(&self) {
        let position = self.entered.fetch_add(1, Ordering::AcqRel);
        loop {
            if self.released.load(Ordering::Acquire) > position {
                return;
            }
            let listener = self.event.listen();
            if self.released.load(Ordering::Acquire) > position {
                return;
            }
            listener.await;
        }
    }
}

struct TestBackend {
    current: Option<flume::Sender<crate::CurrentManagedTurn>>,
    gate: Option<Arc<Gate>>,
}

impl TestBackend {
    fn boxed() -> Box<dyn ActorBackend> {
        Box::new(Self {
            current: None,
            gate: None,
        })
    }

    fn reporting(
        current: flume::Sender<crate::CurrentManagedTurn>,
        gate: Option<Arc<Gate>>,
    ) -> Box<dyn ActorBackend> {
        Box::new(Self {
            current: Some(current),
            gate,
        })
    }
}

struct CancellableBackend {
    entered: flume::Sender<()>,
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
            self.entered.send(()).unwrap();
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

impl ActorBackend for TestBackend {
    fn run_turn<'a>(
        &'a mut self,
        _: &'a mut History,
        context: TurnContext,
        _: AgentInput,
        _: WorkKind,
    ) -> Pin<Box<dyn Future<Output = BackendResult> + Send + 'a>> {
        Box::pin(async move {
            if let Some(sender) = &self.current {
                sender.send(context.managed_turn.clone().unwrap()).unwrap();
            }
            if let Some(gate) = &self.gate {
                gate.enter().await;
            }
            BackendResult::EnteredRun(TurnOutcome::Completed {
                agent_id: context.agent_id,
                turn_id: context.turn_id.unwrap(),
                usage: TokenUsage::default(),
                num_turns: 1,
                reason: crate::DoneReason::EndTurn,
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

fn active_root(
    limits: AgentLimits,
) -> (
    super::AgentManagerHandle,
    super::AgentRef,
    crate::CurrentManagedTurn,
    Arc<Gate>,
) {
    let manager = AgentManagerHandle::new(limits).unwrap();
    let (tx, rx) = flume::bounded(1);
    let gate = Gate::new();
    let root = manager
        .create_root(
            Vec::new(),
            None,
            TestBackend::reporting(tx, Some(Arc::clone(&gate))),
        )
        .unwrap();
    root.actor()
        .unwrap()
        .admit_turn(input(), None, "root".into())
        .unwrap();
    let current = smol::block_on(rx.recv_async()).unwrap();
    (manager, root, current, gate)
}

#[test]
fn root_and_nested_children_use_one_factory() {
    let (manager, root, current, gate) = active_root(AgentLimits::default());
    let child = manager
        .spawn_child(
            &current,
            AgentMetadata::default(),
            Vec::new(),
            None,
            TestBackend::boxed(),
        )
        .unwrap();
    let root_node = root.snapshot().unwrap();
    let child_node = child.snapshot().unwrap();
    assert_eq!(root_node.parent_id, None);
    assert_eq!(root_node.root_id, root.id());
    assert_eq!(root_node.children, vec![child.id()]);
    assert_eq!(child_node.parent_id, Some(root.id()));
    assert_eq!(child_node.root_id, root.id());
    assert_eq!(child_node.depth, 1);
    gate.release(1);
    smol::block_on(manager.shutdown(std::time::Duration::from_secs(1)));
}

#[test]
fn duplicate_root_is_rejected_without_mutating_graph() {
    let manager = AgentManagerHandle::new(AgentLimits::default()).unwrap();
    let root = manager
        .create_root(Vec::new(), None, TestBackend::boxed())
        .unwrap();

    let error = manager
        .create_root(Vec::new(), None, TestBackend::boxed())
        .unwrap_err();

    assert_eq!(error, ManagerError::DuplicateRoot);
    assert_eq!(manager.snapshot().len(), 1);
    assert_eq!(manager.root_id().unwrap(), root.id());
    smol::block_on(manager.shutdown(std::time::Duration::from_secs(1)));
}

#[test]
fn wrong_manager_capability_is_rejected() {
    let (manager, _, current, gate) = active_root(AgentLimits::default());
    let other = AgentManagerHandle::new(AgentLimits::default()).unwrap();
    let error = other
        .spawn_child(
            &current,
            AgentMetadata::default(),
            Vec::new(),
            None,
            TestBackend::boxed(),
        )
        .unwrap_err();
    assert_eq!(error, ManagerError::WrongManager);
    gate.release(1);
    smol::block_on(manager.shutdown(std::time::Duration::from_secs(1)));
}

#[test]
fn retained_context_clone_cannot_spawn_after_guard_drop() {
    let (manager, _, current, gate) = active_root(AgentLimits::default());
    gate.release(1);
    smol::block_on(async {
        loop {
            let active = manager
                .lock_graph()
                .active_turns
                .contains_key(&(current.agent_id(), current.turn_id()));
            if !active {
                break;
            }
            smol::future::yield_now().await;
        }
    });
    let error = manager
        .spawn_child(
            &current,
            AgentMetadata::default(),
            Vec::new(),
            None,
            TestBackend::boxed(),
        )
        .unwrap_err();
    assert!(matches!(error, ManagerError::InactiveTurn { .. }));
    smol::block_on(manager.shutdown(std::time::Duration::from_secs(1)));
}

#[test]
fn factory_failure_rolls_back_reservation() {
    let manager = AgentManagerHandle::new(AgentLimits::default()).unwrap();
    let error = manager
        .create_root_with(Vec::new(), None, |_| {
            Err::<Box<dyn ActorBackend>, _>("boom")
        })
        .unwrap_err();
    assert_eq!(error, ManagerError::Factory("boom".into()));
    assert!(manager.snapshot().is_empty());
    manager
        .create_root(Vec::new(), None, TestBackend::boxed())
        .unwrap();
}

#[test]
fn panicking_root_factory_rolls_back_reservation() {
    let manager = AgentManagerHandle::new(AgentLimits::default()).unwrap();
    let error = manager
        .create_root_with(
            Vec::new(),
            None,
            |_| -> Result<Box<dyn ActorBackend>, String> { panic!("factory panic") },
        )
        .unwrap_err();

    assert_eq!(
        error,
        ManagerError::Factory("agent factory panicked".into())
    );
    assert!(manager.snapshot().is_empty());
    manager
        .create_root(Vec::new(), None, TestBackend::boxed())
        .unwrap();
}

#[test]
fn panicking_child_factory_restores_capacity() {
    let limits = AgentLimits {
        max_children_per_agent: 1,
        max_live_agents: 2,
        ..AgentLimits::default()
    };
    let (manager, _root, current, gate) = active_root(limits);
    let error = manager
        .spawn_child_with(
            &current,
            AgentMetadata::default(),
            Vec::new(),
            None,
            |_| -> Result<Box<dyn ActorBackend>, String> { panic!("factory panic") },
        )
        .unwrap_err();

    assert_eq!(
        error,
        ManagerError::Factory("agent factory panicked".into())
    );
    let child = manager
        .spawn_child(
            &current,
            AgentMetadata::default(),
            Vec::new(),
            None,
            TestBackend::boxed(),
        )
        .unwrap();
    assert_eq!(manager.snapshot().len(), 2);

    child.close_subtree().unwrap();
    drop(current);
    drop(gate);
    smol::block_on(manager.shutdown(std::time::Duration::from_secs(1)));
}

#[test]
fn shutdown_does_not_join_reserved_root_before_factory_completes() {
    let manager = AgentManagerHandle::new(AgentLimits::default()).unwrap();
    let creating = manager.clone();
    let (reserved_tx, reserved_rx) = flume::bounded(1);
    let (release_tx, release_rx) = flume::bounded(1);
    let factory = std::thread::spawn(move || {
        creating.create_root_with(Vec::new(), None, |agent_id| {
            reserved_tx.send(agent_id).unwrap();
            release_rx.recv().unwrap();
            Ok::<_, String>(TestBackend::boxed())
        })
    });
    let root_id = reserved_rx.recv().unwrap();

    let report = smol::block_on(manager.shutdown(std::time::Duration::ZERO));
    assert!(report.joined.is_empty());
    assert_eq!(report.timed_out, vec![root_id]);

    release_tx.send(()).unwrap();
    assert!(matches!(
        factory.join().unwrap(),
        Err(ManagerError::GraphShutdown)
    ));
    smol::block_on(manager.wait_until_reaped());
    assert!(manager.runner_finished(root_id).unwrap());
}

#[test]
fn close_owns_runner_when_reserved_root_factory_succeeds() {
    let manager = AgentManagerHandle::new(AgentLimits::default()).unwrap();
    let creating = manager.clone();
    let (reserved_tx, reserved_rx) = flume::bounded(1);
    let (release_tx, release_rx) = flume::bounded(1);
    let factory = std::thread::spawn(move || {
        creating.create_root_with(Vec::new(), None, |agent_id| {
            reserved_tx.send(agent_id).unwrap();
            release_rx.recv().unwrap();
            Ok::<_, String>(TestBackend::boxed())
        })
    });
    let root_id = reserved_rx.recv().unwrap();

    manager.close_subtree(root_id).unwrap();
    release_tx.send(()).unwrap();
    assert!(matches!(
        factory.join().unwrap(),
        Err(ManagerError::NonLiveAgent(id)) if id == root_id
    ));
    let node = manager.node(root_id).unwrap();
    assert_eq!(node.graph_lifecycle, GraphLifecycle::Closed);
    assert_eq!(node.actor.unwrap().lifecycle, crate::ActorLifecycle::Closed);
    let report = smol::block_on(manager.shutdown(std::time::Duration::from_secs(1)));
    assert_eq!(report.joined, vec![root_id]);
    assert!(report.timed_out.is_empty());
    assert!(manager.runner_finished(root_id).unwrap());
}

#[test]
fn close_and_factory_failure_remove_never_committed_root() {
    let manager = AgentManagerHandle::new(AgentLimits::default()).unwrap();
    let creating = manager.clone();
    let (reserved_tx, reserved_rx) = flume::bounded(1);
    let (release_tx, release_rx) = flume::bounded(1);
    let factory = std::thread::spawn(move || {
        creating.create_root_with(Vec::new(), None, |agent_id| {
            reserved_tx.send(agent_id).unwrap();
            release_rx.recv().unwrap();
            Err::<Box<dyn ActorBackend>, _>("factory failed")
        })
    });
    let root_id = reserved_rx.recv().unwrap();

    manager.close_subtree(root_id).unwrap();
    release_tx.send(()).unwrap();
    assert!(matches!(
        factory.join().unwrap(),
        Err(ManagerError::Factory(_))
    ));
    assert!(matches!(manager.root_id(), Err(ManagerError::MissingRoot)));
    assert!(manager.snapshot().is_empty());
}

#[test]
fn depth_and_child_limits_are_atomic() {
    let limits = AgentLimits {
        max_agent_depth: 1,
        max_children_per_agent: 1,
        ..AgentLimits::default()
    };
    let (manager, root, current, gate) = active_root(limits);
    manager
        .spawn_child(
            &current,
            AgentMetadata::default(),
            Vec::new(),
            None,
            TestBackend::boxed(),
        )
        .unwrap();
    let error = manager
        .spawn_child(
            &current,
            AgentMetadata::default(),
            Vec::new(),
            None,
            TestBackend::boxed(),
        )
        .unwrap_err();
    assert_eq!(
        error,
        ManagerError::ChildLimit {
            parent_id: root.id(),
            max: 1
        }
    );
    assert_eq!(manager.snapshot().len(), 2);
    gate.release(1);
    smol::block_on(manager.shutdown(std::time::Duration::from_secs(1)));
}

#[test]
fn live_agent_limit_is_atomic() {
    let limits = AgentLimits {
        max_live_agents: 2,
        ..AgentLimits::default()
    };
    let (manager, _, current, gate) = active_root(limits);
    manager
        .spawn_child(
            &current,
            AgentMetadata::default(),
            Vec::new(),
            None,
            TestBackend::boxed(),
        )
        .unwrap();

    let error = manager
        .spawn_child(
            &current,
            AgentMetadata::default(),
            Vec::new(),
            None,
            TestBackend::boxed(),
        )
        .unwrap_err();

    assert_eq!(error, ManagerError::LiveAgentLimit { max: 2 });
    assert_eq!(manager.snapshot().len(), 2);
    gate.release(1);
    smol::block_on(manager.shutdown(std::time::Duration::from_secs(1)));
}

#[test]
fn close_child_preserves_parent_and_releases_capacity() {
    let limits = AgentLimits {
        max_children_per_agent: 1,
        ..AgentLimits::default()
    };
    let (manager, root, current, gate) = active_root(limits);
    let child = manager
        .spawn_child(
            &current,
            AgentMetadata::default(),
            Vec::new(),
            None,
            TestBackend::boxed(),
        )
        .unwrap();
    manager.close_subtree(child.id()).unwrap();
    let replacement = manager
        .spawn_child(
            &current,
            AgentMetadata::default(),
            Vec::new(),
            None,
            TestBackend::boxed(),
        )
        .unwrap();
    assert_ne!(child.id(), replacement.id());
    assert_eq!(
        child.snapshot().unwrap().graph_lifecycle,
        GraphLifecycle::Closed
    );
    assert_eq!(
        root.snapshot().unwrap().graph_lifecycle,
        GraphLifecycle::Live
    );
    gate.release(1);
    smol::block_on(manager.shutdown(std::time::Duration::from_secs(1)));
}

#[test]
fn cancel_agent_is_reusable_and_isolates_siblings() {
    smol::block_on(async {
        let (manager, _, current, root_gate) = active_root(AgentLimits::default());
        let (first_tx, first_rx) = flume::bounded(2);
        let first = manager
            .spawn_child(
                &current,
                AgentMetadata::default(),
                Vec::new(),
                None,
                Box::new(CancellableBackend { entered: first_tx }),
            )
            .unwrap();
        let (second_tx, second_rx) = flume::bounded(1);
        let second = manager
            .spawn_child(
                &current,
                AgentMetadata::default(),
                Vec::new(),
                None,
                Box::new(CancellableBackend { entered: second_tx }),
            )
            .unwrap();
        let first_ticket = first
            .actor()
            .unwrap()
            .admit_turn(input(), None, "first".into())
            .unwrap();
        let second_ticket = second
            .actor()
            .unwrap()
            .admit_turn(input(), None, "second".into())
            .unwrap();
        first_rx.recv_async().await.unwrap();
        second_rx.recv_async().await.unwrap();

        manager.cancel_agent(first.id()).unwrap();
        assert!(matches!(
            first_ticket.wait().await,
            TurnOutcome::Cancelled {
                reason: crate::TurnCancellationReason::User,
                ..
            }
        ));
        let mut second_wait = Box::pin(second_ticket.wait());
        assert!(
            futures_lite::future::poll_once(&mut second_wait)
                .await
                .is_none()
        );

        let reused = first
            .actor()
            .unwrap()
            .admit_turn(input(), None, "reused".into())
            .unwrap();
        first_rx.recv_async().await.unwrap();
        manager.cancel_agent(first.id()).unwrap();
        assert!(matches!(reused.wait().await, TurnOutcome::Cancelled { .. }));

        manager.cancel_agent(second.id()).unwrap();
        assert!(matches!(second_wait.await, TurnOutcome::Cancelled { .. }));
        root_gate.release(1);
        let report = manager.shutdown(std::time::Duration::from_secs(1)).await;
        assert!(report.timed_out.is_empty());
    });
}

#[test]
fn descendant_preflight_rejects_before_suspension() {
    let (manager, root, current, gate) = active_root(AgentLimits::default());
    let other = AgentManagerHandle::new(AgentLimits::default()).unwrap();
    let other_root = other
        .create_root(Vec::new(), None, TestBackend::boxed())
        .unwrap();

    assert!(matches!(
        current.validate_descendant(root.id()),
        Err(ManagerError::NotDescendant { .. })
    ));
    assert!(matches!(
        current.validate_descendant(other_root.id()),
        Err(ManagerError::UnknownAgent(_))
    ));
    let state = current.lease.inner.state.lock().unwrap();
    assert_eq!(state.suspensions, 0);
    assert!(state.watcher_cancels.is_empty());
    drop(state);

    gate.release(1);
    let report = smol::block_on(manager.shutdown(std::time::Duration::from_secs(1)));
    assert!(report.timed_out.is_empty());
    let report = smol::block_on(other.shutdown(std::time::Duration::from_secs(1)));
    assert!(report.timed_out.is_empty());
}

#[test]
fn unauthorized_prompt_wait_does_not_cancel_child_turn() {
    smol::block_on(async {
        let (manager, _, current, root_gate) = active_root(AgentLimits::default());
        let child_gate = Gate::new();
        let (child_tx, child_rx) = flume::bounded(1);
        let child = manager
            .spawn_child(
                &current,
                AgentMetadata::default(),
                Vec::new(),
                None,
                TestBackend::reporting(child_tx, Some(Arc::clone(&child_gate))),
            )
            .unwrap();
        let actor = child.actor().unwrap();
        let ticket = actor.admit_turn(input(), None, "child".into()).unwrap();
        child_rx.recv_async().await.unwrap();
        let (wrong_manager, _, wrong_current, wrong_gate) = active_root(AgentLimits::default());

        let Err(error) = wrong_current.lease().wait_for_descendant(
            &wrong_current,
            child.id(),
            &actor,
            ticket.clone(),
            None,
        ) else {
            panic!("wrong manager accepted prompt wait");
        };
        assert_eq!(error, ManagerError::UnknownAgent(child.id()));
        let mut pending = Box::pin(ticket.wait());
        assert!(
            futures_lite::future::poll_once(&mut pending)
                .await
                .is_none()
        );

        child_gate.release(1);
        assert!(matches!(pending.await, TurnOutcome::Completed { .. }));
        root_gate.release(1);
        wrong_gate.release(1);
        let report = manager.shutdown(std::time::Duration::from_secs(1)).await;
        assert!(report.timed_out.is_empty());
        let report = wrong_manager
            .shutdown(std::time::Duration::from_secs(1))
            .await;
        assert!(report.timed_out.is_empty());
    });
}

#[test]
fn parent_waiting_for_child_yields_permit() {
    smol::block_on(async {
        let limits = AgentLimits {
            max_concurrent_agent_turns: 1,
            ..AgentLimits::default()
        };
        let manager = AgentManagerHandle::new(limits).unwrap();
        let root_gate = Gate::new();
        let (root_tx, root_rx) = flume::bounded(1);
        let root = manager
            .create_root(
                Vec::new(),
                None,
                TestBackend::reporting(root_tx, Some(Arc::clone(&root_gate))),
            )
            .unwrap();
        root.actor()
            .unwrap()
            .admit_turn(input(), None, "root".into())
            .unwrap();
        let current = root_rx.recv_async().await.unwrap();
        let child_gate = Gate::new();
        let (child_tx, child_rx) = flume::bounded(1);
        let child = manager
            .spawn_child(
                &current,
                AgentMetadata::default(),
                Vec::new(),
                None,
                TestBackend::reporting(child_tx, Some(Arc::clone(&child_gate))),
            )
            .unwrap();
        let ticket = child
            .actor()
            .unwrap()
            .admit_turn(input(), None, "child".into())
            .unwrap();
        let wait = current
            .lease()
            .wait_for_descendant(&current, child.id(), &child.actor().unwrap(), ticket, None)
            .unwrap();

        child_rx.recv_async().await.unwrap();
        child_gate.release(1);
        let outcome = wait.wait().await.unwrap();
        assert_eq!(outcome.agent_id(), child.id());

        root_gate.release(1);
        let report = manager.shutdown(std::time::Duration::from_secs(1)).await;
        assert!(report.timed_out.is_empty());
    });
}

#[test]
fn watcher_registration_failure_cancels_exact_admitted_child_turn() {
    smol::block_on(async {
        let limits = AgentLimits {
            max_concurrent_agent_turns: 1,
            ..AgentLimits::default()
        };
        let (manager, root, current, root_gate) = active_root(limits);
        let child = manager
            .spawn_child(
                &current,
                AgentMetadata::default(),
                Vec::new(),
                None,
                TestBackend::boxed(),
            )
            .unwrap();
        let actor = child.actor().unwrap();
        let cancelled = actor.admit_turn(input(), None, "cancelled".into()).unwrap();
        let cancelled_id = cancelled.turn_id();
        let lease = current.lease();
        {
            let mut state = lease
                .inner
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            state.closing = true;
        }

        let Err(error) =
            lease.wait_for_descendant(&current, child.id(), &actor, cancelled.clone(), None)
        else {
            panic!("closing lease accepted a watcher");
        };
        assert!(matches!(error, ManagerError::InactiveTurn { .. }));
        assert!(matches!(
            cancelled.wait().await,
            TurnOutcome::Cancelled {
                turn_id,
                reason: crate::TurnCancellationReason::User,
                ..
            } if turn_id == cancelled_id
        ));

        {
            let mut state = lease
                .inner
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            state.closing = false;
        }
        root_gate.release(1);
        let surviving = actor.admit_turn(input(), None, "surviving".into()).unwrap();
        assert!(matches!(
            surviving.wait().await,
            TurnOutcome::Completed { .. }
        ));
        let report = manager.shutdown(std::time::Duration::from_secs(1)).await;
        assert!(report.timed_out.is_empty());
        assert_eq!(root.id(), current.agent_id());
    });
}

#[test]
fn two_child_waits_share_one_parent_suspension() {
    smol::block_on(async {
        let limits = AgentLimits {
            max_concurrent_agent_turns: 1,
            ..AgentLimits::default()
        };
        let manager = AgentManagerHandle::new(limits).unwrap();
        let root_gate = Gate::new();
        let (root_tx, root_rx) = flume::bounded(1);
        let root = manager
            .create_root(
                Vec::new(),
                None,
                TestBackend::reporting(root_tx, Some(Arc::clone(&root_gate))),
            )
            .unwrap();
        root.actor()
            .unwrap()
            .admit_turn(input(), None, "root".into())
            .unwrap();
        let current = root_rx.recv_async().await.unwrap();

        let first_gate = Gate::new();
        let (first_tx, first_rx) = flume::bounded(1);
        let first = manager
            .spawn_child(
                &current,
                AgentMetadata::default(),
                Vec::new(),
                None,
                TestBackend::reporting(first_tx, Some(Arc::clone(&first_gate))),
            )
            .unwrap();
        let first_ticket = first
            .actor()
            .unwrap()
            .admit_turn(input(), None, "first".into())
            .unwrap();
        let first_wait = current
            .lease()
            .wait_for_descendant(
                &current,
                first.id(),
                &first.actor().unwrap(),
                first_ticket,
                None,
            )
            .unwrap();

        let second_gate = Gate::new();
        let (second_tx, second_rx) = flume::bounded(1);
        let second = manager
            .spawn_child(
                &current,
                AgentMetadata::default(),
                Vec::new(),
                None,
                TestBackend::reporting(second_tx, Some(Arc::clone(&second_gate))),
            )
            .unwrap();
        let second_ticket = second
            .actor()
            .unwrap()
            .admit_turn(input(), None, "second".into())
            .unwrap();
        let second_wait = current
            .lease()
            .wait_for_descendant(
                &current,
                second.id(),
                &second.actor().unwrap(),
                second_ticket,
                None,
            )
            .unwrap();

        first_rx.recv_async().await.unwrap();
        first_gate.release(1);
        second_rx.recv_async().await.unwrap();
        let mut first_wait = Box::pin(first_wait.wait());
        assert!(
            futures_lite::future::poll_once(&mut first_wait)
                .await
                .is_none()
        );
        second_gate.release(1);
        assert_eq!(first_wait.await.unwrap().agent_id(), first.id());
        assert_eq!(second_wait.wait().await.unwrap().agent_id(), second.id());

        root_gate.release(1);
        let report = manager.shutdown(std::time::Duration::from_secs(1)).await;
        assert!(report.timed_out.is_empty());
    });
}

#[test]
fn shutdown_timeout_reaper_eventually_joins_timed_out_node() {
    smol::block_on(async {
        let manager = AgentManagerHandle::new(AgentLimits::default()).unwrap();
        let gate = Gate::new();
        let (entered_tx, entered_rx) = flume::bounded(1);
        let root = manager
            .create_root(
                Vec::new(),
                None,
                TestBackend::reporting(entered_tx, Some(Arc::clone(&gate))),
            )
            .unwrap();
        root.actor()
            .unwrap()
            .admit_turn(input(), None, "held".into())
            .unwrap();
        entered_rx.recv_async().await.unwrap();

        let report = manager.shutdown(std::time::Duration::ZERO).await;
        assert!(report.joined.is_empty());
        assert_eq!(report.timed_out, vec![root.id()]);

        gate.release(1);
        manager.wait_until_reaped().await;
        assert!(manager.runner_finished(root.id()).unwrap());
        assert_eq!(
            manager.node(root.id()).unwrap().graph_lifecycle,
            GraphLifecycle::Closed
        );
    });
}

#[test]
fn active_turns_never_exceed_manager_limit() {
    smol::block_on(async {
        let limits = AgentLimits {
            max_concurrent_agent_turns: 1,
            ..AgentLimits::default()
        };
        let manager = AgentManagerHandle::new(limits).unwrap();
        let gate = Gate::new();
        let (root_tx, root_rx) = flume::bounded(1);
        let root = manager
            .create_root(
                Vec::new(),
                None,
                TestBackend::reporting(root_tx, Some(Arc::clone(&gate))),
            )
            .unwrap();
        root.actor()
            .unwrap()
            .admit_turn(input(), None, "root".into())
            .unwrap();
        let current = root_rx.recv_async().await.unwrap();
        let (child_tx, child_rx) = flume::bounded(1);
        let child = manager
            .spawn_child(
                &current,
                AgentMetadata::default(),
                Vec::new(),
                None,
                TestBackend::reporting(child_tx, Some(Arc::clone(&gate))),
            )
            .unwrap();
        child
            .actor()
            .unwrap()
            .admit_turn(input(), None, "child".into())
            .unwrap();
        assert!(child_rx.try_recv().is_err());
        gate.release(1);
        child_rx.recv_async().await.unwrap();
        gate.release(1);
        let report = manager.shutdown(std::time::Duration::from_secs(1)).await;
        assert!(report.timed_out.is_empty());
    });
}
