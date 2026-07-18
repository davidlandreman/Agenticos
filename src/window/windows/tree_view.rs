//! `TreeView` — hierarchical-list widget with expand/collapse nodes,
//! the unified `Selection` model, and keyboard navigation.
//!
//! The tree's underlying model is a flat `Vec<TreeNode>`; each node
//! carries `depth`, `expanded`, `has_children`, and a label. A
//! `visible_rows: Vec<NodeId>` cache (recomputed on every expand /
//! collapse) records which nodes are currently visible in render
//! order, skipping any subtree whose root is collapsed.
//!
//! Selection is delegated to the shared [`Selection`] model. Indices
//! stored in `Selection` are positions into `visible_rows` (not
//! into the flat node vec), since that's the row-index space the
//! user manipulates with mouse and arrow keys. When `visible_rows`
//! changes (via expand / collapse), selection is remapped to keep
//! the user pointed at a sensible row — collapsing a node that
//! contains the selection moves it to the collapsing parent.
//!
//! Pinned constants:
//! - `INDENT_PX` — 16 px per depth level.
//! - Disclosure-triangle hit zone — 16x16 square at row-relative
//!   `x = depth * INDENT_PX`.
//!
//! Painting:
//! - Each visible row is `[indent][▶ or ▼][label]`. The disclosure
//!   triangle is drawn with `draw_line` calls inside the 16x16 cell;
//!   leaf nodes draw no triangle.
//! - The widget paints its full content rect (so a wrapping
//!   `ScrollView` can clip and translate). The row layout assumes
//!   the bounds are the content rect, the same convention `List` uses.
//!
//! Scroll integration:
//! - The widget does NOT draw scrollbars. Wrap in a `ScrollView`
//!   and call `scroll_view.set_content_size(width, tree.content_height())`.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

use crate::graphics::color::Color;
use crate::graphics::fonts::core_font::get_default_font;
use crate::window::event::{KeyCode, MouseEventType};
use crate::window::selection::{ArrowDirection, ClickMods, Selection, SelectionMode};
use crate::window::{Event, EventResult, GraphicsDevice, Rect, Window, WindowId};

use super::base::WindowBase;

/// Pixels of indentation per depth level.
pub const INDENT_PX: i32 = 16;

/// Width of the disclosure-triangle hit zone (also the cell the
/// triangle is drawn inside).
pub const DISCLOSURE_PX: i32 = 16;

/// Type alias for node identifiers — indices into the flat node
/// vec. Stable for the lifetime of the `TreeView` (no node removal
/// in v1).
pub type NodeId = usize;

/// A single node in the tree's flat model.
#[derive(Debug, Clone)]
pub struct TreeNode {
    pub label: String,
    pub depth: usize,
    pub expanded: bool,
    pub has_children: bool,
    /// Parent NodeId (None for roots).
    pub parent: Option<NodeId>,
}

/// Callback fired on Enter when a node is "activated" (default
/// action). Receives the activated `NodeId`.
pub type ActivateCallback = Box<dyn FnMut(NodeId) + Send>;

/// Callback fired when the user clicks on a node label (selection
/// changed via mouse). Receives the newly-selected `NodeId`.
pub type SelectCallback = Box<dyn FnMut(NodeId) + Send>;

/// Hierarchical list with expand/collapse nodes.
pub struct TreeView {
    base: WindowBase,
    nodes: Vec<TreeNode>,
    /// Row order: indices into `nodes` for currently visible rows.
    visible_rows: Vec<NodeId>,
    /// Selection state — indices are into `visible_rows`.
    selection: Selection,
    selection_mode: SelectionMode,
    row_height: usize,
    on_activate: Option<ActivateCallback>,
    on_select: Option<SelectCallback>,
    bg_color: Color,
    text_color: Color,
    selected_bg_color: Color,
    selected_text_color: Color,
}

impl TreeView {
    /// Create a new `TreeView` with the given outer bounds.
    #[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
    pub fn new(bounds: Rect) -> Self {
        Self::new_with_id(WindowId::new(), bounds)
    }

