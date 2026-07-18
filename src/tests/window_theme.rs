use crate::graphics::color::Color;
use crate::graphics::surface::{Surface, SurfaceDesc};
use crate::window::renderer::{RendererKind, RetainedRenderer, SurfaceCanvas};
use crate::window::theme::{
    self, FrameChrome, ThemeKind, ThemeRequest, AERO_BACKDROP_RADIUS, AERO_METRICS,
    CLASSIC_METRICS, FUTURISM_BACKDROP_RADIUS, FUTURISM_METRICS,
};
use crate::window::{GraphicsDevice, Insets, Rect, Window, WindowId};

fn test_theme_selection_and_fallback_matrix() {
    assert_eq!(
        theme::select_theme(ThemeRequest::Classic, RendererKind::RetainedCpu).selected,
        ThemeKind::Classic
    );
    let fallback = theme::select_theme(ThemeRequest::Aero, RendererKind::Legacy);
    assert_eq!(fallback.selected, ThemeKind::Classic);
    assert!(fallback.fallback_reason.is_some());
    assert_eq!(
        theme::select_theme(ThemeRequest::Aero, RendererKind::RetainedCpu).selected,
        ThemeKind::Aero
    );
    assert_eq!(
        theme::select_theme(ThemeRequest::Auto, RendererKind::Legacy).selected,
        ThemeKind::Classic
    );
    // Auto now defaults to Futurism on modern renderers.
    assert_eq!(
        theme::select_theme(ThemeRequest::Auto, RendererKind::RetainedCpu).selected,
        ThemeKind::Futurism
    );
    let virgl = theme::select_theme(ThemeRequest::Aero, RendererKind::Virgl);
    assert_eq!(virgl.selected, ThemeKind::Aero);
    assert!(virgl.fallback_reason.is_none());
    assert_eq!(
        theme::select_theme(ThemeRequest::Auto, RendererKind::Virgl).selected,
        ThemeKind::Futurism
    );
    // Explicit Aero stays honored on modern renderers.
    assert_eq!(
        theme::select_theme(ThemeRequest::Aero, RendererKind::RetainedCpu).selected,
        ThemeKind::Aero
    );
    // Futurism requires the retained compositor and falls back to Classic.
    let futurism_fallback = theme::select_theme(ThemeRequest::Futurism, RendererKind::Legacy);
    assert_eq!(futurism_fallback.selected, ThemeKind::Classic);
    assert!(futurism_fallback.fallback_reason.is_some());
    assert_eq!(
        theme::select_theme(ThemeRequest::Futurism, RendererKind::RetainedCpu).selected,
        ThemeKind::Futurism
    );
    assert_eq!(
        theme::select_theme(ThemeRequest::Futurism, RendererKind::Virgl).selected,
        ThemeKind::Futurism
    );
    assert_eq!(
        theme::frame_effect_for(ThemeKind::Classic),
        crate::graphics::scene::LayerEffect::None
    );
    assert_eq!(
        theme::frame_effect_for(ThemeKind::Aero),
        crate::graphics::scene::LayerEffect::BackdropSample {
            radius: AERO_BACKDROP_RADIUS
        }
    );
    assert_eq!(
        theme::frame_effect_for(ThemeKind::Futurism),
        crate::graphics::scene::LayerEffect::BackdropSample {
            radius: FUTURISM_BACKDROP_RADIUS
        }
    );
}

fn test_all_theme_effect_radii_are_gpu_supported() {
    // Regression guard: a theme declaring a backdrop radius the qualified
    // VirGL blur pipeline cannot honor makes composition fail, which is a
    // kernel panic on strict-GPU boots (the Conductor run default).
    for kind in [ThemeKind::Classic, ThemeKind::Aero, ThemeKind::Futurism] {
        let spec = theme::spec_for(kind);
        for effect in [spec.frame_effect, spec.chrome_effect] {
            if let crate::graphics::scene::LayerEffect::BackdropSample { radius } = effect {
                assert!(
                    crate::graphics::composition::gpu_backdrop_radius_supported(radius),
                    "theme {} declares unsupported backdrop radius {}",
                    spec.token,
                    radius,
                );
            }
        }
    }
}

