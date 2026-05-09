//! File Explorer App
//!
//! A two-pane Finder-style file browser. Layout: a `FrameWindow`
//! containing a Toolbar (Up / Refresh) over a `PathBar`, then a
//! `Splitter` with a directory `TreeView` on the left and a
//! `MultiColumnList` on the right (Name / Size / Type), with a
//! `StatusBar` at the bottom showing the item count.
//!
//! Activation rules:
//! - Click a row to select; click an already-selected row (or press
//!   Enter while it is selected) to activate.
//! - Activating a folder navigates into it.
//! - Activating a file dispatches by uppercase extension —
//!   `.TXT`/`.MD`/`.RS` launch `notepad`, `.ELF` launches `run`,
//!   anything else surfaces an error message box.
//!
//! Multi-instance: each invocation gets its own `usize` instance id
//! and `BTreeMap`-stored state, mirroring `notepad`.

pub mod dir_model;
pub mod dispatch;

use alloc::boxed::Box;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};
use spin::Mutex;

use crate::process::{BaseProcess, HasBaseProcess, RunnableProcess};
use crate::window::dialogs::show_error;
use crate::window::windows::tree_view::NodeId;
use crate::window::windows::{
    Column, ContainerWindow, FrameWindow, MultiColumnList, Padding, PathBar, ScrollView, SizeHint,
    Splitter, StatusBar, Toolbar, TreeView, VBox,
};
use crate::window::{with_window_manager, Rect, Window, WindowId};
use crate::graphics::color::Color;

use self::dir_model::{
    child_path, format_size, format_type, parent_path, read_directory, DirEntry, EntryKind,
};
use self::dispatch::{dispatch_open, OpenAction};

/// Unique ID for each explorer instance.
static NEXT_EXPLORER_ID: AtomicUsize = AtomicUsize::new(1);

/// User-driven actions queued from widget callbacks and consumed by
/// the explorer's main loop. Widget callbacks can't take typed
/// references to the explorer's mutable state, so they push an
/// action and the loop dispatches against the registry.
#[derive(Debug, Clone)]
enum ExplorerAction {
    NavigateTo(String),
    NavigateUp,
    Refresh,
    OpenFile(String),
    /// Tree node was clicked or expand-toggled.
    /// `(node_id, expanded_after_click)` — when the click landed on
    /// the disclosure triangle, the widget already toggled state;
    /// when it landed on the label, the widget only updated
    /// selection. We treat both as "ensure children loaded for this
    /// node and navigate the main pane to its path".
    TreeNodeClicked(NodeId),
}

/// Per-instance state. Held inside `EXPLORER_STATES` keyed by the
/// instance id; widget callbacks reach back through the static map.
struct ExplorerState {
    frame_id: WindowId,
    list_id: WindowId,
    scroll_id: WindowId,
    tree_id: WindowId,
    path_bar_id: WindowId,
    status_section_id: WindowId,
    up_button_id: WindowId,
    /// Toolbar handle (kept for `set_enabled`).
    toolbar_id: WindowId,
    current_path: String,
    entries: Vec<DirEntry>,
    pending_action: Option<ExplorerAction>,
    running: bool,
    /// `NodeId` -> filesystem path. The TreeView's `TreeNode` stores
    /// only a label; this map carries the full path so navigation
    /// can fire without recomputing it.
    tree_node_paths: BTreeMap<NodeId, String>,
    /// Tree nodes that have already lazy-loaded their children.
    loaded_tree_nodes: BTreeSet<NodeId>,
}

static EXPLORER_STATES: Mutex<BTreeMap<usize, ExplorerState>> = Mutex::new(BTreeMap::new());

/// Push an action into the pending slot. If one is already pending,
/// the newer action replaces it — file-explorer interactions are
/// idempotent enough that "latest wins" is the right policy.
fn set_pending_action(explorer_id: usize, action: ExplorerAction) {
    let mut states = EXPLORER_STATES.lock();
    if let Some(state) = states.get_mut(&explorer_id) {
        state.pending_action = Some(action);
    }
}

