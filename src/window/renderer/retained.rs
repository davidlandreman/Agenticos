use alloc::boxed::Box;
use alloc::collections::{BTreeMap, BTreeSet};

use crate::drivers::virtio::gpu::{ScanoutResource, VirtioGpu};
use crate::graphics::composition::{
    ClientGlFrame, ClientGlId, ClientGlInfo, CompositionEngine, CompositionEngineKind,
    CpuCompositionEngine, RenderStats, VirglCompositionEngine,
};
use crate::graphics::scene::{BackdropCoverage, Layer, LayerEffect, LayerSource, SceneFrame};
use crate::graphics::surface::{
    fractional_alpha_coverage, Surface, SurfaceBudget, SurfaceClass, SurfaceDesc, SurfaceError,
    SurfaceId,
};
use crate::window::{Rect, WindowId};

const DEFAULT_SURFACE_BUDGET: usize = 48 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetainedRendererError {
    Surface(SurfaceError),
    Composition,
}

impl From<SurfaceError> for RetainedRendererError {
    fn from(value: SurfaceError) -> Self {
        Self::Surface(value)
    }
}

pub struct RetainedRenderer {
    surfaces: BTreeMap<SurfaceId, Surface>,
    root_surfaces: BTreeMap<WindowId, SurfaceId>,
    bounds: BTreeMap<WindowId, Rect>,
    backdrop_coverage: BTreeMap<SurfaceId, BackdropCoverage>,
    engine: Box<dyn CompositionEngine>,
    budget: SurfaceBudget,
    last_stats: RenderStats,
    virtio_presenter: Option<(VirtioGpu, ScanoutResource)>,
    pending_coverage_stats: (u64, u64, u64),
}

impl RetainedRenderer {
    pub fn new(width: u32, height: u32) -> Result<Self, RetainedRendererError> {
        let engine = Box::new(
            CpuCompositionEngine::new(width, height)
                .map_err(|_| RetainedRendererError::Composition)?,
        );
        Self::with_engine(width, height, engine, true)
    }

    pub fn new_gpu(width: u32, height: u32) -> Result<Self, RetainedRendererError> {
        let engine = Box::new(
            VirglCompositionEngine::new(width, height)
                .map_err(|_| RetainedRendererError::Composition)?,
        );
        Self::with_engine(width, height, engine, false)
    }

    fn with_engine(
        width: u32,
        height: u32,
        engine: Box<dyn CompositionEngine>,
        allow_virtio_presenter: bool,
    ) -> Result<Self, RetainedRendererError> {
        let output_bytes = SurfaceDesc::new(width, height).byte_len()?;
        let mut budget = SurfaceBudget::new(DEFAULT_SURFACE_BUDGET);
        budget.reserve(SurfaceClass::Output, output_bytes)?;
        let virtio_presenter = allow_virtio_presenter.then(|| VirtioGpu::discover().ok()).flatten().and_then(|mut gpu| {
            match gpu.create_scanout(width, height) {
                Ok(resource) => {
                    crate::debug_info!("retained presenter candidate=virtio-gpu-2d scanout={} size={}x{}", resource.scanout_id, width, height);
                    Some((gpu, resource))
                }
                Err(error) => {
                    crate::debug_warn!("virtio-gpu 2D presenter initialization failed: {:?}; fallback=boot-framebuffer", error);
                    None
                }
            }
        });
        Ok(Self {
            surfaces: BTreeMap::new(),
            root_surfaces: BTreeMap::new(),
            bounds: BTreeMap::new(),
            backdrop_coverage: BTreeMap::new(),
            engine,
            budget,
            last_stats: RenderStats::default(),
            virtio_presenter,
            pending_coverage_stats: (0, 0, 0),
        })
    }

    pub fn engine_kind(&self) -> CompositionEngineKind {
        self.engine.kind()
    }

    pub fn create_gl_client(
        &mut self,
        width: u32,
        height: u32,
    ) -> Result<ClientGlId, RetainedRendererError> {
        self.engine
            .create_gl_client(width, height)
            .map_err(|_| RetainedRendererError::Composition)
    }

    pub fn submit_gl_client_frame(
        &mut self,
        id: ClientGlId,
        frame: ClientGlFrame,
    ) -> Result<(), RetainedRendererError> {
        self.engine
            .submit_gl_client_frame(id, frame)
            .map_err(|_| RetainedRendererError::Composition)
    }

