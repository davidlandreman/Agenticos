use alloc::collections::BTreeMap;

use crate::graphics::composition::{CompositionEngine, CpuCompositionEngine};
use crate::graphics::scene::{Layer, SceneFrame};
use crate::graphics::surface::{PremulArgb, Surface, SurfaceDesc, SurfaceId};
use crate::window::Rect;

fn solid_surface(width: u32, height: u32, pixel: PremulArgb) -> Surface {
    let mut surface = Surface::new(SurfaceDesc::new(width, height)).unwrap();
    surface.clear(Rect::new(0, 0, width, height), pixel);
    surface
}

fn test_half_red_over_blue_oracle() {
    let blue_id = SurfaceId(1);
    let red_id = SurfaceId(2);
    let mut surfaces = BTreeMap::new();
    surfaces.insert(
        blue_id,
        solid_surface(2, 2, PremulArgb::from_rgba(0, 0, 255, 255)),
    );
    surfaces.insert(
        red_id,
        solid_surface(1, 1, PremulArgb::from_rgba(255, 0, 0, 128)),
    );

    let mut scene = SceneFrame::new(2, 2);
    scene.push(Layer::opaque(blue_id, Rect::new(0, 0, 2, 2)));
    scene.push(Layer::opaque(red_id, Rect::new(0, 0, 1, 1)));
    let mut engine = CpuCompositionEngine::new(2, 2).unwrap();
    let stats = engine
        .compose(&scene, &surfaces, &[Rect::new(0, 0, 2, 2)])
        .unwrap();
    let (r, g, b, a) = engine.output().pixel(0, 0).unwrap().to_rgba();
    assert_eq!(a, 255);
    assert!((r as i16 - 128).abs() <= 1);
    assert_eq!(g, 0);
    assert!((b as i16 - 127).abs() <= 1);
    assert_eq!(stats.output_pixels_damaged, 4);
    assert_eq!(
        engine.output().pixel(1, 1).unwrap().to_rgba(),
        (0, 0, 255, 255)
    );
}

fn test_damage_preserves_untouched_output() {
    let id = SurfaceId(1);
    let mut surfaces = BTreeMap::new();
    surfaces.insert(
        id,
        solid_surface(2, 1, PremulArgb::from_rgba(10, 20, 30, 255)),
    );
    let mut scene = SceneFrame::new(2, 1);
    scene.push(Layer::opaque(id, Rect::new(0, 0, 2, 1)));
    let mut engine = CpuCompositionEngine::new(2, 1).unwrap();
    engine
        .compose(&scene, &surfaces, &[Rect::new(0, 0, 2, 1)])
        .unwrap();
    surfaces
        .get_mut(&id)
        .unwrap()
        .set_pixel(0, 0, PremulArgb::from_rgba(200, 0, 0, 255));
    engine
        .compose(&scene, &surfaces, &[Rect::new(0, 0, 1, 1)])
        .unwrap();
    assert_eq!(
        engine.output().pixel(0, 0).unwrap().to_rgba(),
        (200, 0, 0, 255)
    );
    assert_eq!(
        engine.output().pixel(1, 0).unwrap().to_rgba(),
        (10, 20, 30, 255)
    );
}

fn test_layer_opacity() {
    let id = SurfaceId(1);
    let mut surfaces = BTreeMap::new();
    surfaces.insert(
        id,
        solid_surface(1, 1, PremulArgb::from_rgba(255, 255, 255, 255)),
    );
    let mut layer = Layer::opaque(id, Rect::new(0, 0, 1, 1));
    layer.opacity = 128;
    let mut scene = SceneFrame::new(1, 1);
    scene.push(layer);
    let mut engine = CpuCompositionEngine::new(1, 1).unwrap();
    engine
        .compose(&scene, &surfaces, &[Rect::new(0, 0, 1, 1)])
        .unwrap();
    assert_eq!(engine.output().pixel(0, 0).unwrap().a(), 128);
}

