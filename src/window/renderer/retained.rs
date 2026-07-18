use alloc::boxed::Box;
use alloc::collections::{BTreeMap, BTreeSet};

use crate::drivers::virtio::gpu::{ScanoutResource, VirtioGpu};
use crate::graphics::composition::{
    CompositionEngine, CompositionEngineKind, CpuCompositionEngine, RenderStats,
    VirglCompositionEngine,
};
use crate::graphics::scene::{Layer, SceneFrame};
use crate::graphics::surface::{
    Surface, SurfaceBudget, SurfaceClass, SurfaceDesc, SurfaceError, SurfaceId,
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
    engine: Box<dyn CompositionEngine>,
    budget: SurfaceBudget,
    last_stats: RenderStats,
    virtio_presenter: Option<(VirtioGpu, ScanoutResource)>,
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
            engine,
            budget,
            last_stats: RenderStats::default(),
            virtio_presenter,
        })
    }

    pub fn engine_kind(&self) -> CompositionEngineKind {
        self.engine.kind()
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

    pub fn compose(
        &mut self,
        scene: &SceneFrame,
        damage: &[Rect],
    ) -> Result<RenderStats, RetainedRendererError> {
        let mut snapshots = BTreeMap::<SurfaceId, alloc::vec::Vec<Rect>>::new();
        for layer in &scene.layers {
            if !layer.visible || layer.opacity == 0 || snapshots.contains_key(&layer.surface_id) {
                continue;
            }
            if let Some(surface) = self.surfaces.get(&layer.surface_id) {
                snapshots.insert(layer.surface_id, surface.damage_snapshot());
            }
        }
        let stats = self
            .engine
            .compose(scene, &self.surfaces, damage)
            .map_err(|_| RetainedRendererError::Composition)?;
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
    ) -> Result<bool, RetainedRendererError> {
        self.engine
            .update_hardware_cursor(x, y, pixels)
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