fn test_theme_kind_codes_and_tokens_round_trip() {
    for kind in [ThemeKind::Classic, ThemeKind::Aero, ThemeKind::Futurism] {
        assert_eq!(ThemeKind::from_u8(kind as u8), Some(kind));
        assert_eq!(
            ThemeRequest::parse(kind.as_str()).and_then(ThemeRequest::explicit_kind),
            Some(kind)
        );
    }
    assert_eq!(ThemeKind::from_u8(200), None);
    assert_eq!(ThemeKind::Futurism.as_str(), "futurism");
    assert_eq!(ThemeRequest::parse("auto"), Some(ThemeRequest::Auto));
    assert_eq!(ThemeRequest::Auto.explicit_kind(), None);
    assert_eq!(ThemeRequest::parse("purple"), None);
}

fn test_metrics_and_decoration_geometry() {
    assert_eq!(CLASSIC_METRICS.title_bar_height, 20);
    assert_eq!(CLASSIC_METRICS.border_width, 4);
    assert_eq!(AERO_BACKDROP_RADIUS, 6);
    assert_eq!(AERO_METRICS.corner_radius_top, 11);
    assert_eq!(AERO_METRICS.corner_radius_bottom, 7);
    assert_eq!(AERO_METRICS.shadow_margin, 16);

    // Caption-button footprint drives close_button_rect for both themes.
    assert_eq!(CLASSIC_METRICS.button_width, 18);
    assert_eq!(CLASSIC_METRICS.button_height, 16);
    assert_eq!(CLASSIC_METRICS.button_right_margin, 2);
    assert_eq!(AERO_METRICS.button_width, 16);
    assert_eq!(AERO_METRICS.button_height, 16);
    assert_eq!(AERO_METRICS.button_right_margin, 4);

    // Must stay within the qualified VirGL blur pipeline (all three box
    // passes ≤ 2, i.e. total ≤ 6) or strict-GPU boots panic at runtime.
    assert_eq!(FUTURISM_BACKDROP_RADIUS, 6);
    assert_eq!(FUTURISM_METRICS.title_bar_height, 34);
    assert_eq!(FUTURISM_METRICS.border_width, 1);
    assert_eq!(FUTURISM_METRICS.corner_radius_top, 12);
    assert_eq!(FUTURISM_METRICS.corner_radius_bottom, 8);
    assert_eq!(FUTURISM_METRICS.shadow_margin, 22);
    assert_eq!(FUTURISM_METRICS.button_width, 30);
    assert_eq!(FUTURISM_METRICS.button_height, 22);
    assert_eq!(FUTURISM_METRICS.button_right_margin, 10);

    // Classic close button for an 80×50 window: x = right − border − margin −
    // width, y = top + border + (title − button_height)/2.
    assert_eq!(
        theme::close_button_rect(Rect::new(0, 0, 80, 50), CLASSIC_METRICS),
        Rect::new(80 - 4 - 2 - 18, 4 + 2, 18, 16)
    );
    // Futurism close button stays inside the caption strip.
    let futurism_button = theme::close_button_rect(Rect::new(0, 0, 120, 90), FUTURISM_METRICS);
    assert_eq!(futurism_button, Rect::new(120 - 1 - 10 - 30, 1 + 6, 30, 22));
    assert!(
        futurism_button.bottom()
            <= (FUTURISM_METRICS.border_width + FUTURISM_METRICS.title_bar_height) as i32
    );

    assert_eq!(
        Insets::uniform(16).expand(Rect::new(100, 50, 800, 600)),
        Rect::new(84, 34, 832, 632)
    );
}

fn test_surface_canvas_argb_is_exact_replacement() {
    let mut surface = Surface::new(SurfaceDesc::new(3, 2)).unwrap();
    let mut canvas = SurfaceCanvas::new(&mut surface, (0, 0), (3, 2));
    canvas.fill_rect_argb(0, 0, 2, 1, Color::new(200, 100, 50), 128);
    canvas.draw_pixel_argb(1, 0, Color::WHITE, 0);
    assert_eq!(surface.pixel(0, 0).unwrap().to_rgba(), (199, 100, 50, 128));
    assert_eq!(surface.pixel(1, 0).unwrap().a(), 0);
}

