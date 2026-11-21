#![forbid(unsafe_code)]
//! Composable TUI component traits, registry, and `SelectList`.

use std::collections::BTreeMap;
use std::sync::Arc;

use ratatui::text::Line;
use serde_json::Value;

/// Placement slot for custom dock widgets in the bottom TUI dock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DockSlot {
    /// Rendered immediately above the input editor.
    AboveEditor,
    /// Rendered between the input editor and the bottom footer.
    BelowEditor,
}

/// Composable transcript card or inspector panel component.
pub trait TuiComponent: Send + Sync {
    /// Unique slot identifier (matching a `UIRendererSpec::id` or `UIPanelSpec::id`).
    fn id(&self) -> &str;

    /// Render styled lines for the given available `width` and JSON `data` payload.
    fn render_lines(&self, width: u16, data: &Value) -> Vec<Line<'static>>;
}

/// Composable dock widget rendered above or below the TUI editor.
pub trait TuiDockWidget: Send + Sync {
    /// Unique widget identifier.
    fn id(&self) -> &str;

    /// Target dock placement slot.
    fn slot(&self) -> DockSlot;

    /// Render styled lines for the given available `width`.
    fn render_lines(&self, width: u16) -> Vec<Line<'static>>;
}

/// Registry allowing Rust builders to override or extend default `UISurface`
/// renderers, panels, and dock widgets without modifying `gestalt-std`.
#[derive(Default, Clone)]
pub struct TuiComponentRegistry {
    /// Custom transcript card renderers keyed by renderer ID (e.g., `"diff_preview"`).
    pub renderers: BTreeMap<String, Arc<dyn TuiComponent>>,
    /// Custom inspector overlay panels keyed by panel ID (e.g., `"policy_gate"`).
    pub panels: BTreeMap<String, Arc<dyn TuiComponent>>,
    /// Additional dock widgets rendered above or below the editor.
    pub dock_widgets: Vec<Arc<dyn TuiDockWidget>>,
}

impl TuiComponentRegistry {
    /// Register or override a transcript card renderer.
    pub fn register_renderer(&mut self, component: Arc<dyn TuiComponent>) {
        self.renderers.insert(component.id().to_string(), component);
    }

    /// Register or override an inspector overlay panel.
    pub fn register_panel(&mut self, component: Arc<dyn TuiComponent>) {
        self.panels.insert(component.id().to_string(), component);
    }

    /// Append a custom dock widget.
    pub fn register_dock_widget(&mut self, widget: Arc<dyn TuiDockWidget>) {
        self.dock_widgets.push(widget);
    }
}

/// Single selectable entry inside [`SelectListState`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectListItem {
    /// Inserted command or option value (e.g., `"/mode"`).
    pub value: String,
    /// Primary display label.
    pub label: String,
    /// Optional short description shown alongside the label.
    pub description: String,
}

/// Filterable, scrollable list state for command autocomplete and selection menus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectListState {
    items: Vec<SelectListItem>,
    filtered_indices: Vec<usize>,
    selected_idx: usize,
    max_visible: usize,
}

impl SelectListState {
    /// Construct a [`SelectListState`] with `items` and `max_visible` window size.
    #[must_use]
    pub fn new(items: Vec<SelectListItem>, max_visible: usize) -> Self {
        let filtered_indices = (0..items.len()).collect();
        Self {
            items,
            filtered_indices,
            selected_idx: 0,
            max_visible: max_visible.max(1),
        }
    }

    /// Filter items by case-insensitive prefix or substring match on `value`/`label`.
    pub fn set_filter(&mut self, filter: &str) {
        let needle = filter.trim().to_ascii_lowercase();
        self.filtered_indices = self
            .items
            .iter()
            .enumerate()
            .filter(|(_, item)| {
                needle.is_empty()
                    || item.value.to_ascii_lowercase().starts_with(&needle)
                    || item.label.to_ascii_lowercase().contains(&needle)
            })
            .map(|(i, _)| i)
            .collect();
        self.selected_idx = 0;
    }

    /// Return `true` if no items match the current filter.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.filtered_indices.is_empty()
    }

    /// Return the currently selected item, if any.
    #[must_use]
    pub fn selected_item(&self) -> Option<&SelectListItem> {
        let raw_idx = *self.filtered_indices.get(self.selected_idx)?;
        self.items.get(raw_idx)
    }

    /// Return the selected index within the filtered list.
    #[must_use]
    pub fn selected_index(&self) -> usize {
        self.selected_idx
    }

    /// Select a specific filtered index (clamped).
    pub fn select(&mut self, idx: usize) {
        if !self.filtered_indices.is_empty() {
            self.selected_idx = idx.min(self.filtered_indices.len() - 1);
        }
    }

    /// Move selection up by 1 (wrapping at top).
    pub fn move_up(&mut self) {
        if self.filtered_indices.is_empty() {
            return;
        }
        if self.selected_idx == 0 {
            self.selected_idx = self.filtered_indices.len() - 1;
        } else {
            self.selected_idx -= 1;
        }
    }

    /// Move selection down by 1 (wrapping at bottom).
    pub fn move_down(&mut self) {
        if self.filtered_indices.is_empty() {
            return;
        }
        self.selected_idx = (self.selected_idx + 1) % self.filtered_indices.len();
    }

    /// Return visible `(filtered_idx, &SelectListItem)` slice centered around selection.
    #[must_use]
    pub fn visible_items(&self) -> Vec<(usize, &SelectListItem)> {
        let total = self.filtered_indices.len();
        if total == 0 {
            return Vec::new();
        }
        let window = self.max_visible.min(total);
        let half = window / 2;
        let start = if self.selected_idx <= half {
            0
        } else if self.selected_idx + (window - half) >= total {
            total.saturating_sub(window)
        } else {
            self.selected_idx - half
        };
        let end = (start + window).min(total);
        (start..end)
            .filter_map(|f_idx| {
                let raw = self.filtered_indices[f_idx];
                self.items.get(raw).map(|item| (f_idx, item))
            })
            .collect()
    }
}
