use crate::graphics::scene::{Layer, LayerEffect, SceneFrame, Transform2D};
use crate::graphics::surface::{PremulArgb, SurfaceId};
use crate::window::renderer::RetainedRenderer;
use crate::window::{Rect, WindowId};

fn test_stable_scene_order() {
    let mut scene = SceneFrame::new(100, 100);
    let mut top = Layer::opaque(SurfaceId(2), Rect::new(10, 10, 20, 20));
    top.z_index = 10;
    let mut first = Layer::opaque(SurfaceId(1), Rect::new(0, 0, 100, 100));
    first.z_index = 0;
    scene.push(top);
    scene.push(first);
    scene.sort_by_z();
    assert_eq!(scene.layers[0].canonical_surface_id(), Some(SurfaceId(1)));
    assert_eq!(scene.layers[1].canonical_surface_id(), Some(SurfaceId(2)));
}

fn test_translation_and_effect_damage_bounds() {
    let mut layer = Layer::opaque(SurfaceId(1), Rect::new(10, 20, 30, 40));
    layer.transform = Transform2D::translation(5, -3);
    assert_eq!(layer.output_bounds(), Rect::new(15, 17, 30, 40));
    layer.effect = LayerEffect::BackdropSample { radius: 4 };
    assert_eq!(layer.output_bounds(), Rect::new(15, 17, 30, 40));
}

fn test_opaque_defaults() {
    let layer = Layer::opaque(SurfaceId(7), Rect::new(1, 2, 3, 4));
    assert_eq!(layer.opacity, 255);
    assert_eq!(layer.transform, Transform2D::IDENTITY);
    assert_eq!(layer.effect, LayerEffect::None);
    assert!(layer.visible);
}

fn test_backdrop_damage_reaches_adjacent_glass() {
    let mut scene = SceneFrame::new(40, 20);
    let mut glass = Layer::opaque(SurfaceId(1), Rect::new(10, 5, 10, 10));
    glass.effect = LayerEffect::BackdropSample { radius: 4 };
    scene.push(glass);
    let damage =
        crate::window::WindowManager::test_expand_backdrop_damage(&scene, &[Rect::new(6, 8, 1, 1)]);
    assert!(damage
        .iter()
        .any(|rect| rect.contains_point(crate::window::Point::new(10, 8))));
}

fn test_shared_backdrop_radius_contract() {
    assert_eq!(crate::graphics::scene::backdrop_box_radii(0), [0, 0, 0]);
    assert_eq!(crate::graphics::scene::backdrop_box_radii(4), [1, 1, 2]);

    let mut layers = [
        Layer::opaque(SurfaceId(1), Rect::new(0, 0, 2, 2)),
        Layer::opaque(SurfaceId(2), Rect::new(0, 0, 2, 2)),
    ];
    layers[0].effect = LayerEffect::BackdropSample { radius: 4 };
    layers[1].effect = LayerEffect::BackdropSample { radius: 2 };
    assert_eq!(crate::graphics::scene::backdrop_halo(&layers), 6);
    layers[1].visible = false;
    assert_eq!(crate::graphics::scene::backdrop_halo(&layers), 4);
}

fn test_moved_glass_reuses_coverage_and_crops_blur_work() {
    let mut renderer = RetainedRenderer::new(160, 120).unwrap();
    let background_root = WindowId::new();
    let glass_root = WindowId::new();
    let background_bounds = Rect::new(0, 0, 160, 120);
    let glass_bounds = Rect::new(8, 8, 128, 96);
    let (background_id, _) = renderer
        .ensure_surface(background_root, background_bounds)
        .unwrap();
    renderer.surface_mut(background_id).unwrap().clear(
        Rect::new(0, 0, 160, 120),
        PremulArgb::from_rgba(20, 80, 140, 255),
    );
    let (glass_id, _) = renderer.ensure_surface(glass_root, glass_bounds).unwrap();
    let glass = renderer.surface_mut(glass_id).unwrap();
    let tint = PremulArgb::from_rgba(190, 220, 245, 160);
    glass.clear(Rect::new(0, 0, 128, 8), tint);
    glass.clear(Rect::new(0, 88, 128, 8), tint);
    glass.clear(Rect::new(0, 8, 8, 80), tint);
    glass.clear(Rect::new(120, 8, 8, 80), tint);
    glass.clear(
        Rect::new(8, 8, 112, 80),
        PremulArgb::from_rgba(32, 32, 32, 255),
    );

    let mut scene = renderer.build_scene(&[
        (background_root, background_bounds),
        (glass_root, glass_bounds),
    ]);
    scene.layers[1].effect = LayerEffect::BackdropSample { radius: 4 };
    renderer.prepare_backdrop_coverage(&mut scene);
    let first = renderer
        .compose(&scene, &[Rect::new(0, 0, 160, 120)])
        .unwrap();
    assert_eq!(first.backdrop_coverage_scans, 1);
    assert_eq!(first.backdrop_coverage_regions, 4);
    let full_window_blur_work = 128 * 96 * 6;
    assert!(first.backdrop_blur_pixels < full_window_blur_work);

    let moved = Rect::new(9, 8, 128, 96);
    renderer.ensure_surface(glass_root, moved).unwrap();
    let mut moved_scene =
        renderer.build_scene(&[(background_root, background_bounds), (glass_root, moved)]);
    moved_scene.layers[1].effect = LayerEffect::BackdropSample { radius: 4 };
    renderer.prepare_backdrop_coverage(&mut moved_scene);
    let moved_stats = renderer
        .compose(&moved_scene, &[glass_bounds.union(&moved)])
        .unwrap();
    assert_eq!(moved_stats.backdrop_coverage_scans, 0);
    assert_eq!(moved_stats.backdrop_coverage_pixels_scanned, 0);
    assert!(moved_stats.backdrop_blur_pixels < full_window_blur_work);

    renderer.surface_mut(glass_id).unwrap().clear(
        Rect::new(16, 16, 1, 1),
        PremulArgb::from_rgba(190, 220, 245, 160),
    );
    let mut no_effect_scene =
        renderer.build_scene(&[(background_root, background_bounds), (glass_root, moved)]);
    renderer.prepare_backdrop_coverage(&mut no_effect_scene);
    renderer
        .compose(&no_effect_scene, &[Rect::new(25, 24, 1, 1)])
        .unwrap();

    let mut restored_effect_scene =
        renderer.build_scene(&[(background_root, background_bounds), (glass_root, moved)]);
    restored_effect_scene.layers[1].effect = LayerEffect::BackdropSample { radius: 4 };
    renderer.prepare_backdrop_coverage(&mut restored_effect_scene);
    let restored_stats = renderer
        .compose(&restored_effect_scene, &[Rect::new(25, 24, 1, 1)])
        .unwrap();
    assert_eq!(restored_stats.backdrop_coverage_scans, 1);
}

pub fn get_tests() -> &'static [&'static dyn crate::lib::test_utils::Testable] {
    &[
        &test_stable_scene_order,
        &test_translation_and_effect_damage_bounds,
        &test_opaque_defaults,
        &test_backdrop_damage_reaches_adjacent_glass,
        &test_shared_backdrop_radius_contract,
        &test_moved_glass_reuses_coverage_and_crops_blur_work,
    ]
}
