use crate::lib::test_utils::Testable;

pub fn get_tests() -> &'static [&'static dyn Testable] {
    &[
        &test_madt_parser,
        &test_percpu_and_trampoline_layout,
        &test_all_discovered_cpus_online_and_ticking,
        &test_cross_cpu_kernel_dispatch,
        &test_cross_cpu_sleep_handoff_stress,
        &test_cross_cpu_kernel_termination_retires_stack,
        &test_cross_cpu_ring3_setup_keeps_cr3_owned,
        &test_cross_cpu_ring3_run_and_exit,
    ]
}

fn test_madt_parser() {
    use alloc::vec;

    let length = 44 + 8 + 8 + 12 + 10;
    let mut table = vec![0u8; length];
    table[0..4].copy_from_slice(b"APIC");
    table[4..8].copy_from_slice(&(length as u32).to_le_bytes());
    table[8] = 1;
    table[36..40].copy_from_slice(&0xfee0_0000u32.to_le_bytes());
    table[40..44].copy_from_slice(&1u32.to_le_bytes());

    let mut offset = 44;
    for (acpi_id, lapic_id) in [(7u8, 3u8), (8, 4)] {
        table[offset] = 0;
        table[offset + 1] = 8;
        table[offset + 2] = acpi_id;
        table[offset + 3] = lapic_id;
        table[offset + 4..offset + 8].copy_from_slice(&1u32.to_le_bytes());
        offset += 8;
    }
    table[offset] = 1;
    table[offset + 1] = 12;
    table[offset + 2] = 2;
    table[offset + 4..offset + 8].copy_from_slice(&0xfec0_0000u32.to_le_bytes());
    offset += 12;
    table[offset] = 2;
    table[offset + 1] = 10;
    table[offset + 3] = 0;
    table[offset + 4..offset + 8].copy_from_slice(&2u32.to_le_bytes());
    table[offset + 8..offset + 10].copy_from_slice(&0u16.to_le_bytes());

    let sum = table.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte));
    table[9] = 0u8.wrapping_sub(sum);

    let topology =
        crate::arch::x86_64::acpi::parse_madt(&table, 4).expect("valid MADT fixture must parse");
    assert_eq!(topology.cpu_count, 2);
    assert_eq!(topology.cpus[0].lapic_id, 4, "BSP must be slot zero");
    assert_eq!(topology.ioapic.unwrap().address, 0xfec0_0000);
    assert_eq!(topology.override_for_irq(0).unwrap().gsi, 2);
}

fn test_percpu_and_trampoline_layout() {
    assert_eq!(
        crate::arch::x86_64::percpu::abi_offsets_for_test(),
        (0, 8, 16)
    );
    let (size, cr3, stack, entry, cpu) = crate::arch::x86_64::smp::trampoline_layout_for_test();
    assert!(size <= 4096);
    for patch in [cr3, stack, entry, cpu] {
        assert!(patch < size, "trampoline patch site must be inside blob");
    }
}

fn test_all_discovered_cpus_online_and_ticking() {
    let topology = crate::arch::x86_64::acpi::topology();
    assert_eq!(
        crate::arch::x86_64::smp::online_cpu_count(),
        topology.cpu_count,
        "every discovered CPU must check in"
    );
    let before: alloc::vec::Vec<_> = (0..topology.cpu_count)
        .map(|cpu| {
            crate::arch::x86_64::percpu::cpu_time_snapshot(cpu)
                .expect("online CPU must expose time counters")
        })
        .collect();
    let deadline = crate::arch::x86_64::interrupts::get_timer_ticks() + 5;
    while crate::arch::x86_64::interrupts::get_timer_ticks() < deadline {
        x86_64::instructions::hlt();
    }
    for (cpu, before) in before.iter().enumerate() {
        let after = crate::arch::x86_64::percpu::cpu_time_snapshot(cpu)
            .expect("online CPU counters vanished");
        let before_total = before
            .user
            .saturating_add(before.system)
            .saturating_add(before.idle);
        let after_total = after
            .user
            .saturating_add(after.system)
            .saturating_add(after.idle);
        assert!(
            after_total > before_total,
            "CPU {cpu} accounting did not advance"
        );
    }
    for cpu in 1..topology.cpu_count {
        assert!(
            crate::arch::x86_64::interrupts::lapic_timer_ticks(cpu) > 0,
            "CPU {cpu} LAPIC timer did not tick"
        );
    }
}

