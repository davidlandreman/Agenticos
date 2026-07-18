use crate::graphics::surface::{
    PremulArgb, Surface, SurfaceBudget, SurfaceClass, SurfaceDesc, SurfaceError,
};
use crate::window::Rect;

fn test_premultiply_round_trip() {
    let pixel = PremulArgb::from_rgba(200, 100, 50, 128);
    let (r, g, b, a) = pixel.to_rgba();
    assert_eq!(a, 128);
    assert!((r as i16 - 200).abs() <= 1);
    assert!((g as i16 - 100).abs() <= 1);
    assert!((b as i16 - 50).abs() <= 1);
}

fn test_alpha_extremes() {
    assert_eq!(
        PremulArgb::from_rgba(255, 12, 3, 0),
        PremulArgb::TRANSPARENT
    );
    assert_eq!(
        PremulArgb::from_rgba(255, 12, 3, 255).to_rgba(),
        (255, 12, 3, 255)
    );
}

fn test_half_alpha_source_over() {
    let red = PremulArgb::from_rgba(255, 0, 0, 128);
    let blue = PremulArgb::from_rgba(0, 0, 255, 255);
    let (r, g, b, a) = red.source_over(blue).to_rgba();
    assert_eq!(a, 255);
    assert!((r as i16 - 128).abs() <= 1);
    assert_eq!(g, 0);
    assert!((b as i16 - 127).abs() <= 1);
}

fn test_surface_checked_sizes_and_damage_merge() {
    assert_eq!(SurfaceDesc::new(0, 2).byte_len(), Err(SurfaceError::Empty));
    assert_eq!(
        SurfaceDesc::new(u32::MAX, u32::MAX).byte_len(),
        Err(SurfaceError::SizeOverflow)
    );
    let mut surface = Surface::new(SurfaceDesc::new(8, 8)).unwrap();
    surface.clear_damage();
    surface.mark_damage(Rect::new(0, 0, 2, 2));
    surface.mark_damage(Rect::new(2, 0, 2, 2));
    assert_eq!(surface.damage(), &[Rect::new(0, 0, 4, 2)]);
}

fn test_surface_resize_and_rows() {
    let mut surface = Surface::new(SurfaceDesc::new(3, 2)).unwrap();
    assert_eq!(surface.row(0).unwrap().len(), 3);
    assert!(surface.resize(SurfaceDesc::new(4, 5)).unwrap());
    assert_eq!(surface.pixels().len(), 20);
    assert!(!surface.resize(SurfaceDesc::new(4, 5)).unwrap());
}

fn test_budget_rejection_and_accounting() {
    let mut budget = SurfaceBudget::new(100);
    budget.reserve(SurfaceClass::Visible, 60).unwrap();
    assert_eq!(
        budget.reserve(SurfaceClass::Output, 41),
        Err(SurfaceError::BudgetExceeded)
    );
    budget.reserve(SurfaceClass::Output, 40).unwrap();
    assert_eq!(budget.total(), 100);
    assert_eq!(budget.peak_bytes(), 100);
    budget.release(SurfaceClass::Visible, 20);
    assert_eq!(budget.visible_bytes(), 40);
}

pub fn get_tests() -> &'static [&'static dyn crate::lib::test_utils::Testable] {
    &[
        &test_premultiply_round_trip,
        &test_alpha_extremes,
        &test_half_alpha_source_over,
        &test_surface_checked_sizes_and_damage_merge,
        &test_surface_resize_and_rows,
        &test_budget_rejection_and_accounting,
    ]
}
