use crate::graphics::color::Color;
use crate::graphics::surface::{Surface, SurfaceDesc};
use crate::window::renderer::{RendererKind, RetainedRenderer, SurfaceCanvas};
use crate::window::theme::{
    self, FrameChrome, ThemeKind, ThemeRequest, AERO_METRICS, CLASSIC_METRICS,
};
use crate::window::{GraphicsDevice, Insets, Rect, WindowId};

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
    assert_eq!(
        theme::select_theme(ThemeRequest::Auto, RendererKind::RetainedCpu).selected,
        ThemeKind::Aero
    );
    let virgl = theme::select_theme(ThemeRequest::Aero, RendererKind::Virgl);
    assert_eq!(virgl.selected, ThemeKind::Aero);
    assert!(virgl.fallback_reason.is_none());
    assert_eq!(
        theme::select_theme(ThemeRequest::Auto, RendererKind::Virgl).selected,
        ThemeKind::Classic
    );
}

fn test_metrics_and_decoration_geometry() {
    assert_eq!(CLASSIC_METRICS.title_bar_height, 20);
    assert_eq!(CLASSIC_METRICS.border_width, 4);
    assert_eq!(AERO_METRICS.shadow_margin, 16);

    // Caption-button footprint drives close_button_rect for both themes.
    assert_eq!(CLASSIC_METRICS.button_width, 18);
    assert_eq!(CLASSIC_METRICS.button_height, 16);
    assert_eq!(CLASSIC_METRICS.button_right_margin, 2);
    assert_eq!(AERO_METRICS.button_width, 16);
    assert_eq!(AERO_METRICS.button_height, 16);
    assert_eq!(AERO_METRICS.button_right_margin, 4);

    // Classic close button for an 80×50 window: x = right − border − margin −
    // width, y = top + border + (title − button_height)/2.
    assert_eq!(
        theme::close_button_rect(Rect::new(0, 0, 80, 50), CLASSIC_METRICS),
        Rect::new(80 - 4 - 2 - 18, 4 + 2, 18, 16)
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
        title: "",
        active: true,
        close_button_rect: theme::close_button_rect(bounds, AERO_METRICS),
    };
    let mut surface = Surface::new(SurfaceDesc::new(112, 82)).unwrap();
    let mut canvas = SurfaceCanvas::new(&mut surface, (0, 0), (112, 82));
    theme::draw_frame_for(ThemeKind::Aero, &chrome, &mut canvas);

    assert_eq!(surface.pixel(16, 16).unwrap().a(), 0);
    let chrome_alpha = surface.pixel(46, 26).unwrap().a();
    assert!((140..=200).contains(&chrome_alpha));
    let near = surface.pixel(15, 40).unwrap().a();
    let middle = surface.pixel(8, 40).unwrap().a();
    let far = surface.pixel(0, 40).unwrap().a();
    assert!(near > middle && middle > far);
    let client_y = bounds.y as u32 + AERO_METRICS.border_width + AERO_METRICS.title_bar_height + 3;
    assert_eq!(surface.pixel(46, client_y).unwrap().a(), 0);
}

pub fn get_tests() -> &'static [&'static dyn crate::lib::test_utils::Testable] {
    &[
        &test_theme_selection_and_fallback_matrix,
        &test_metrics_and_decoration_geometry,
        &test_surface_canvas_argb_is_exact_replacement,
        &test_retained_surface_uses_decorated_bounds,
        &test_classic_key_pixels_regression,
        &test_aero_alpha_corners_shadow_and_client,
    ]
}
