use crate::window::renderer::{select_renderer, CompositorRequest, RendererKind, SelectionError};

fn test_parse_requests() {
    assert_eq!(
        CompositorRequest::parse("legacy"),
        Some(CompositorRequest::Legacy)
    );
    assert_eq!(
        CompositorRequest::parse("retained\n"),
        Some(CompositorRequest::Retained)
    );
    assert_eq!(
        CompositorRequest::parse("gpu"),
        Some(CompositorRequest::Gpu)
    );
    assert_eq!(
        CompositorRequest::parse("auto"),
        Some(CompositorRequest::Auto)
    );
    assert_eq!(CompositorRequest::parse("metal"), None);
}

fn test_default_legacy_is_direct() {
    let selected = select_renderer(CompositorRequest::Legacy, false, true, true).unwrap();
    assert_eq!(selected.selected, RendererKind::Legacy);
    assert_eq!(selected.fallback_reason, None);
}

fn test_auto_fallback_chain() {
    let retained = select_renderer(CompositorRequest::Auto, false, true, false).unwrap();
    assert_eq!(retained.selected, RendererKind::RetainedCpu);
    assert!(retained.fallback_reason.is_some());
    let legacy = select_renderer(CompositorRequest::Auto, false, false, false).unwrap();
    assert_eq!(legacy.selected, RendererKind::Legacy);
}

fn test_gpu_strict_failure() {
    assert_eq!(
        select_renderer(CompositorRequest::Gpu, true, true, false),
        Err(SelectionError::GpuUnavailable),
    );
}

fn test_gpu_only_after_smoke_gate() {
    let selected = select_renderer(CompositorRequest::Gpu, true, true, true).unwrap();
    assert_eq!(selected.selected, RendererKind::Virgl);
}

pub fn get_tests() -> &'static [&'static dyn crate::lib::test_utils::Testable] {
    &[
        &test_parse_requests,
        &test_default_legacy_is_direct,
        &test_auto_fallback_chain,
        &test_gpu_strict_failure,
        &test_gpu_only_after_smoke_gate,
    ]
}