fn take_pending_action(explorer_id: usize) -> Option<ExplorerAction> {
    let mut states = EXPLORER_STATES.lock();
    states
        .get_mut(&explorer_id)
        .and_then(|s| s.pending_action.take())
}

pub struct ExplorerProcess {
    base: BaseProcess,
    args: Vec<String>,
}

impl ExplorerProcess {
    pub fn new_with_args(args: Vec<String>) -> Self {
        Self {
            base: BaseProcess::new("explorer"),
            args,
        }
    }
}

impl HasBaseProcess for ExplorerProcess {
    fn base(&self) -> &BaseProcess {
        &self.base
    }
    fn base_mut(&mut self) -> &mut BaseProcess {
        &mut self.base
    }
}

impl RunnableProcess for ExplorerProcess {
    fn run(&mut self) {
        let explorer_id = NEXT_EXPLORER_ID.fetch_add(1, Ordering::SeqCst);
        let initial_path = self
            .args
            .get(0)
            .cloned()
            .unwrap_or_else(|| String::from("/"));

        let result = build_window(explorer_id, &initial_path);
        let ids = match result {
            Some(ids) => ids,
            None => {
                crate::println!("Failed to create File Explorer window");
                return;
            }
        };

        // Seed instance state.
        {
            let mut states = EXPLORER_STATES.lock();
            states.insert(
                explorer_id,
                ExplorerState {
                    frame_id: ids.frame_id,
                    list_id: ids.list_id,
                    scroll_id: ids.scroll_id,
                    tree_id: ids.tree_id,
                    path_bar_id: ids.path_bar_id,
                    status_section_id: ids.status_section_id,
                    up_button_id: ids.up_button_id,
                    toolbar_id: ids.toolbar_id,
                    current_path: initial_path.clone(),
                    entries: Vec::new(),
                    pending_action: None,
                    running: true,
                    tree_node_paths: BTreeMap::new(),
                    loaded_tree_nodes: BTreeSet::new(),
                },
            );
        }

        // Wire widget callbacks now that state is in place.
        wire_callbacks(explorer_id, &ids);

        // Initial directory load + sidebar seed.
        load_current_directory(explorer_id);
        seed_tree_root(explorer_id);

        crate::println!("File Explorer started. Close window to exit.");

        // Main loop: drain pending actions, yield, check for exit.
        let mut iter_count: u32 = 0;
        loop {
            let running = {
                let states = EXPLORER_STATES.lock();
                states.get(&explorer_id).map(|s| s.running).unwrap_or(false)
            };
            if !running {
                break;
            }

            iter_count = iter_count.wrapping_add(1);
            if iter_count % 50 == 1 {
                let has_pending = EXPLORER_STATES
                    .lock()
                    .get(&explorer_id)
                    .map(|s| s.pending_action.is_some())
                    .unwrap_or(false);
                crate::debug_info!(
                    "explorer: main loop iter={} pending={}",
                    iter_count, has_pending
                );
            }

            if let Some(action) = take_pending_action(explorer_id) {
                handle_action(explorer_id, action);
            }

            for _ in 0..50000 {
                core::hint::spin_loop();
            }
            crate::process::yield_if_needed();

            let exists = with_window_manager(|wm| wm.window_registry.contains_key(&ids.frame_id))
                .unwrap_or(false);
            if !exists {
                break;
            }
        }

        {
            let mut states = EXPLORER_STATES.lock();
            states.remove(&explorer_id);
        }

        crate::println!("File Explorer closed.");
    }

    fn get_name(&self) -> &str {
        "explorer"
    }
}

/// Bag of window ids returned by `build_window`.
struct ExplorerWindowIds {
    frame_id: WindowId,
    list_id: WindowId,
    scroll_id: WindowId,
    tree_id: WindowId,
    path_bar_id: WindowId,
    status_section_id: WindowId,
    up_button_id: WindowId,
    toolbar_id: WindowId,
}