fn test_cross_cpu_kernel_dispatch() {
    use alloc::string::String;
    use core::sync::atomic::{AtomicUsize, Ordering};

    static DONE: AtomicUsize = AtomicUsize::new(0);
    let cpu_count = crate::arch::x86_64::smp::online_cpu_count();
    if cpu_count == 1 {
        return;
    }
    crate::arch::x86_64::smp::set_test_ap_dispatch_enabled(true);
    DONE.store(0, Ordering::Release);
    let mut spawned = alloc::vec::Vec::new();
    for index in 0..cpu_count * 2 {
        let pid = crate::process::spawn_process(String::from("smp-burn"), None, move || {
            let deadline = crate::arch::x86_64::interrupts::get_timer_ticks() + 8;
            while crate::arch::x86_64::interrupts::get_timer_ticks() < deadline {
                core::hint::spin_loop();
            }
            let _ = index;
            DONE.fetch_add(1, Ordering::AcqRel);
        });
        spawned.push(pid);
    }
    let timeout = crate::arch::x86_64::interrupts::get_timer_ticks() + 200;
    while DONE.load(Ordering::Acquire) < cpu_count * 2
        && crate::arch::x86_64::interrupts::get_timer_ticks() < timeout
    {
        x86_64::instructions::hlt();
    }
    crate::arch::x86_64::smp::set_test_ap_dispatch_enabled(false);
    let completed = DONE.load(Ordering::Acquire);
    if completed != cpu_count * 2 {
        let scheduler = crate::process::scheduler::SCHEDULER.lock();
        crate::debug_error!(
            "SMP dispatch timeout: completed={} ready={} tick={}",
            completed,
            scheduler.ready_entity_count(),
            crate::arch::x86_64::interrupts::get_timer_ticks()
        );
        for cpu in 0..cpu_count {
            crate::debug_error!(
                "  CPU {} current={:?} dispatches={}",
                cpu,
                scheduler.current_entity_on_cpu(cpu),
                crate::arch::x86_64::percpu::dispatches(cpu)
            );
        }
        for pid in spawned {
            let context = scheduler.get_context(pid).copied();
            crate::debug_error!(
                "  PID {} state={:?} context={:?}",
                pid,
                scheduler.entity_state(crate::process::entity::EntityId::KernelThread(pid)),
                context.map(|ctx| (ctx.rip, ctx.rsp))
            );
        }
    }
    assert_eq!(completed, cpu_count * 2);
    let active_aps = (1..cpu_count)
        .filter(|cpu| crate::arch::x86_64::percpu::dispatches(*cpu) > 0)
        .count();
    assert!(active_aps > 0, "no AP dispatched a kernel thread");
}