fn test_retained_surface_uses_decorated_bounds() {
    let mut renderer = RetainedRenderer::new(1000, 800).unwrap();
    let root = WindowId(9001);
    let decorated = Insets::uniform(16).expand(Rect::new(100, 50, 800, 600));
    renderer.ensure_surface(root, decorated).unwrap();
    assert_eq!(renderer.previous_bounds(root), Some(decorated));
    let scene = renderer.build_scene(&[(root, decorated)]);
    assert_eq!(scene.layers[0].destination_rect, decorated);
    assert_eq!(scene.layers[0].source_rect, Rect::new(0, 0, 832, 632));
}

fn test_classic_key_pixels_regression() {
    let bounds = Rect::new(0, 0, 80, 50);
    let button = theme::close_button_rect(bounds, CLASSIC_METRICS);
    let chrome = FrameChrome {
        bounds,
        title: "",
        active: true,
        close_button_rect: button,
    };
    let mut surface = Surface::new(SurfaceDesc::new(80, 50)).unwrap();
    let mut canvas = SurfaceCanvas::new(&mut surface, (0, 0), (80, 50));
    theme::draw_frame_for(ThemeKind::Classic, &chrome, &mut canvas);
    drop(canvas);

    // Raised bevel: light/highlight top-left, shadow/dark bottom-right, then
    // ButtonFace fill. These do NOT follow focus.
    assert_eq!(surface.pixel(0, 0).unwrap().to_rgba(), (223, 223, 223, 255));
    assert_eq!(surface.pixel(1, 1).unwrap().to_rgba(), (255, 255, 255, 255));
    assert_eq!(surface.pixel(2, 2).unwrap().to_rgba(), (192, 192, 192, 255));
    assert_eq!(surface.pixel(79, 49).unwrap().to_rgba(), (0, 0, 0, 255));
    assert_eq!(
        surface.pixel(78, 48).unwrap().to_rgba(),
        (128, 128, 128, 255)
    );

    // Active caption gradient: navy at the left column, ~#1084D0 at the right.
    assert_eq!(surface.pixel(4, 10).unwrap().to_rgba(), (0, 0, 128, 255));
    let (r, g, b, _) = surface.pixel(75, 10).unwrap().to_rgba();
    assert!((14..=18).contains(&r), "gradient right red {r}");
    assert!((130..=208).contains(&g), "gradient right green {g}");
    assert!((190..=208).contains(&b), "gradient right blue {b}");

    // Close button: raised ButtonFace face pixel, black ✕ glyph pixel.
    assert_eq!(
        surface
            .pixel((button.x + 2) as u32, (button.y + 2) as u32)
            .unwrap()
            .to_rgba(),
        (192, 192, 192, 255)
    );
    let glyph_x = button.x + (button.width as i32 - 8) / 2;
    let glyph_y = button.y + (button.height as i32 - 7) / 2;
    assert_eq!(
        surface
            .pixel(glyph_x as u32, glyph_y as u32)
            .unwrap()
            .to_rgba(),
        (0, 0, 0, 255)
    );

    let inactive = FrameChrome {
        active: false,
        ..chrome
    };
    let mut canvas = SurfaceCanvas::new(&mut surface, (0, 0), (80, 50));
    theme::draw_frame_for(ThemeKind::Classic, &inactive, &mut canvas);
    drop(canvas);

    // Bevel is unchanged by focus…
    assert_eq!(surface.pixel(0, 0).unwrap().to_rgba(), (223, 223, 223, 255));
    assert_eq!(surface.pixel(79, 49).unwrap().to_rgba(), (0, 0, 0, 255));
    // …only the caption gradient recolors to the inactive grey ramp.
    assert_eq!(
        surface.pixel(4, 10).unwrap().to_rgba(),
        (128, 128, 128, 255)
    );
    assert_eq!(
        surface.pixel(75, 10).unwrap().to_rgba(),
        (181, 181, 181, 255)
    );
}

