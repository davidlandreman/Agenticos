use crate::lib::test_utils::Testable;
use crate::process::entity::{EntityId, LatencyContract, RunState};
use crate::process::run_queue::RunQueue;
use crate::process::scheduler::Scheduler;

fn test_entity_id_tags_pid_namespaces() {
    assert_ne!(EntityId::KernelThread(7), EntityId::UserProcess(7));
}

fn test_run_queue_fifo_and_dedup() {
    let mut queue = RunQueue::new();
    queue.reserve().unwrap();
    assert!(queue.enqueue(EntityId::UserProcess(1)).unwrap());
    assert!(!queue.enqueue(EntityId::UserProcess(1)).unwrap());
    assert!(queue.enqueue(EntityId::KernelThread(2)).unwrap());
    assert_eq!(queue.len(), 2);
    assert_eq!(queue.remove_at(0), Some(EntityId::UserProcess(1)));
    assert_eq!(queue.remove_at(0), Some(EntityId::KernelThread(2)));
    assert!(queue.is_empty());
}

fn test_scheduler_fair_queue_revolves() {
    let mut scheduler = Scheduler::new();
    scheduler.init();
    for pid in 101..=103 {
        scheduler.register_user(pid).unwrap();
        scheduler
            .make_ready(EntityId::UserProcess(pid), None)
            .unwrap();
    }
    assert_eq!(scheduler.runnable_user_count(), 3);
    for expected in [101, 102, 103, 101, 102, 103] {
        let id = scheduler.schedule_entity().unwrap();
        assert_eq!(id, EntityId::UserProcess(expected));
        assert_eq!(scheduler.current_entity(), Some(id));
        assert_eq!(scheduler.entity_state(id), Some(RunState::Running));
        scheduler.yield_entity(id);
    }
    assert_eq!(scheduler.ready_entity_count(), 3);
}

fn test_latency_contract_is_one_shot_override() {
    let mut scheduler = Scheduler::new();
    scheduler.init();
    scheduler.register_user(201).unwrap();
    scheduler.register_user(202).unwrap();
    scheduler
        .make_ready(EntityId::UserProcess(201), None)
        .unwrap();
    scheduler
        .make_ready(EntityId::UserProcess(202), Some(LatencyContract::new(0)))
        .unwrap();
    assert!(!scheduler
        .make_ready(EntityId::UserProcess(202), None)
        .unwrap());
    assert_eq!(
        scheduler.schedule_entity(),
        Some(EntityId::UserProcess(202))
    );
    scheduler.yield_entity(EntityId::UserProcess(202));
    assert_eq!(
        scheduler.schedule_entity(),
        Some(EntityId::UserProcess(201))
    );
    assert_eq!(scheduler.latency_misses_for_test(), 0);
}

fn test_timer_arm_update_and_cancel() {
    use crate::process::timer::{TimerAction, TimerKey, TimerKind, TimerQueue};

    let mut timers = TimerQueue::new();
    let entity = EntityId::UserProcess(301);
    let key = TimerKey {
        entity,
        kind: TimerKind::UserSleep,
    };
    let now = crate::arch::x86_64::interrupts::get_timer_ticks();
    assert_eq!(
        timers.arm_for_test(key, now + 10, TimerAction::UserSleep(301)),
        Ok(1)
    );
    assert_eq!(
        timers.arm_for_test(key, now + 20, TimerAction::UserSleep(301)),
        Ok(2)
    );
    assert_eq!(timers.pending_for_test(), 1);
    assert!(timers.cancel_for_test(key));
    assert_eq!(timers.pending_for_test(), 0);
}

fn test_timer_heap_orders_and_bounds_a_deferred_pass() {
    use crate::process::timer::{
        TimerAction, TimerKey, TimerKind, TimerQueue, MAX_TIMER_EXPIRATIONS_PER_PASS,
    };

    let mut timers = TimerQueue::new();
    for pid in (400..440).rev() {
        let key = TimerKey {
            entity: EntityId::UserProcess(pid),
            kind: TimerKind::UserSleep,
        };
        timers
            .arm_for_test(key, pid as u64, TimerAction::UserSleep(pid))
            .unwrap();
    }

    let mut last_deadline = 0;
    for _ in 0..MAX_TIMER_EXPIRATIONS_PER_PASS {
        let (_, deadline) = timers.pop_due_for_test(u64::MAX).unwrap();
        assert!(deadline >= last_deadline);
        last_deadline = deadline;
    }
    assert_eq!(
        timers.pending_for_test(),
        40 - MAX_TIMER_EXPIRATIONS_PER_PASS
    );
}

pub fn get_tests() -> &'static [&'static dyn Testable] {
    &[
        &test_entity_id_tags_pid_namespaces,
        &test_run_queue_fifo_and_dedup,
        &test_scheduler_fair_queue_revolves,
        &test_latency_contract_is_one_shot_override,
        &test_timer_arm_update_and_cancel,
        &test_timer_heap_orders_and_bounds_a_deferred_pass,
    ]
}