    pub fn gl_client_info(&self, id: ClientGlId) -> Option<ClientGlInfo> {
        self.engine.gl_client_info(id)
    }

    pub fn destroy_gl_client(&mut self, id: ClientGlId) -> Result<(), RetainedRendererError> {
        self.engine
            .destroy_gl_client(id)
            .map_err(|_| RetainedRendererError::Composition)
    }

    pub fn ensure_surface(
        &mut self,
        root: WindowId,
        bounds: Rect,
    ) -> Result<(SurfaceId, bool), RetainedRendererError> {
        let id = SurfaceId(root.0 as u64);
        let desc = SurfaceDesc::new(bounds.width, bounds.height);
        let bytes = desc.byte_len()?;

        if let Some(surface) = self.surfaces.get(&id) {
            if surface.desc() == desc {
                self.root_surfaces.insert(root, id);
                self.bounds.insert(root, bounds);
                return Ok((id, false));
            }
        }

        let old_bytes = self.surfaces.get(&id).map(Surface::byte_len).unwrap_or(0);
        if bytes > old_bytes {
            self.budget
                .reserve(SurfaceClass::Visible, bytes - old_bytes)?;
        }
        let surface = match Surface::new(desc) {
            Ok(surface) => surface,
            Err(error) => {
                if bytes > old_bytes {
                    self.budget
                        .release(SurfaceClass::Visible, bytes - old_bytes);
                }
                return Err(error.into());
            }
        };
        if old_bytes > bytes {
            self.budget
                .release(SurfaceClass::Visible, old_bytes - bytes);
        }
        self.surfaces.insert(id, surface);
        self.root_surfaces.insert(root, id);
        self.bounds.insert(root, bounds);
        Ok((id, true))
    }

    pub fn surface_mut(&mut self, id: SurfaceId) -> Option<&mut Surface> {
        self.surfaces.get_mut(&id)
    }
    pub fn previous_bounds(&self, root: WindowId) -> Option<Rect> {
        self.bounds.get(&root).copied()
    }

    pub fn retain_roots(&mut self, roots: &[WindowId]) {
        let keep: BTreeSet<WindowId> = roots.iter().copied().collect();
        let stale: alloc::vec::Vec<WindowId> = self
            .root_surfaces
            .keys()
            .copied()
            .filter(|id| !keep.contains(id))
            .collect();
        for root in stale {
            if let Some(id) = self.root_surfaces.remove(&root) {
                if let Some(surface) = self.surfaces.remove(&id) {
                    self.budget
                        .release(SurfaceClass::Visible, surface.byte_len());
                }
                self.backdrop_coverage.remove(&id);
            }
            self.bounds.remove(&root);
        }
    }

    pub fn build_scene(&self, ordered_roots: &[(WindowId, Rect)]) -> SceneFrame {
        let output = self.engine.output().desc();
        let mut scene = SceneFrame::new(output.width, output.height);
        for (z, (root, bounds)) in ordered_roots.iter().enumerate() {
            let Some(&surface_id) = self.root_surfaces.get(root) else {
                continue;
            };
            let mut layer = Layer::opaque(surface_id, *bounds);
            layer.clip_rect = Rect::new(0, 0, output.width, output.height);
            layer.z_index = z as i32;
            scene.push(layer);
        }
        scene
    }

    /// Populate per-surface backdrop coverage after the manager has attached
    /// effect metadata to the scene. Clean moved surfaces reuse their cache.
    pub fn prepare_backdrop_coverage(&mut self, scene: &mut SceneFrame) {
        // Invalidate before selecting effect layers: a surface may be damaged
        // while its backdrop effect is temporarily disabled, and composition
        // will acknowledge that damage before the effect is enabled again.
        for (surface_id, surface) in &self.surfaces {
            if !surface.damage().is_empty() {
                self.backdrop_coverage.remove(surface_id);
            }
        }
        let mut scans = 0u64;
        let mut pixels_scanned = 0u64;
        let mut region_count = 0u64;
        let layers = scene.layers.clone();
        for layer in &layers {
            if !layer.visible
                || layer.opacity == 0
                || !matches!(layer.effect, LayerEffect::BackdropSample { .. })
            {
                continue;
            }
            let LayerSource::Canonical(surface_id) = layer.source else {
                continue;
            };
            let Some(surface) = self.surfaces.get(&surface_id) else {
                continue;
            };
            if layer.opacity != u8::MAX {
                scene.set_backdrop_coverage(surface_id, BackdropCoverage::Full);
                region_count = region_count.saturating_add(1);
                continue;
            }
            let coverage = self.backdrop_coverage.entry(surface_id).or_insert_with(|| {
                scans = scans.saturating_add(1);
                pixels_scanned =
                    pixels_scanned.saturating_add(surface.width() as u64 * surface.height() as u64);
                fractional_alpha_coverage(surface)
            });
            region_count = region_count.saturating_add(match coverage {
                BackdropCoverage::Empty => 0,
                BackdropCoverage::Full => 1,
                BackdropCoverage::Regions(regions) => regions.len() as u64,
            });
            scene.set_backdrop_coverage(surface_id, coverage.clone());
        }
        self.pending_coverage_stats = (scans, pixels_scanned, region_count);
    }