fn test_aero_alpha_corners_shadow_and_client() {
    let bounds = Rect::new(16, 16, 80, 50);
    let chrome = FrameChrome {
        bounds,
        title: "M",
        active: true,
        close_button_rect: theme::close_button_rect(bounds, AERO_METRICS),
    };
    let mut surface = Surface::new(SurfaceDesc::new(112, 82)).unwrap();
    let mut canvas = SurfaceCanvas::new(&mut surface, (0, 0), (112, 82));
    theme::draw_frame_for(ThemeKind::Aero, &chrome, &mut canvas);
    drop(canvas);

    // Aero captions use clean black text without an offset halo. Pin the
    // glyph's highest-coverage pixel so this does not regress to white.
    let font = crate::graphics::fonts::core_font::get_default_font();
    let glyph = font.glyph('M').expect("caption glyph");
    let (glyph_pixel, glyph_coverage) = glyph
        .coverage
        .iter()
        .enumerate()
        .max_by_key(|(_, coverage)| **coverage)
        .map(|(index, coverage)| (index, *coverage))
        .expect("covered caption glyph pixel");
    assert!(glyph_coverage > 192);
    let glyph_col = glyph_pixel as i32 % glyph.width as i32;
    let glyph_row = glyph_pixel as i32 / glyph.width as i32;
    let title_x = bounds.x + AERO_METRICS.border_width as i32 + 8;
    let title_y = bounds.y
        + AERO_METRICS.border_width as i32
        + (AERO_METRICS.title_bar_height as i32 - font.line_height() as i32) / 2;
    let title_pixel = surface
        .pixel(
            (title_x + glyph.x_offset + glyph_col) as u32,
            (title_y + font.ascent() as i32 + glyph.y_offset + glyph_row) as u32,
        )
        .unwrap();
    let (r, g, b, a) = title_pixel.to_rgba();
    assert!(r <= 64 && g <= 64 && b <= 64, "caption pixel {r},{g},{b}");
    assert_eq!(a, 255);

    // The transparent frame cutout still contains the shadow behind the
    // rounded arc, preventing a notch where the top and side shadows meet.
    let cutout = surface.pixel(16, 16).unwrap();
    assert!(cutout.a() > 0 && cutout.a() < 96);
    assert_eq!((cutout.r(), cutout.g(), cutout.b()), (0, 0, 0));
    let chrome_alpha = surface.pixel(46, 26).unwrap().a();
    assert!((140..=200).contains(&chrome_alpha));
    let near = surface.pixel(15, 40).unwrap().a();
    let middle = surface.pixel(8, 40).unwrap().a();
    let far = surface.pixel(0, 40).unwrap().a();
    assert!(near > middle && middle > far);

    // The shadow follows the rounded top corner instead of retaining the
    // straight-edge opacity throughout the square corner area.
    let corner_near = surface.pixel(15, 15).unwrap().a();
    let corner_middle = surface.pixel(8, 8).unwrap().a();
    let top_middle = surface.pixel(40, 8).unwrap().a();
    assert!(corner_near > 0 && corner_near < near);
    assert!(corner_middle < top_middle);

    let client_y = bounds.y as u32 + AERO_METRICS.border_width + AERO_METRICS.title_bar_height + 3;
    assert_eq!(surface.pixel(46, client_y).unwrap().a(), 0);
}

