use crate::graphics::scene::{Layer, LayerEffect, SceneFrame, Transform2D};
use crate::graphics::surface::SurfaceId;
use crate::window::Rect;

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

pub fn get_tests() -> &'static [&'static dyn crate::lib::test_utils::Testable] {
    &[
        &test_stable_scene_order,
        &test_translation_and_effect_damage_bounds,
        &test_opaque_defaults,
        &test_backdrop_damage_reaches_adjacent_glass,
    ]
}