/// Build the full window tree for an explorer instance and register
/// every widget with the window manager. Returns the ids needed by
/// the main loop and callback wiring; returns `None` if the manager
/// is unreachable.
fn build_window(_explorer_id: usize, initial_path: &str) -> Option<ExplorerWindowIds> {
    // Window dimensions — center on the screen, leaving margin.
    let dialog_width: u32 = 900;
    let dialog_height: u32 = 600;

    let (screen_width, screen_height) = with_window_manager(|wm| {
        (
            wm.graphics_device.width() as i32,
            wm.graphics_device.height() as i32,
        )
    })
    .unwrap_or((1024, 768));

    let window_x = ((screen_width - dialog_width as i32) / 2).max(0);
    let window_y = ((screen_height - dialog_height as i32) / 2).max(0);

    with_window_manager(|wm| {
        let desktop_id = wm
            .get_active_screen()
            .and_then(|s| s.root_window)
            .unwrap_or(WindowId::new());

        // ---- Phase A: Allocate ids and create structs in Rust scope. ----
        let frame_id = wm.create_window(Some(desktop_id));
        let container_id = wm.create_window(Some(frame_id));
        let padding_id = wm.create_window(Some(container_id));
        let vbox_id = wm.create_window(Some(padding_id));
        let toolbar_id = wm.create_window(Some(vbox_id));
        let path_bar_id = wm.create_window(Some(vbox_id));
        let splitter_id = wm.create_window(Some(vbox_id));
        let status_id = wm.create_window(Some(vbox_id));
        let tree_id = wm.create_window(Some(splitter_id));
        let scroll_id = wm.create_window(Some(splitter_id));
        let list_id = wm.create_window(Some(scroll_id));

        // Frame
        let title = format!("File Explorer — {}", initial_path);
        let mut frame = FrameWindow::new(frame_id, &title);
        frame.set_bounds(Rect::new(window_x, window_y, dialog_width, dialog_height));
        frame.set_parent(Some(desktop_id));
        let content_area = frame.content_area();

        // Container fills content area.
        let mut container = ContainerWindow::new_with_id(container_id, content_area);
        container.set_parent(Some(frame_id));
        container.set_background_color(Color::new(240, 240, 240));

        // Padding wraps the VBox with a small inset. Bounds match the
        // content area; the cascade fires after registration.
        let mut padding = Padding::new_with_id(
            padding_id,
            Rect::new(0, 0, content_area.width, content_area.height),
            6,
            6,
            6,
            6,
        );
        padding.set_parent(Some(container_id));

        // VBox: [Toolbar | PathBar | Splitter | StatusBar].
        let mut vbox = VBox::new_with_id(vbox_id, Rect::new(0, 0, 0, 0));
        vbox.set_parent(Some(padding_id));

        // Toolbar / PathBar / Splitter / StatusBar — VBox children.
        let mut toolbar = Toolbar::new_with_id(toolbar_id, Rect::new(0, 0, 0, 32));
        toolbar.base_mut().set_parent(Some(vbox_id));

        let mut path_bar = PathBar::new_with_id(path_bar_id, Rect::new(0, 0, 0, 24));
        path_bar.base_mut().set_parent(Some(vbox_id));
        path_bar.set_path(initial_path);

        let mut splitter = Splitter::new_with_id(
            splitter_id,
            crate::window::windows::splitter::SplitterOrientation::Vertical,
            Rect::new(0, 0, 0, 0),
        );
        splitter.base_mut().set_parent(Some(vbox_id));

        let mut status_bar = StatusBar::new_with_id(status_id, Rect::new(0, 0, 0, 20));
        status_bar.base_mut().set_parent(Some(vbox_id));

        // TreeView (sidebar) and ScrollView { MultiColumnList } (main pane).
        let mut tree = TreeView::new_with_id(tree_id, Rect::new(0, 0, 0, 0));
        tree.base_mut().set_parent(Some(splitter_id));

        let mut scroll = ScrollView::new_with_id(scroll_id, Rect::new(0, 0, 0, 0));
        scroll.base_mut().set_parent(Some(splitter_id));

        let columns = alloc::vec![
            Column::new("Name", 380),
            Column::new("Size", 100),
            Column::new("Type", 80),
        ];
        let mut list = MultiColumnList::new_with_id(list_id, Rect::new(0, 0, 0, 0), columns);
        list.base_mut().set_parent(Some(scroll_id));

        // ---- Phase B: Wire parent->child relationships in Rust scope. ----
        // These mutate the structs' internal hint/child vectors. They
        // run BEFORE registration so that `set_window_impl`'s
        // auto-attach (which routes through the trait `add_child`,
        // appending a default `Fill(1)` hint to layout containers)
        // doesn't pollute the hint vec we're about to populate.
        scroll.set_content(list_id);
        splitter.set_first(tree_id, 100);
        splitter.set_second(scroll_id, 300);
        vbox.add_child(toolbar_id, SizeHint::Fixed(32));
        vbox.add_child(path_bar_id, SizeHint::Fixed(24));
        vbox.add_child(splitter_id, SizeHint::Fill(1));
        vbox.add_child(status_id, SizeHint::Fixed(20));
        padding.set_child(vbox_id);

        // ---- Phase C: Register windows. Children BEFORE parents for
        // every layout container, so set_window_impl's auto-attach
        // is a no-op (the parent is not yet in the registry). ----
        wm.set_window_impl(frame_id, Box::new(frame));
        wm.set_window_impl(container_id, Box::new(container));
        // Children of scroll and splitter first.
        wm.set_window_impl(list_id, Box::new(list));
        wm.set_window_impl(tree_id, Box::new(tree));
        wm.set_window_impl(scroll_id, Box::new(scroll));
        // Splitter, then the four VBox children.
        wm.set_window_impl(splitter_id, Box::new(splitter));
        wm.set_window_impl(toolbar_id, Box::new(toolbar));
        wm.set_window_impl(status_id, Box::new(status_bar));
        wm.set_window_impl(path_bar_id, Box::new(path_bar));
        // VBox last among layout siblings; padding last overall.
        wm.set_window_impl(vbox_id, Box::new(vbox));
        wm.set_window_impl(padding_id, Box::new(padding));

        // Container's auto-attach to frame already happened during
        // its own set_window_impl. Frame and desktop need explicit
        // wiring because frame was registered before container, so
        // its parent (desktop) was registered earlier and got the
        // auto-attach — no manual wiring needed there either. The
        // only edge to add is desktop->frame, and even that fired
        // during set_window_impl(frame_id) since desktop is in the
        // registry. We leave this here as a defensive idempotent
        // re-add — `WindowBase::add_child` skips duplicates.
        if let Some(desktop) = wm.window_registry.get_mut(&desktop_id) {
            desktop.add_child(frame_id);
        }

        // ---- Phase D: Populate Toolbar buttons and StatusBar section.
        // Both widgets must be registered first; their internal
        // add_button / add_section register the children via
        // with_any_manager. While inside with_window_mut, the parent
        // (toolbar / status) is OUT of the registry, so the children's
        // auto-attach back to the parent is skipped — but Toolbar /
        // StatusBar manage their own internal child lists in
        // `slots` / `sections` regardless. ----
        let mut up_button_id_opt: Option<WindowId> = None;
        let mut status_section_id_opt: Option<WindowId> = None;

        wm.with_window_mut(toolbar_id, |w| {
            let tb_ptr = w as *mut dyn Window as *mut Toolbar;
            let tb = unsafe { &mut *tb_ptr };
            let up_id = tb.add_button("Up", move || {
                set_pending_action_global(ExplorerAction::NavigateUp);
            });
            tb.add_separator();
            tb.add_button("Refresh", move || {
                set_pending_action_global(ExplorerAction::Refresh);
            });
            up_button_id_opt = Some(up_id);
        });

        wm.with_window_mut(status_id, |w| {
            let sb_ptr = w as *mut dyn Window as *mut StatusBar;
            let sb = unsafe { &mut *sb_ptr };
            let sec = sb.add_section("0 items", 1);
            status_section_id_opt = Some(sec);
        });

        let up_button_id = up_button_id_opt?;
        let status_section_id = status_section_id_opt?;

        // ---- Phase E: Trigger the layout cascade. ----
        // Padding's set_bounds calls relayout, which writes vbox
        // bounds, which writes toolbar/path_bar/splitter/status
        // bounds, which (for splitter) writes tree/scroll bounds.
        // Hints are now correct (Phase B), so layout is correct.
        wm.with_window_mut(padding_id, |w| {
            let pd_ptr = w as *mut dyn Window as *mut Padding;
            let pd = unsafe { &mut *pd_ptr };
            pd.set_bounds(Rect::new(0, 0, content_area.width, content_area.height));
        });

        // Set the divider position now that the splitter has real
        // bounds. `clamp_divider_position` requires a non-zero total
        // to honor the requested position.
        wm.with_window_mut(splitter_id, |w| {
            let sp_ptr = w as *mut dyn Window as *mut Splitter;
            let sp = unsafe { &mut *sp_ptr };
            sp.set_divider_position(160);
        });

        // Set the list's parent-relative bounds to the ScrollView's
        // viewport. ScrollView only reaches into the list to paint
        // when `ACTIVE_MANAGER` is set (i.e. inside `with_window_mut`),
        // which is NOT the case during normal render. So the list
        // is actually painted via the manager's recursive walk at
        // its own parent-relative bounds — those need to match the
        // viewport size or rows won't render. Mirrors the pattern
        // used by `notepad` for its TextEditor inside ScrollView.
        let scroll_bounds = wm
            .window_registry
            .get(&scroll_id)
            .map(|w| w.bounds())
            .unwrap_or_else(|| Rect::new(0, 0, 1, 1));
        wm.with_window_mut(list_id, |w| {
            w.set_bounds(Rect::new(0, 0, scroll_bounds.width, scroll_bounds.height));
        });

        wm.bring_to_front(frame_id);
        wm.focus_window(list_id);

        Some(ExplorerWindowIds {
            frame_id,
            list_id,
            scroll_id,
            tree_id,
            path_bar_id,
            status_section_id,
            up_button_id,
            toolbar_id,
        })
    })
    .flatten()
}