    /// Create a new `TreeView` with a specific window id.
    pub fn new_with_id(id: WindowId, bounds: Rect) -> Self {
        TreeView {
            base: WindowBase::new_with_id(id, bounds),
            nodes: Vec::new(),
            visible_rows: Vec::new(),
            selection: Selection::None,
            selection_mode: SelectionMode::Single,
            row_height: 16,
            on_activate: None,
            on_select: None,
            bg_color: crate::window::PALETTE_CONTENT_BG,
            text_color: crate::window::PALETTE_TEXT,
            selected_bg_color: crate::window::PALETTE_HIGHLIGHT_BG,
            selected_text_color: crate::window::PALETTE_HIGHLIGHT_TEXT,
        }
    }

    /// Add a new node under `parent` (or as a root when `parent`
    /// is `None`). The node is appended to the flat `nodes` vec
    /// (so existing `NodeId`s remain stable across `add_node`
    /// calls). Display order is computed by `recompute_visible_rows`
    /// via a tree walk using parent links. New nodes default to
    /// `expanded == false`. Returns the new node's `NodeId`.
    pub fn add_node(&mut self, parent: Option<NodeId>, label: &str) -> NodeId {
        let depth = match parent {
            Some(p) => self.nodes[p].depth + 1,
            None => 0,
        };

        if let Some(p) = parent {
            self.nodes[p].has_children = true;
        }

        let new_id = self.nodes.len();
        self.nodes.push(TreeNode {
            label: String::from(label),
            depth,
            expanded: false,
            has_children: false,
            parent,
        });

        self.recompute_visible_rows();
        self.base.invalidate();
        new_id
    }

    /// Expand the given node (a no-op for leaves and already-
    /// expanded nodes).
    pub fn expand(&mut self, id: NodeId) {
        if id >= self.nodes.len() {
            return;
        }
        if !self.nodes[id].has_children {
            return;
        }
        if self.nodes[id].expanded {
            return;
        }
        self.nodes[id].expanded = true;
        self.recompute_visible_rows();
        self.base.invalidate();
    }

    /// Collapse the given node. If the current selection lives
    /// inside the collapsed subtree, selection moves to the
    /// collapsed node itself.
    pub fn collapse(&mut self, id: NodeId) {
        if id >= self.nodes.len() {
            return;
        }
        if !self.nodes[id].has_children {
            return;
        }
        if !self.nodes[id].expanded {
            return;
        }

        // Capture the selected NodeId (if any) before the row
        // layout changes.
        let prior_selected_node = self
            .selection
            .iter()
            .next()
            .and_then(|row| self.visible_rows.get(row).copied());

        self.nodes[id].expanded = false;
        self.recompute_visible_rows();

        // If the selected node was inside the collapsed subtree
        // it's no longer in `visible_rows` — move selection to
        // the collapsing parent's row.
        if let Some(sel_nid) = prior_selected_node {
            if !self.visible_rows.contains(&sel_nid) {
                if let Some(parent_row) = self.row_of_node(id) {
                    self.selection = Selection::Single(parent_row);
                } else {
                    self.selection = Selection::None;
                }
            } else {
                // Re-anchor: the selected node may be at a different
                // row index after the collapse.
                if let Some(new_row) = self.row_of_node(sel_nid) {
                    self.selection = Selection::Single(new_row);
                }
            }
        }

        self.base.invalidate();
    }

    /// Returns whether the given node is currently expanded.
    pub fn is_expanded(&self, id: NodeId) -> bool {
        self.nodes
            .get(id)
            .map(|n| n.expanded)
            .unwrap_or(false)
    }

    /// Borrow the underlying selection state. Indices are in
    /// `visible_rows` index space.
    #[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
    pub fn selection(&self) -> &Selection {
        &self.selection
    }

    /// Get the currently selected `NodeId`, if any. Convenience
    /// wrapper around `selection().iter().next()` plus a lookup
    /// into `visible_rows`.
    pub fn selected_node(&self) -> Option<NodeId> {
        self.selection
            .iter()
            .next()
            .and_then(|row| self.visible_rows.get(row).copied())
    }

    /// Configure the selection mode. Switching `Multi` -> `Single`
    /// collapses the existing selection to its first element.

    /// Current selection mode.

    /// Register the activate callback (fired on Enter).