fn test_cross_cpu_sleep_handoff_stress() {
    use alloc::string::String;
    use core::sync::atomic::{AtomicUsize, Ordering};

    static DONE: AtomicUsize = AtomicUsize::new(0);
    static CPU_MASK: AtomicUsize = AtomicUsize::new(0);

    let cpu_count = crate::arch::x86_64::smp::online_cpu_count();
    if cpu_count == 1 {
        return;
    }

    const SLEEPS_PER_THREAD: usize = 32;
    let thread_count = cpu_count * 2;
    DONE.store(0, Ordering::Release);
    CPU_MASK.store(0, Ordering::Release);
    crate::arch::x86_64::smp::set_test_ap_dispatch_enabled(true);

    for _ in 0..thread_count {
        crate::process::spawn_process(String::from("smp-sleep-handoff"), None, || {
            for _ in 0..SLEEPS_PER_THREAD {
                let cpu = crate::arch::x86_64::percpu::cpu_id();
                CPU_MASK.fetch_or(1usize << cpu, Ordering::AcqRel);
                crate::process::sleep_ticks_with_contract(
                    1,
                    Some(crate::process::entity::LatencyContract::new(2)),
                );
            }
            DONE.fetch_add(1, Ordering::AcqRel);
        });
    }

    let timeout = crate::arch::x86_64::interrupts::get_timer_ticks().saturating_add(1_000);
    while DONE.load(Ordering::Acquire) != thread_count
        && crate::arch::x86_64::interrupts::get_timer_ticks() < timeout
    {
        x86_64::instructions::hlt();
    }
    crate::arch::x86_64::smp::set_test_ap_dispatch_enabled(false);

    assert_eq!(
        DONE.load(Ordering::Acquire),
        thread_count,
        "cross-CPU sleep/wake handoff timed out",
    );
    assert!(
        CPU_MASK.load(Ordering::Acquire) & !1 != 0,
        "sleep handoff stress never executed on an AP",
    );
}

fn test_cross_cpu_kernel_termination_retires_stack() {
    use alloc::string::String;
    use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    static STARTED: AtomicBool = AtomicBool::new(false);
    static RUN_CPU: AtomicUsize = AtomicUsize::new(usize::MAX);

    if crate::arch::x86_64::smp::online_cpu_count() == 1 {
        return;
    }

    STARTED.store(false, Ordering::Release);
    RUN_CPU.store(usize::MAX, Ordering::Release);
    crate::arch::x86_64::smp::set_test_ap_dispatch_enabled(true);
    let pid = crate::process::spawn_process(String::from("smp-remote-kill"), None, || {
        RUN_CPU.store(crate::arch::x86_64::percpu::cpu_id(), Ordering::Release);
        STARTED.store(true, Ordering::Release);
        loop {
            core::hint::spin_loop();
        }
    });

    let start_timeout = crate::arch::x86_64::interrupts::get_timer_ticks().saturating_add(100);
    while !STARTED.load(Ordering::Acquire)
        && crate::arch::x86_64::interrupts::get_timer_ticks() < start_timeout
    {
        x86_64::instructions::hlt();
    }
    assert!(STARTED.load(Ordering::Acquire));
    assert_ne!(RUN_CPU.load(Ordering::Acquire), 0);

    crate::process::terminate_process(pid);
    let exit_timeout = crate::arch::x86_64::interrupts::get_timer_ticks().saturating_add(100);
    loop {
        let removed = crate::process::scheduler::SCHEDULER
            .lock()
            .entity_state(crate::process::entity::EntityId::KernelThread(pid))
            .is_none();
        if removed || crate::arch::x86_64::interrupts::get_timer_ticks() >= exit_timeout {
            assert!(removed, "remote kernel-thread termination timed out");
            break;
        }
        x86_64::instructions::hlt();
    }
    crate::arch::x86_64::smp::set_test_ap_dispatch_enabled(false);
}