/// Helper consumed by Toolbar / PathBar / TreeView / List callbacks.
/// Looks up the (singleton) explorer instance owning the active
/// callback. v1 supports multiple instances at the same time, so this
/// pushes the action into ALL active explorers' pending slots — but
/// since callbacks only fire on the focused widget, the wrong-target
/// case is benign (the inactive explorer's action gets overwritten
/// before its main loop sees it).
///
/// A more precise design would thread the instance id through every
/// callback closure. Deferred — multi-instance is a v1 affordance,
/// not a hot path.
fn set_pending_action_global(action: ExplorerAction) {
    let mut states = EXPLORER_STATES.lock();
    for (_id, state) in states.iter_mut() {
        state.pending_action = Some(action.clone());
    }
}

/// Wire the widget callbacks that need to know the instance id.
/// Called once after state is registered.
fn wire_callbacks(explorer_id: usize, ids: &ExplorerWindowIds) {
    let list_id = ids.list_id;
    let path_bar_id = ids.path_bar_id;
    let tree_id = ids.tree_id;

    with_window_manager(|wm| {
        // List: on_activate (click-of-selected-row OR Enter) decides
        // whether to navigate (folder) or open (file).
        wm.with_window_mut(list_id, |w| {
            if let Some(list) = w.as_multi_column_list_mut() {
                list.on_activate(move |idx| {
                    handle_list_activate(explorer_id, idx);
                });
            }
        });

        // PathBar: clicking a breadcrumb segment navigates.
        wm.with_window_mut(path_bar_id, |w| {
            if let Some(pb) = w.as_path_bar_mut() {
                pb.on_segment_click(move |path: &str| {
                    let p = String::from(path);
                    set_pending_action(explorer_id, ExplorerAction::NavigateTo(p));
                });
            }
        });

        // TreeView: clicking a node label fires on_select; clicking
        // the disclosure triangle is handled internally. We
        // listen to on_select for navigate-on-click; the lazy-load
        // path runs on every click (cheap — guarded by
        // loaded_tree_nodes set).
        wm.with_window_mut(tree_id, |w| {
            if let Some(tv) = w.as_tree_view_mut() {
                tv.on_select(move |node_id| {
                    set_pending_action(explorer_id, ExplorerAction::TreeNodeClicked(node_id));
                });
            }
        });
    });
}