fn test_backdrop_blur_uniform_is_identity() {
    let backdrop_id = SurfaceId(1);
    let glass_id = SurfaceId(2);
    let mut surfaces = BTreeMap::new();
    surfaces.insert(
        backdrop_id,
        solid_surface(7, 3, PremulArgb::from_rgba(30, 90, 150, 255)),
    );
    surfaces.insert(
        glass_id,
        solid_surface(7, 3, PremulArgb::from_rgba(220, 235, 250, 160)),
    );
    let mut scene = SceneFrame::new(7, 3);
    scene.push(Layer::opaque(backdrop_id, Rect::new(0, 0, 7, 3)));
    let mut glass = Layer::opaque(glass_id, Rect::new(0, 0, 7, 3));
    glass.effect = crate::graphics::scene::LayerEffect::BackdropSample { radius: 3 };
    scene.push(glass);
    let mut blurred = CpuCompositionEngine::new(7, 3).unwrap();
    blurred
        .compose(&scene, &surfaces, &[Rect::new(0, 0, 7, 3)])
        .unwrap();
    glass.effect = crate::graphics::scene::LayerEffect::None;
    scene.layers[1] = glass;
    let mut plain = CpuCompositionEngine::new(7, 3).unwrap();
    plain
        .compose(&scene, &surfaces, &[Rect::new(0, 0, 7, 3)])
        .unwrap();
    assert_eq!(blurred.output().pixels(), plain.output().pixels());
}

fn test_backdrop_blur_spreads_impulse() {
    let backdrop_id = SurfaceId(1);
    let glass_id = SurfaceId(2);
    let mut backdrop = solid_surface(9, 1, PremulArgb::from_rgba(0, 0, 0, 255));
    backdrop.set_pixel(4, 0, PremulArgb::from_rgba(255, 255, 255, 255));
    let mut surfaces = BTreeMap::new();
    surfaces.insert(backdrop_id, backdrop);
    surfaces.insert(
        glass_id,
        solid_surface(9, 1, PremulArgb::from_rgba(0, 0, 0, 1)),
    );
    let mut scene = SceneFrame::new(9, 1);
    scene.push(Layer::opaque(backdrop_id, Rect::new(0, 0, 9, 1)));
    let mut glass = Layer::opaque(glass_id, Rect::new(0, 0, 9, 1));
    glass.effect = crate::graphics::scene::LayerEffect::BackdropSample { radius: 3 };
    scene.push(glass);
    let mut engine = CpuCompositionEngine::new(9, 1).unwrap();
    engine
        .compose(&scene, &surfaces, &[Rect::new(0, 0, 9, 1)])
        .unwrap();
    let center = engine.output().pixel(4, 0).unwrap().to_rgba().0;
    let neighbor = engine.output().pixel(3, 0).unwrap().to_rgba().0;
    assert!(center < 255);
    assert!(neighbor > 0);
}

fn test_backdrop_damage_uses_current_neighborhood() {
    let backdrop_id = SurfaceId(1);
    let glass_id = SurfaceId(2);
    let mut surfaces = BTreeMap::new();
    surfaces.insert(
        backdrop_id,
        solid_surface(7, 1, PremulArgb::from_rgba(0, 0, 255, 255)),
    );
    surfaces.insert(
        glass_id,
        solid_surface(1, 1, PremulArgb::from_rgba(255, 255, 255, 128)),
    );
    let mut scene = SceneFrame::new(7, 1);
    scene.push(Layer::opaque(backdrop_id, Rect::new(0, 0, 7, 1)));
    let mut glass = Layer::opaque(glass_id, Rect::new(3, 0, 1, 1));
    glass.effect = crate::graphics::scene::LayerEffect::BackdropSample { radius: 2 };
    scene.push(glass);
    let mut engine = CpuCompositionEngine::new(7, 1).unwrap();
    engine
        .compose(&scene, &surfaces, &[Rect::new(0, 0, 7, 1)])
        .unwrap();
    let before = engine.output().pixel(3, 0).unwrap();
    surfaces
        .get_mut(&backdrop_id)
        .unwrap()
        .set_pixel(1, 0, PremulArgb::from_rgba(255, 0, 0, 255));
    engine
        .compose(&scene, &surfaces, &[Rect::new(1, 0, 3, 1)])
        .unwrap();
    assert_ne!(engine.output().pixel(3, 0).unwrap(), before);
}

pub fn get_tests() -> &'static [&'static dyn crate::lib::test_utils::Testable] {
    &[
        &test_half_red_over_blue_oracle,
        &test_damage_preserves_untouched_output,
        &test_layer_opacity,
        &test_backdrop_blur_uniform_is_identity,
        &test_backdrop_blur_spreads_impulse,
        &test_backdrop_damage_uses_current_neighborhood,
    ]
}