fn test_cross_cpu_ring3_setup_keeps_cr3_owned() {
    use alloc::string::String;
    use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use x86_64::registers::control::Cr3;

    static COMPLETED: AtomicUsize = AtomicUsize::new(0);
    static BURNERS_DONE: AtomicUsize = AtomicUsize::new(0);
    static LAST_CPU: AtomicUsize = AtomicUsize::new(usize::MAX);
    static START: AtomicBool = AtomicBool::new(false);

    let cpu_count = crate::arch::x86_64::smp::online_cpu_count();
    if cpu_count == 1 || !crate::fs::exists("/host/TASKMGR.ELF") {
        return;
    }

    const LAUNCHES: usize = 1;
    let burner_count = cpu_count * 2;
    COMPLETED.store(0, Ordering::Release);
    BURNERS_DONE.store(0, Ordering::Release);
    LAST_CPU.store(usize::MAX, Ordering::Release);
    START.store(false, Ordering::Release);
    crate::userland::launcher::set_test_cr3_setup_delay(12);
    crate::arch::x86_64::smp::set_test_ap_dispatch_enabled(true);

    let mut kernel_pids = alloc::vec::Vec::new();
    for _ in 0..LAUNCHES {
        kernel_pids.push(crate::process::spawn_process(
            String::from("smp-ring3-setup"),
            None,
            || {
                while !START.load(Ordering::Acquire) {
                    core::hint::spin_loop();
                }
                let cpu = crate::arch::x86_64::percpu::cpu_id();
                let pid = crate::userland::launcher::prepare_user_binary_unstarted(
                    "/host/TASKMGR.ELF",
                    &["taskmgr"],
                    &crate::userland::process_service::DEFAULT_USER_ENV,
                    None,
                )
                .expect("cross-CPU Task Manager setup");

                let (active, _) = Cr3::read();
                assert_eq!(
                    Some(active),
                    crate::mm::paging::kernel_l4_frame(),
                    "kernel thread returned from user setup with a user CR3"
                );
                drop(crate::userland::lifecycle::remove_process(pid));
                LAST_CPU.store(cpu, Ordering::Release);
                COMPLETED.fetch_add(1, Ordering::AcqRel);
            },
        ));
    }
    for _ in 0..burner_count {
        crate::process::spawn_process(String::from("smp-cr3-burn"), None, || {
            while !START.load(Ordering::Acquire) {
                core::hint::spin_loop();
            }
            let deadline = crate::arch::x86_64::interrupts::get_timer_ticks().saturating_add(25);
            while crate::arch::x86_64::interrupts::get_timer_ticks() < deadline {
                core::hint::spin_loop();
            }
            BURNERS_DONE.fetch_add(1, Ordering::AcqRel);
        });
    }
    START.store(true, Ordering::Release);

    let timeout = crate::arch::x86_64::interrupts::get_timer_ticks().saturating_add(500);
    while (COMPLETED.load(Ordering::Acquire) != LAUNCHES
        || BURNERS_DONE.load(Ordering::Acquire) != burner_count)
        && crate::arch::x86_64::interrupts::get_timer_ticks() < timeout
    {
        x86_64::instructions::hlt();
    }

    crate::userland::launcher::set_test_cr3_setup_delay(0);
    crate::arch::x86_64::smp::set_test_ap_dispatch_enabled(false);
    if COMPLETED.load(Ordering::Acquire) != LAUNCHES {
        let scheduler = crate::process::scheduler::SCHEDULER.lock();
        crate::debug_error!(
            "ring3 setup timeout: completed={} progress={:?} ready={}",
            COMPLETED.load(Ordering::Acquire),
            crate::userland::launcher::test_setup_progress(),
            scheduler.ready_entity_count(),
        );
        for cpu in 0..cpu_count {
            crate::debug_error!(
                "  CPU {cpu}: current={:?}",
                scheduler.current_entity_on_cpu(cpu)
            );
        }
        for pid in kernel_pids {
            crate::debug_error!(
                "  kernel pid {pid}: state={:?}",
                scheduler.entity_diagnostics_for_test(
                    crate::process::entity::EntityId::KernelThread(pid)
                )
            );
        }
    }
    assert_eq!(COMPLETED.load(Ordering::Acquire), LAUNCHES);
    assert_ne!(
        LAST_CPU.load(Ordering::Acquire),
        0,
        "ring-3 setup did not execute on an AP"
    );
}