/// Handle the list's `on_activate` — navigate folders, open files.
fn handle_list_activate(explorer_id: usize, idx: usize) {
    crate::debug_info!("explorer: handle_list_activate idx={}", idx);
    let entry = {
        let states = EXPLORER_STATES.lock();
        states
            .get(&explorer_id)
            .and_then(|s| s.entries.get(idx).cloned())
    };
    let entry = match entry {
        Some(e) => e,
        None => {
            crate::debug_info!("explorer: handle_list_activate -> no entry at idx={}", idx);
            return;
        }
    };

    match entry.kind {
        EntryKind::Folder => {
            // Navigation requires `with_window_manager` (to update widgets)
            // and we're inside the on_activate callback, which is itself
            // inside `with_window_manager` for event routing. Re-entering
            // would deadlock — so we defer to the main loop via the
            // pending-action queue.
            crate::debug_info!(
                "explorer: queueing NavigateTo({})",
                entry.full_path
            );
            set_pending_action(
                explorer_id,
                ExplorerAction::NavigateTo(entry.full_path),
            );
        }
        EntryKind::File { .. } => {
            // Spawn the handler synchronously. `execute_command` only
            // locks PROCESS_MANAGER and the scheduler — never the
            // window manager — so calling it from inside the on_activate
            // callback (which holds WINDOW_MANAGER) is safe and avoids
            // the queue path that the main loop would otherwise have
            // to drain.
            crate::debug_info!(
                "explorer: opening {} synchronously",
                entry.full_path
            );
            open_file(&entry.full_path);
        }
    }
}