    pub fn compose(
        &mut self,
        scene: &SceneFrame,
        damage: &[Rect],
    ) -> Result<RenderStats, RetainedRendererError> {
        let mut snapshots = BTreeMap::<SurfaceId, alloc::vec::Vec<Rect>>::new();
        for layer in &scene.layers {
            let Some(surface_id) = layer.canonical_surface_id() else {
                continue;
            };
            if !layer.visible || layer.opacity == 0 || snapshots.contains_key(&surface_id) {
                continue;
            }
            if let Some(surface) = self.surfaces.get(&surface_id) {
                snapshots.insert(surface_id, surface.damage_snapshot());
            }
        }
        let mut stats = self
            .engine
            .compose(scene, &self.surfaces, damage)
            .map_err(|_| RetainedRendererError::Composition)?;
        let (scans, pixels_scanned, regions) = self.pending_coverage_stats;
        self.pending_coverage_stats = (0, 0, 0);
        stats.backdrop_coverage_scans = scans;
        stats.backdrop_coverage_pixels_scanned = pixels_scanned;
        stats.backdrop_coverage_regions = regions;
        for (surface_id, snapshot) in snapshots {
            if let Some(surface) = self.surfaces.get_mut(&surface_id) {
                let _ = surface.acknowledge_damage(&snapshot);
            }
        }
        self.last_stats = stats;
        Ok(stats)
    }

    pub fn output(&self) -> &Surface {
        self.engine.output()
    }
    pub fn output_mut(&mut self) -> &mut Surface {
        self.engine.output_mut()
    }

    pub fn uses_direct_scanout(&self) -> bool {
        self.engine.uses_direct_scanout()
    }

    pub fn present_direct(&mut self, damage: &[Rect]) -> Result<u64, RetainedRendererError> {
        self.engine
            .present_direct(damage)
            .map_err(|_| RetainedRendererError::Composition)
    }

    pub fn hardware_cursor_needs_image(&self) -> bool {
        self.engine.hardware_cursor_needs_image()
    }

    pub fn update_hardware_cursor(
        &mut self,
        x: u32,
        y: u32,
        pixels: Option<&[u32]>,
        hot_x: u32,
        hot_y: u32,
    ) -> Result<bool, RetainedRendererError> {
        self.engine
            .update_hardware_cursor(x, y, pixels, hot_x, hot_y)
            .map_err(|_| RetainedRendererError::Composition)
    }

    /// Capture the canonical composed output rather than the boot framebuffer.
    /// Once a VirtIO presenter takes over, the boot framebuffer is intentionally
    /// no longer updated, so it cannot be used as the screenshot source.

    #[expect(dead_code, reason = "intentional kernel API surface")]
    pub const fn last_stats(&self) -> RenderStats {
        self.last_stats
    }
    pub const fn budget(&self) -> &SurfaceBudget {
        &self.budget
    }

    pub fn has_virtio_presenter(&self) -> bool {
        self.virtio_presenter.is_some()
    }

    pub fn present_virtio(&mut self, damage: &[Rect]) -> Result<bool, RetainedRendererError> {
        let Some((mut gpu, mut resource)) = self.virtio_presenter.take() else {
            return Ok(false);
        };
        let result = gpu.present(&mut resource, self.engine.output(), damage);
        match result {
            Ok(()) => {
                self.virtio_presenter = Some((gpu, resource));
                Ok(true)
            }
            Err(error) => {
                crate::debug_warn!(
                    "virtio-gpu presenter failed: {:?}; fallback=boot-framebuffer",
                    error
                );
                let _ = resource;
                gpu.reset();
                Ok(false)
            }
        }
    }
}