fn test_cross_cpu_ring3_run_and_exit() {
    use alloc::string::String;
    use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use x86_64::registers::control::Cr3;

    static START: AtomicBool = AtomicBool::new(false);
    static DONE: AtomicBool = AtomicBool::new(false);
    static BURNERS_DONE: AtomicUsize = AtomicUsize::new(0);
    static RUN_CPU: AtomicUsize = AtomicUsize::new(usize::MAX);

    let cpu_count = crate::arch::x86_64::smp::online_cpu_count();
    if cpu_count == 1 || !crate::fs::exists("/host/HELLOCPP.ELF") {
        return;
    }

    START.store(false, Ordering::Release);
    DONE.store(false, Ordering::Release);
    BURNERS_DONE.store(0, Ordering::Release);
    RUN_CPU.store(usize::MAX, Ordering::Release);
    crate::arch::x86_64::smp::set_test_ap_dispatch_enabled(true);

    let launcher_pid = crate::process::spawn_process(String::from("smp-ring3-run"), None, || {
        while !START.load(Ordering::Acquire) {
            core::hint::spin_loop();
        }
        RUN_CPU.store(crate::arch::x86_64::percpu::cpu_id(), Ordering::Release);
        let result = crate::userland::launcher::launch_user_binary(
            "/host/HELLOCPP.ELF",
            &["/host/HELLOCPP.ELF", "--noecho"],
            &[],
        );
        assert_eq!(
            result,
            Ok((crate::userland::lifecycle::ExitKind::Cooperative, 0))
        );
        let (active, _) = Cr3::read();
        assert_eq!(Some(active), crate::mm::paging::kernel_l4_frame());
        DONE.store(true, Ordering::Release);
    });

    // Keep alternatives runnable so the launcher's kernel and user portions
    // are both eligible to migrate/preempt during the test.
    for _ in 0..cpu_count * 2 {
        crate::process::spawn_process(String::from("smp-ring3-run-burn"), None, || {
            while !START.load(Ordering::Acquire) {
                core::hint::spin_loop();
            }
            let deadline = crate::arch::x86_64::interrupts::get_timer_ticks().saturating_add(30);
            while crate::arch::x86_64::interrupts::get_timer_ticks() < deadline {
                core::hint::spin_loop();
            }
            BURNERS_DONE.fetch_add(1, Ordering::AcqRel);
        });
    }
    START.store(true, Ordering::Release);

    let timeout = crate::arch::x86_64::interrupts::get_timer_ticks().saturating_add(1_000);
    while (!DONE.load(Ordering::Acquire) || BURNERS_DONE.load(Ordering::Acquire) != cpu_count * 2)
        && crate::arch::x86_64::interrupts::get_timer_ticks() < timeout
    {
        x86_64::instructions::hlt();
    }
    crate::arch::x86_64::smp::set_test_ap_dispatch_enabled(false);
    if !DONE.load(Ordering::Acquire) {
        let scheduler = crate::process::scheduler::SCHEDULER.lock();
        crate::debug_error!(
            "ring3 run timeout: launcher={} state={:?} ready={} requests={:?} wake_slots={}",
            launcher_pid,
            scheduler.entity_diagnostics_for_test(crate::process::entity::EntityId::KernelThread(
                launcher_pid
            )),
            scheduler.ready_entity_count(),
            crate::drivers::virtio::block::request_diagnostics(),
            crate::process::pending_kernel_io_wakes_for_test(),
        );
        for cpu in 0..cpu_count {
            crate::debug_error!(
                "  CPU {cpu}: current={:?}",
                scheduler.current_entity_on_cpu(cpu)
            );
        }
        let table = crate::userland::lifecycle::PROCESS_TABLE.lock();
        for (pid, process) in &table.by_pid {
            if *pid != crate::userland::lifecycle::KERNEL_PID {
                crate::debug_error!(
                    "  user pid {pid}: exit={:?} blocked={:?}",
                    process.exit_kind,
                    table.ring3_blocked.get(pid),
                );
            }
        }
    }
    assert!(
        DONE.load(Ordering::Acquire),
        "cross-CPU ring-3 run timed out"
    );
    assert_ne!(RUN_CPU.load(Ordering::Acquire), 0);
}