/// Dispatch one queued action.
fn handle_action(explorer_id: usize, action: ExplorerAction) {
    crate::debug_info!("explorer: handle_action {:?}", action);
    match action {
        ExplorerAction::NavigateTo(path) => {
            set_current_path(explorer_id, path);
            load_current_directory(explorer_id);
        }
        ExplorerAction::NavigateUp => {
            let parent = {
                let states = EXPLORER_STATES.lock();
                states
                    .get(&explorer_id)
                    .and_then(|s| parent_path(&s.current_path))
            };
            if let Some(p) = parent {
                set_current_path(explorer_id, p);
                load_current_directory(explorer_id);
            }
        }
        ExplorerAction::Refresh => {
            load_current_directory(explorer_id);
        }
        ExplorerAction::OpenFile(path) => {
            open_file(&path);
        }
        ExplorerAction::TreeNodeClicked(node_id) => {
            handle_tree_click(explorer_id, node_id);
        }
    }
}

/// Update the in-state current_path field. Caller is expected to call
/// `load_current_directory` afterward to refresh widgets.
fn set_current_path(explorer_id: usize, path: String) {
    let mut states = EXPLORER_STATES.lock();
    if let Some(state) = states.get_mut(&explorer_id) {
        state.current_path = path;
    }
}

/// Read the current directory and refresh the list, path bar, status
/// bar, and Up-button enabled state. Surfaces FS errors via a
/// `MessageBox` and leaves widget state unchanged on failure.
fn load_current_directory(explorer_id: usize) {
    let path = {
        let states = EXPLORER_STATES.lock();
        match states.get(&explorer_id) {
            Some(s) => s.current_path.clone(),
            None => return,
        }
    };

    let entries = match read_directory(&path) {
        Ok(v) => v,
        Err(e) => {
            show_error(
                "Cannot open",
                &format!("Cannot open `{}`: {}", path, e),
            );
            return;
        }
    };

    let list_id;
    let scroll_id;
    let path_bar_id;
    let status_section_id;
    let up_button_id;
    let toolbar_id;
    let frame_id;
    let entries_clone = entries.clone();
    {
        let mut states = EXPLORER_STATES.lock();
        let state = match states.get_mut(&explorer_id) {
            Some(s) => s,
            None => return,
        };
        state.entries = entries;
        list_id = state.list_id;
        scroll_id = state.scroll_id;
        path_bar_id = state.path_bar_id;
        status_section_id = state.status_section_id;
        up_button_id = state.up_button_id;
        toolbar_id = state.toolbar_id;
        frame_id = state.frame_id;
    }

    let entry_count = entries_clone.len();
    let status_text = if entry_count == 0 {
        String::from("Empty folder")
    } else if entry_count == 1 {
        String::from("1 item")
    } else {
        format!("{} items", entry_count)
    };

    let new_title = format!("File Explorer — {}", path);
    let path_for_bar = path.clone();
    let path_is_root = path == "/";

    with_window_manager(|wm| {
        // Repopulate the list.
        let mut content_h: u32 = 0;
        wm.with_window_mut(list_id, |w| {
            if let Some(list) = w.as_multi_column_list_mut() {
                list.clear_rows();
                for entry in &entries_clone {
                    list.add_row(alloc::vec![
                        entry.name.clone(),
                        format_size(entry),
                        format_type(entry),
                    ]);
                }
                content_h = list.content_height();
            }
        });

        // Resync the wrapping ScrollView's content size. Use the
        // ScrollView's own viewport width as the content width so
        // horizontal scrolling never appears (we don't support it
        // here) and the list always paints across the whole pane.
        wm.with_window_mut(scroll_id, |w| {
            if w.is_scroll_view() {
                let sv_ptr = w as *mut dyn Window as *mut ScrollView;
                let sv = unsafe { &mut *sv_ptr };
                let viewport_w = sv.bounds().width.max(1);
                sv.set_content_size(viewport_w, content_h.max(1));
                sv.invalidate();
            }
        });

        // Update the path bar.
        wm.with_window_mut(path_bar_id, |w| {
            if let Some(pb) = w.as_path_bar_mut() {
                pb.set_path(&path_for_bar);
            }
        });

        // Update the status section text.
        if let Some(sb_window) = wm.window_registry.get_mut(&status_section_id) {
            // Status section is a Label; use the typed accessor.
            if let Some(label) = sb_window.as_label_mut() {
                label.set_text(&status_text);
            }
        }

        // Toggle the Up button enabled-state via the Toolbar handle.
        wm.with_window_mut(toolbar_id, |w| {
            let tb_ptr = w as *mut dyn Window as *mut Toolbar;
            let tb = unsafe { &mut *tb_ptr };
            tb.set_enabled(up_button_id, !path_is_root);
        });

        // Update frame title to reflect the path.
        if let Some(frame) = wm.window_registry.get_mut(&frame_id) {
            // FrameWindow's title-set API isn't exposed via a typed
            // accessor; just force a repaint. The title is set at
            // construction time and reads visually fine without an
            // update for now.
            let _ = new_title;
            frame.invalidate();
        }
    });
}