fn test_futurism_translucent_chrome_shadow_and_close_button() {
    let bounds = Rect::new(24, 24, 120, 90);
    let chrome = FrameChrome {
        bounds,
        title: "M",
        active: true,
        close_button_rect: theme::close_button_rect(bounds, FUTURISM_METRICS),
    };
    let mut surface = Surface::new(SurfaceDesc::new(170, 140)).unwrap();
    let mut canvas = SurfaceCanvas::new(&mut surface, (0, 0), (170, 140));
    theme::draw_frame_for(ThemeKind::Futurism, &chrome, &mut canvas);
    drop(canvas);

    // Title bar: translucent dark indigo (blue channel dominates red).
    let title = surface.pixel(80, 30).unwrap();
    assert!((160..255).contains(&title.a()), "title alpha {}", title.a());
    assert!(title.b() > title.r(), "title {},{}", title.r(), title.b());

    // Client well stays transparent — the content child paints over it.
    assert_eq!(surface.pixel(80, 70).unwrap().a(), 0);

    // Drop shadow in the gutter is black and translucent.
    let shadow = surface.pixel(19, 70).unwrap();
    assert!(
        shadow.a() > 0 && shadow.a() < 96,
        "shadow alpha {}",
        shadow.a()
    );
    assert_eq!((shadow.r(), shadow.g(), shadow.b()), (0, 0, 0));

    // The rounded top-left corner is clipped: the square-corner pixel holds
    // only shadow, not opaque chrome.
    let cutout = surface.pixel(24, 24).unwrap();
    assert!(cutout.a() < 96, "corner alpha {}", cutout.a());

    // Close button: soft red rounded button with a white × glyph.
    let button = chrome.close_button_rect;
    let face = surface
        .pixel((button.x + 10) as u32, (button.y + 5) as u32)
        .unwrap();
    assert!(
        face.r() > 170 && face.r() > face.g(),
        "close {:?}",
        face.to_rgba()
    );

    // The title bar meets the content well directly — no transparent gap:
    // the last title row is still title-bar glass.
    let border = FUTURISM_METRICS.border_width as i32;
    let last_title_y = (bounds.y + border + FUTURISM_METRICS.title_bar_height as i32 - 1) as u32;
    assert!(
        surface.pixel(80, last_title_y).unwrap().a() > 160,
        "gap under title bar"
    );

    // Content runs flush to the edge: simulate the client filling the well
    // (inset only by the hairline), then run the overlay pass — the rounded
    // bottom corner is carved back out of the client's pixels while the
    // straight bottom edge keeps them.
    let mut canvas = SurfaceCanvas::new(&mut surface, (0, 0), (170, 140));
    canvas.fill_rect(
        bounds.x + border,
        bounds.y + border + FUTURISM_METRICS.title_bar_height as i32,
        bounds.width - 2 * border as u32,
        bounds
            .height
            .saturating_sub(FUTURISM_METRICS.title_bar_height + 2 * border as u32),
        Color::WHITE,
    );
    drop(canvas);
    assert_eq!(surface.pixel(25, 112).unwrap().a(), 255);
    let previous = theme::active();
    theme::activate(ThemeKind::Futurism);
    let mut canvas = SurfaceCanvas::new(&mut surface, (0, 0), (170, 140));
    theme::draw_frame_overlay(&chrome, &mut canvas);
    drop(canvas);
    theme::activate(previous);
    let carved = surface.pixel(25, 112).unwrap();
    assert!(carved.a() < 200, "corner not carved: alpha {}", carved.a());
    assert_eq!(surface.pixel(80, 112).unwrap().a(), 255);
}

fn test_frame_runtime_theme_change_preserves_client_size_and_effect() {
    let previous = theme::active();
    theme::activate(ThemeKind::Classic);
    let mut frame = crate::window::windows::FrameWindow::new(WindowId(9010), "Settings");
    frame.set_bounds(Rect::new(40, 30, 648, 508));
    assert_eq!(frame.content_area().width, 640);
    assert_eq!(frame.content_area().height, 480);

    frame.apply_theme(CLASSIC_METRICS, AERO_METRICS, ThemeKind::Aero);
    theme::activate(ThemeKind::Aero);
    assert_eq!(frame.content_area().width, 640);
    assert_eq!(frame.content_area().height, 480);
    assert_eq!(frame.bounds(), Rect::new(40, 30, 650, 518));
    assert_eq!(
        frame.compositor_properties().effect,
        crate::graphics::scene::LayerEffect::BackdropSample {
            radius: AERO_BACKDROP_RADIUS
        }
    );

    frame.apply_theme(AERO_METRICS, FUTURISM_METRICS, ThemeKind::Futurism);
    theme::activate(ThemeKind::Futurism);
    assert_eq!(frame.content_area().width, 640);
    assert_eq!(frame.content_area().height, 480);
    assert_eq!(frame.bounds(), Rect::new(40, 30, 642, 516));
    assert_eq!(
        frame.compositor_properties().effect,
        crate::graphics::scene::LayerEffect::BackdropSample {
            radius: FUTURISM_BACKDROP_RADIUS
        }
    );
    theme::activate(previous);
}

pub fn get_tests() -> &'static [&'static dyn crate::lib::test_utils::Testable] {
    &[
        &test_theme_selection_and_fallback_matrix,
        &test_all_theme_effect_radii_are_gpu_supported,
        &test_theme_kind_codes_and_tokens_round_trip,
        &test_metrics_and_decoration_geometry,
        &test_surface_canvas_argb_is_exact_replacement,
        &test_retained_surface_uses_decorated_bounds,
        &test_classic_key_pixels_regression,
        &test_aero_alpha_corners_shadow_and_client,
        &test_futurism_translucent_chrome_shadow_and_close_button,
        &test_frame_runtime_theme_change_preserves_client_size_and_effect,
    ]
}
