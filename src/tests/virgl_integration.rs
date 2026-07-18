//! Hardware-backed VirGL qualification. This module is run only by
//! `scripts/test-virgl-integration.sh`; ordinary all-tests boots do not attach
//! a GL device and therefore treat it as an inert registration check.

use alloc::collections::BTreeMap;

use crate::drivers::{fw_cfg, virtio::gpu::VirtioGpu};
use crate::graphics::composition::{
    CompositionEngine, CpuCompositionEngine, VirglCompositionEngine,
};
use crate::graphics::scene::{Layer, SceneFrame};
use crate::graphics::surface::{PremulArgb, Surface, SurfaceDesc, SurfaceId};
use crate::window::cursor::CursorRenderer;
use crate::window::Rect;

const GATE_PATH: &str = "opt/agenticos/virgl_test";
const SCANOUT_GATE_PATH: &str = "opt/agenticos/virgl_scanout_test";

fn gate_enabled(path: &str) -> bool {
    let mut gate = [0u8; 4];
    fw_cfg::read_file(path, &mut gate)
        .and_then(|len| core::str::from_utf8(&gate[..len]).ok())
        .map(|value| value.trim() == "1")
        .unwrap_or(false)
}

fn test_clear_and_readback() {
    let enabled = gate_enabled(GATE_PATH);
    if !enabled {
        // The full suite intentionally remains host-independent. Selecting
        // this module without the dedicated runner is an error, however.
        if crate::tests::filter::filter_str().is_some() {
            panic!("VirGL integration test requires scripts/test-virgl-integration.sh");
        }
        return;
    }

    let mut gpu = VirtioGpu::discover().expect("qualified VirtIO-GPU device not discovered");
    let fence = gpu
        .virgl_clear_readback_smoke()
        .expect("VirGL clear/readback qualification failed");
    crate::debug_info!("VirGL clear/readback qualification passed fence={}", fence);
    let alpha_fence = gpu
        .virgl_alpha_readback_smoke()
        .expect("VirGL alpha/readback qualification failed");
    crate::debug_info!(
        "VirGL premultiplied-alpha qualification passed fence={}",
        alpha_fence
    );
    gpu.virgl_lifecycle_smoke(100)
        .expect("VirGL 100-cycle lifecycle qualification failed");
    crate::debug_info!("VirGL 100-cycle lifecycle qualification passed");
    gpu.reset();

    let mut surfaces = BTreeMap::new();
    let background_id = SurfaceId(1);
    let foreground_id = SurfaceId(2);
    let mut background = Surface::new(SurfaceDesc::new(8, 8)).unwrap();
    for y in 0..8 {
        for x in 0..8 {
            background.set_pixel(
                x,
                y,
                PremulArgb::from_rgba((x * 23) as u8, (y * 29) as u8, 160, 255),
            );
        }
    }
    let mut foreground = Surface::new(SurfaceDesc::new(3, 2)).unwrap();
    let colors = [
        PremulArgb::from_rgba(255, 0, 0, 128),
        PremulArgb::from_rgba(0, 255, 0, 96),
        PremulArgb::from_rgba(0, 0, 255, 192),
        PremulArgb::from_rgba(255, 255, 0, 64),
        PremulArgb::from_rgba(255, 0, 255, 160),
        PremulArgb::from_rgba(0, 255, 255, 224),
    ];
    for y in 0..2 {
        for x in 0..3 {
            foreground.set_pixel(x, y, colors[(y * 3 + x) as usize]);
        }
    }
    surfaces.insert(background_id, background);
    surfaces.insert(foreground_id, foreground);

    let mut scene = SceneFrame::new(8, 8);
    scene.push(Layer::opaque(background_id, Rect::new(0, 0, 8, 8)));
    let mut foreground_layer = Layer::opaque(foreground_id, Rect::new(2, 3, 3, 2));
    foreground_layer.opacity = 211;
    scene.push(foreground_layer);
    let damage = [Rect::new(0, 0, 8, 8)];
    let mut cpu = CpuCompositionEngine::new(8, 8).unwrap();
    cpu.compose(&scene, &surfaces, &damage).unwrap();
    let mut virgl = VirglCompositionEngine::new(8, 8).unwrap();
    let production_stats = virgl.compose(&scene, &surfaces, &damage).unwrap();
    assert_eq!(production_stats.gpu_readback_bytes, 0);
    assert_eq!(production_stats.gpu_readback_cycles, 0);
    let readback_bytes = virgl
        .readback_output()
        .expect("explicit VirGL diagnostic readback failed");
    assert_eq!(readback_bytes, 8 * 8 * 4);
    assert!(virgl.uses_direct_scanout());
    assert_eq!(
        virgl
            .present_direct(&damage)
            .expect("production VirGL direct present failed"),
        1
    );
    assert!(virgl.hardware_cursor_needs_image());
    let cursor = CursorRenderer::hardware_argb_64();
    assert!(virgl
        .update_hardware_cursor(4, 4, Some(&cursor))
        .expect("VirGL hardware cursor definition failed"));
    assert!(!virgl.hardware_cursor_needs_image());
    assert!(virgl
        .update_hardware_cursor(5, 5, None)
        .expect("VirGL hardware cursor move failed"));
    for (index, (expected, actual)) in cpu
        .output()
        .pixels()
        .iter()
        .zip(virgl.output().pixels())
        .enumerate()
    {
        for (cpu_channel, gpu_channel) in [
            (expected.a(), actual.a()),
            (expected.r(), actual.r()),
            (expected.g(), actual.g()),
            (expected.b(), actual.b()),
        ] {
            assert!(
                cpu_channel.abs_diff(gpu_channel) <= 1,
                "production VirGL mismatch pixel={} cpu={:?} gpu={:?}",
                index,
                expected,
                actual
            );
        }
    }
    crate::debug_info!("VirGL production scene matches CPU reference within one channel value");

    if gate_enabled(SCANOUT_GATE_PATH) {
        drop(virgl);
        let mut gpu = VirtioGpu::discover().expect("scanout VirtIO-GPU rediscovery failed");
        let mut fixture = gpu
            .virgl_scanout_smoke(1280, 720)
            .expect("direct VirGL scanout qualification failed");
        crate::debug_info!(
            "VIRGL_SCANOUT_READY scanout={} color=ff00ff size=1280x720",
            fixture.scanout_id
        );
        let deadline = crate::arch::x86_64::interrupts::get_timer_ticks().saturating_add(500);
        while crate::arch::x86_64::interrupts::get_timer_ticks() < deadline {
            core::hint::spin_loop();
        }
        gpu.destroy_virgl_scanout_fixture(&mut fixture)
            .expect("direct VirGL scanout teardown failed");
        gpu.reset();
        crate::debug_info!("VirGL direct scanout qualification passed");
    }
}

pub fn get_tests() -> &'static [&'static dyn crate::lib::test_utils::Testable] {
    &[&test_clear_and_readback]
}