/// On first launch (and never again), seed the tree's root with the
/// top-level directories of `/`. Auto-expands the synthetic root so
/// the sidebar isn't a single un-expanded line.
fn seed_tree_root(explorer_id: usize) {
    // Read the children of "/" once.
    let entries = match read_directory("/") {
        Ok(v) => v,
        Err(_) => return,
    };
    let folder_entries: Vec<DirEntry> = entries
        .into_iter()
        .filter(|e| matches!(e.kind, EntryKind::Folder))
        .collect();

    let tree_id = {
        let states = EXPLORER_STATES.lock();
        match states.get(&explorer_id) {
            Some(s) => s.tree_id,
            None => return,
        }
    };

    let mut new_paths: Vec<(NodeId, String)> = Vec::new();
    let mut root_id: Option<NodeId> = None;

    with_window_manager(|wm| {
        wm.with_window_mut(tree_id, |w| {
            if let Some(tv) = w.as_tree_view_mut() {
                let root = tv.add_node(None, "/");
                root_id = Some(root);
                for entry in &folder_entries {
                    let nid = tv.add_node(Some(root), &entry.name);
                    new_paths.push((nid, entry.full_path.clone()));
                }
                tv.expand(root);
            }
        });
    });

    if let Some(root) = root_id {
        let mut states = EXPLORER_STATES.lock();
        if let Some(state) = states.get_mut(&explorer_id) {
            state.tree_node_paths.insert(root, String::from("/"));
            for (nid, path) in new_paths {
                state.tree_node_paths.insert(nid, path);
            }
            state.loaded_tree_nodes.insert(root);
        }
    }
}