    /// Register the select callback (fired when the user clicks a node
    /// label, changing the selection). Not fired by arrow-key navigation
    /// — keyboard navigation only commits to the new selection on Enter
    /// (which fires `on_activate`).
    pub fn on_select<F>(&mut self, callback: F)
    where
        F: FnMut(NodeId) + Send + 'static,
    {
        self.on_select = Some(Box::new(callback));
    }

    /// Number of currently visible rows (non-collapsed nodes).
    #[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
    pub fn visible_row_count(&self) -> usize {
        self.visible_rows.len()
    }

    /// `NodeId` at the given visible row, if any.
    #[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
    pub fn node_at_row(&self, row: usize) -> Option<NodeId> {
        self.visible_rows.get(row).copied()
    }

    /// Row height in pixels.

    /// Natural content height in pixels — feed this to a wrapping
    /// `ScrollView::set_content_size`.
    #[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
    pub fn content_height(&self) -> u32 {
        (self.visible_rows.len() * self.row_height) as u32
    }

    /// Programmatically set the selection to a particular row,
    /// or clear it with `None`.
    #[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
    pub fn set_selected_row(&mut self, row: Option<usize>) {
        let new_sel = match row.filter(|&r| r < self.visible_rows.len()) {
            Some(r) => Selection::Single(r),
            None => Selection::None,
        };
        if self.selection != new_sel {
            self.selection = new_sel;
            self.base.invalidate();
        }
    }

    // ---------------------------------------------------------------
    // Internal helpers
    // ---------------------------------------------------------------