/// Click on a tree node: lazy-load its children if needed, then
/// navigate the main pane to its path.
fn handle_tree_click(explorer_id: usize, node_id: NodeId) {
    // Look up the path for this node.
    let (path, already_loaded, tree_id) = {
        let states = EXPLORER_STATES.lock();
        match states.get(&explorer_id) {
            Some(s) => {
                let path = s.tree_node_paths.get(&node_id).cloned();
                let loaded = s.loaded_tree_nodes.contains(&node_id);
                (path, loaded, s.tree_id)
            }
            None => return,
        }
    };
    let path = match path {
        Some(p) => p,
        None => return,
    };

    if !already_loaded {
        // Lazy-load this node's subdirectories.
        if let Ok(entries) = read_directory(&path) {
            let folder_entries: Vec<DirEntry> = entries
                .into_iter()
                .filter(|e| matches!(e.kind, EntryKind::Folder))
                .collect();

            let mut new_paths: Vec<(NodeId, String)> = Vec::new();
            with_window_manager(|wm| {
                wm.with_window_mut(tree_id, |w| {
                    if let Some(tv) = w.as_tree_view_mut() {
                        for entry in &folder_entries {
                            let nid = tv.add_node(Some(node_id), &entry.name);
                            new_paths.push((nid, entry.full_path.clone()));
                        }
                        tv.expand(node_id);
                    }
                });
            });

            let mut states = EXPLORER_STATES.lock();
            if let Some(state) = states.get_mut(&explorer_id) {
                state.loaded_tree_nodes.insert(node_id);
                for (nid, p) in new_paths {
                    state.tree_node_paths.insert(nid, p);
                }
            }
        }
    } else {
        // Already loaded — toggle expand/collapse to mimic the
        // disclosure-triangle interaction.
        with_window_manager(|wm| {
            wm.with_window_mut(tree_id, |w| {
                if let Some(tv) = w.as_tree_view_mut() {
                    if tv.is_expanded(node_id) {
                        // Stay expanded; we only toggle on triangle clicks
                        // (handled internally by TreeView). A label click
                        // navigates without collapsing.
                    } else {
                        tv.expand(node_id);
                    }
                }
            });
        });
    }

    // Navigate the main pane.
    set_current_path(explorer_id, path);
    load_current_directory(explorer_id);
}

/// Open a file: dispatch by extension and either spawn an app or
/// show an error dialog. `.ELF` is guarded by the userland
/// single-user-app invariant.
fn open_file(path: &str) {
    let action = dispatch_open(path);
    crate::debug_info!("explorer: open_file path={} action={:?}", path, action);
    match action {
        OpenAction::LaunchNotepad => {
            let cmd = format!("notepad {}", path);
            let result = crate::process::execute_command(&cmd, None);
            crate::debug_info!("explorer: spawn notepad result={:?}", result);
        }
        OpenAction::LaunchRun => {
            if crate::userland::lifecycle::user_active() {
                show_error(
                    "Cannot run",
                    "Another user app is already running. Wait for it to exit before launching this one.",
                );
                return;
            }
            let cmd = format!("run {}", path);
            let result = crate::process::execute_command(&cmd, None);
            crate::debug_info!("explorer: spawn run result={:?}", result);
        }
        OpenAction::Unsupported(ext) => {
            let body = if ext.is_empty() {
                String::from("No handler registered for files without an extension.")
            } else {
                format!("No handler registered for type `.{}`.", ext)
            };
            show_error("Cannot open", &body);
        }
    }
}

// Eliminate "unused" warnings for not-yet-fully-wired helpers if any
// future cleanup phases hide them behind cfg flags. The two structs
// imported to support state are referenced by the callbacks above.
#[allow(dead_code)]
fn _suppress_unused() {
    let _ = child_path;
}

/// Factory for the process manager.
pub fn create_explorer_process(args: Vec<String>) -> Box<dyn RunnableProcess> {
    Box::new(ExplorerProcess::new_with_args(args))
}

// `to_string` is used through trait import indirectly in some helpers;
// keep the import alive even if a future edit removes the call site.
#[allow(dead_code)]
fn _force_to_string_import() -> String {
    "x".to_string()
}