    /// Walk the tree in pre-order using parent links, skipping
    /// subtrees of any node that is currently collapsed. The
    /// result is the new `visible_rows` order.
    fn recompute_visible_rows(&mut self) {
        self.visible_rows.clear();
        // Collect roots (nodes with no parent) in insertion order.
        let roots: Vec<NodeId> = self
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(idx, n)| if n.parent.is_none() { Some(idx) } else { None })
            .collect();
        for root in roots {
            self.walk_visible(root);
        }
    }

    /// Recursive helper for `recompute_visible_rows` — append
    /// `id` and (if expanded) all of its descendants in pre-order.
    fn walk_visible(&mut self, id: NodeId) {
        self.visible_rows.push(id);
        if !self.nodes[id].expanded {
            return;
        }
        // Gather children of `id` in insertion order.
        let children: Vec<NodeId> = self
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(idx, n)| {
                if n.parent == Some(id) {
                    Some(idx)
                } else {
                    None
                }
            })
            .collect();
        for child in children {
            self.walk_visible(child);
        }
    }

    /// Return the visible-row index of a given NodeId, if visible.
    fn row_of_node(&self, id: NodeId) -> Option<usize> {
        self.visible_rows.iter().position(|&n| n == id)
    }

    /// Convert a row-relative y coordinate (in the same frame as
    /// `MouseEvent::position`) to a visible-row index, if any.
    fn y_to_row(&self, y: i32) -> Option<usize> {
        let bounds = self.base.bounds();
        let relative_y = y - bounds.y;
        if relative_y < 0 || self.row_height == 0 {
            return None;
        }
        let row = (relative_y as usize) / self.row_height;
        if row < self.visible_rows.len() {
            Some(row)
        } else {
            None
        }
    }

    /// Apply a click to the selection model.
    fn apply_click(&mut self, row: usize, mods: ClickMods) {
        let before = self.selection.clone();
        self.selection.click(row, mods, self.selection_mode);
        if before != self.selection {
            self.base.invalidate();
        }
    }

    /// Apply an arrow-key navigation step.
    fn apply_arrow_vertical(&mut self, direction: ArrowDirection, mods: ClickMods) {
        if self.visible_rows.is_empty() {
            return;
        }
        let before = self.selection.clone();
        self.selection
            .arrow(direction, self.visible_rows.len(), mods, self.selection_mode);
        if before != self.selection {
            self.base.invalidate();
        }
    }

    /// Right-arrow semantics: on a collapsed non-leaf, expand.
    /// On an expanded non-leaf, move to first child. On a leaf,
    /// no-op.
    fn apply_arrow_right(&mut self) {
        let Some(row) = self.selection.iter().next() else {
            return;
        };
        let Some(nid) = self.visible_rows.get(row).copied() else {
            return;
        };
        let node = &self.nodes[nid];
        if !node.has_children {
            return;
        }
        if !node.expanded {
            self.expand(nid);
            // After expanding, the row index of `nid` is unchanged
            // (nodes above it didn't move) — keep selection where
            // it is.
        } else {
            // Move to first child: that is the row immediately
            // after `row` in `visible_rows` (because pre-order
            // ensures the first child follows the parent in flat
            // order, and the parent is currently expanded).
            let next_row = row + 1;
            if next_row < self.visible_rows.len() {
                self.selection = Selection::Single(next_row);
                self.base.invalidate();
            }
        }
    }

    /// Left-arrow semantics: on an expanded node, collapse.
    /// Otherwise, move to parent (if any).
    fn apply_arrow_left(&mut self) {
        let Some(row) = self.selection.iter().next() else {
            return;
        };
        let Some(nid) = self.visible_rows.get(row).copied() else {
            return;
        };
        let node = &self.nodes[nid];
        if node.has_children && node.expanded {
            self.collapse(nid);
            return;
        }
        // Collapsed or leaf — move selection to parent row, if any.
        if let Some(parent_id) = node.parent {
            if let Some(parent_row) = self.row_of_node(parent_id) {
                self.selection = Selection::Single(parent_row);
                self.base.invalidate();
            }
        }
    }

    /// Enter semantics: fire `on_activate` for the selected node.
    fn fire_activate(&mut self) {
        let Some(nid) = self.selected_node() else {
            return;
        };
        if let Some(ref mut callback) = self.on_activate {
            callback(nid);
        }
    }

    /// Draw the disclosure triangle for a node inside its 16x16
    /// cell at top-left `(x, y)`. `expanded == true` draws ▼,
    /// otherwise ▶.
    fn draw_disclosure_triangle(
        &self,
        device: &mut dyn GraphicsDevice,
        x: i32,
        y: i32,
        expanded: bool,
        color: Color,
    ) {
        // Triangle inscribed inside the 16x16 cell with a small
        // inset. Drawn using `draw_line` calls — three edges plus
        // a few horizontal fill lines for a "filled" appearance.
        // The cell's inset is 4 px on each side, giving an 8x8
        // inner triangle box.
        let inset = 4;
        let lx = x + inset;
        let ty = y + inset;
        let cell_inner = DISCLOSURE_PX - 2 * inset;
        let rx = lx + cell_inner;
        let by = ty + cell_inner;
        let cx = lx + cell_inner / 2;
        let cy = ty + cell_inner / 2;

        if expanded {
            // ▼ — base along the top, apex at bottom-center.
            // Outline:
            device.draw_line(lx, ty, rx, ty, color);
            device.draw_line(lx, ty, cx, by, color);
            device.draw_line(rx, ty, cx, by, color);
            // Filled rows:
            for fy in 0..cell_inner / 2 {
                let span = (cell_inner / 2) - fy;
                device.draw_line(cx - span, ty + fy, cx + span, ty + fy, color);
            }
        } else {
            // ▶ — base along the left, apex at right-center.
            device.draw_line(lx, ty, lx, by, color);
            device.draw_line(lx, ty, rx, cy, color);
            device.draw_line(lx, by, rx, cy, color);
            for fx in 0..cell_inner / 2 {
                let span = (cell_inner / 2) - fx;
                device.draw_line(lx + fx, cy - span, lx + fx, cy + span, color);
            }
        }
    }
}

impl Window for TreeView {
    fn base(&self) -> &WindowBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut WindowBase {
        &mut self.base
    }

    fn can_focus(&self) -> bool {
        true
    }

    fn as_tree_view_mut(&mut self) -> Option<&mut TreeView> {
        Some(self)
    }

    fn paint(&mut self, device: &mut dyn GraphicsDevice) {
        if !self.base.visible() {
            return;
        }

        let bounds = self.base.bounds();
        let x = bounds.x;
        let y = bounds.y;
        let width = bounds.width;
        let row_height = self.row_height as i32;

        device.fill_rect(x, y, width, bounds.height, self.bg_color);

        let font = get_default_font();
        let line_h = font.line_height() as usize;
        let text_padding: i32 = 2;

        for (row_index, &nid) in self.visible_rows.iter().enumerate() {
            let row_y = y + (row_index as i32) * row_height;
            let node = &self.nodes[nid];
            let is_selected = self.selection.is_selected(row_index);
            let depth_px = (node.depth as i32) * INDENT_PX;

            if is_selected {
                device.fill_rect(
                    x + 1,
                    row_y,
                    width.saturating_sub(2),
                    self.row_height as u32,
                    self.selected_bg_color,
                );
            }

            let fg = if is_selected {
                self.selected_text_color
            } else {
                self.text_color
            };

            // Draw disclosure triangle if this node has children.
            if node.has_children {
                let cell_x = x + depth_px;
                let cell_y = row_y + (row_height - DISCLOSURE_PX) / 2;
                self.draw_disclosure_triangle(
                    device,
                    cell_x,
                    cell_y,
                    node.expanded,
                    fg,
                );
            }

            // Draw label after the disclosure cell.
            let text_x = x + depth_px + DISCLOSURE_PX + text_padding;
            let text_y = row_y + (row_height - line_h as i32) / 2;
            device.draw_text(text_x, text_y, &node.label, font.as_font(), fg);
        }

        self.base.clear_needs_repaint();
    }

    fn handle_event(&mut self, event: Event) -> EventResult {
        match event {
            Event::Mouse(mouse_event) => {
                let bounds = self.base.bounds();
                if !bounds.contains_point(mouse_event.position) {
                    return EventResult::Ignored;
                }

                match mouse_event.event_type {
                    MouseEventType::ButtonDown if mouse_event.buttons.left => {
                        let Some(row) = self.y_to_row(mouse_event.position.y) else {
                            return EventResult::Handled;
                        };
                        let nid = self.visible_rows[row];
                        let depth = self.nodes[nid].depth;
                        let has_children = self.nodes[nid].has_children;

                        // Row-relative x (same frame as bounds.x).
                        let relative_x = mouse_event.position.x - bounds.x;
                        let triangle_left = (depth as i32) * INDENT_PX;
                        let triangle_right = triangle_left + DISCLOSURE_PX;

                        if relative_x >= triangle_left
                            && relative_x < triangle_right
                            && has_children
                        {
                            // Click on disclosure triangle: toggle
                            // expand/collapse without changing selection.
                            if self.nodes[nid].expanded {
                                self.collapse(nid);
                            } else {
                                self.expand(nid);
                            }
                        } else {
                            // Click on label (or in the indent of a leaf):
                            // select the row.
                            let mods = ClickMods::new(
                                mouse_event.modifiers.shift,
                                mouse_event.modifiers.ctrl,
                            );
                            self.apply_click(row, mods);
                            // Fire on_select with the now-selected NodeId
                            // (not the row index — callers want the
                            // stable node identity, not the ephemeral
                            // visible-row position).
                            if let Some(nid) =
                                self.visible_rows.get(row).copied()
                            {
                                if let Some(ref mut callback) = self.on_select {
                                    callback(nid);
                                }
                            }
                        }
                        EventResult::Handled
                    }
                    _ => EventResult::Ignored,
                }
            }
            Event::Keyboard(kbd_event) if kbd_event.pressed => {
                let mods = ClickMods::new(kbd_event.modifiers.shift, kbd_event.modifiers.ctrl);
                match kbd_event.key_code {
                    KeyCode::Up => {
                        self.apply_arrow_vertical(ArrowDirection::Up, mods);
                        EventResult::Handled
                    }
                    KeyCode::Down => {
                        self.apply_arrow_vertical(ArrowDirection::Down, mods);
                        EventResult::Handled
                    }
                    KeyCode::Right => {
                        self.apply_arrow_right();
                        EventResult::Handled
                    }
                    KeyCode::Left => {
                        self.apply_arrow_left();
                        EventResult::Handled
                    }
                    KeyCode::Enter => {
                        self.fire_activate();
                        EventResult::Handled
                    }
                    _ => EventResult::Ignored,
                }
            }
            _ => EventResult::Ignored,
        }
    }
}
