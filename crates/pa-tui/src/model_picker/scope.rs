//! The picker's scope concern (TS the model selector's `scope` state):
//! the session's scoped keys against the catalog, the active-list swap
//! behind the scope key, the per-model membership toggle, and the selection
//! rule a scope change re-applies.

use super::{ModelPicker, ModelPickerAction};

/// The active list (TS `ModelScope`): the session's scoped entries or the
/// full catalog. The picker opens `Scoped` while the session holds scoped
/// models and the scope key swaps between them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ModelScope {
    All,
    Scoped,
}

impl ModelPicker {
    /// The scope row's active side (the render and the adoption event's
    /// `scoped` value).
    pub(crate) fn scoped_side(&self) -> bool {
        matches!(self.scope, ModelScope::Scoped)
    }

    /// Whether the session holds a scope (the scope row renders and the
    /// toggle only runs then; TS keys it off the session's list —
    /// `scopedModels.length > 0`, model-selector.ts:231/:253-255 — never
    /// off what the loaded catalog happens to resolve: an unresolvable
    /// snapshot still scopes, and a refresh that empties the positions
    /// never strands the scoped side).
    pub(crate) fn has_scoped_models(&self) -> bool {
        !self.scoped_models.is_empty()
    }

    /// Whether the model is in the session's scope (the row's `scoped` tag).
    pub(crate) fn is_scoped(&self, model: &pa_types::ai::Model) -> bool {
        let key = Self::model_key_provider(&model.provider, &model.id);
        self.scoped_models.contains(&key)
    }

    /// The membership key: add the highlighted model to the scope (at the
    /// end of the scope order) or remove it. The list stays on the side the
    /// user is looking at, except that removing the last scoped model drops
    /// the empty scoped side back to the catalog, and the highlight stays on
    /// the model it was on while it is still listed.
    pub(super) fn toggle_selected_model_scope(&mut self) -> ModelPickerAction {
        let Some(model) = self.selected_model() else {
            return ModelPickerAction::None;
        };
        let (provider, model_id) = (model.provider.clone(), model.id.clone());
        let key = Self::model_key_provider(&provider, &model_id);
        let scoped =
            if let Some(position) = self.scoped_models.iter().position(|entry| *entry == key) {
                self.scoped_models.remove(position);
                false
            } else {
                self.scoped_models.push(key.clone());
                true
            };
        if !self.has_scoped_models() {
            self.scope = ModelScope::All;
        }
        self.resolve_scope_positions();
        let query = self.search.value().to_string();
        self.filter_models(&query);
        if let Some(position) = self
            .filtered
            .iter()
            .position(|&index| self.key_at(index) == key)
        {
            self.selected = position;
        } else {
            self.selected = self.selected.min(self.filtered.len().saturating_sub(1));
        }
        ModelPickerAction::ModelScopeChanged {
            provider,
            model_id,
            scoped,
        }
    }

    /// The reorder keys: move the highlighted model one place earlier
    /// (`-1`) or later (`1`) in the scope order, which is the order Alt+M
    /// cycles in. It only runs on the scoped side with an empty filter, where
    /// the listed rows are the whole scope; the highlight follows the model,
    /// and the first and last rows stay put.
    pub(super) fn move_selected_scoped_model(&mut self, direction: isize) -> ModelPickerAction {
        if !self.scoped_side() || !self.search.value().is_empty() {
            return ModelPickerAction::None;
        }
        let Some(target) = self.selected.checked_add_signed(direction) else {
            return ModelPickerAction::None;
        };
        let (Some(&from), Some(&to)) =
            (self.filtered.get(self.selected), self.filtered.get(target))
        else {
            return ModelPickerAction::None;
        };
        let moved = self.key_at(from);
        let neighbor = self.key_at(to);
        let (Some(moved_at), Some(neighbor_at)) = (
            self.scoped_models.iter().position(|key| *key == moved),
            self.scoped_models.iter().position(|key| *key == neighbor),
        ) else {
            return ModelPickerAction::None;
        };
        self.scoped_models.swap(moved_at, neighbor_at);
        self.resolve_scope_positions();
        let query = self.search.value().to_string();
        self.filter_models(&query);
        self.selected = target;
        ModelPickerAction::ScopeReordered {
            keys: self.scoped_models.clone(),
        }
    }

    /// Re-map the scoped keys against the loaded catalog: an entry
    /// missing from the loaded catalog is not listed, and one a later
    /// refresh brings appears (the earlier map could only run against
    /// what had loaded).
    pub(crate) fn resolve_scope_positions(&mut self) {
        self.scoped_positions = self
            .scoped_models
            .iter()
            .filter_map(|key| {
                self.all_models
                    .iter()
                    .position(|model| Self::model_key_provider(&model.provider, &model.id) == *key)
            })
            .collect();
    }

    /// The active list as `all_models` positions (TS `activeModels`): the
    /// scoped entries in the session's scope order when scoped, else the
    /// whole catalog.
    pub(super) fn active_indices(&self) -> Vec<usize> {
        match self.scope {
            ModelScope::All => (0..self.all_models.len()).collect(),
            ModelScope::Scoped => self.scoped_positions.clone(),
        }
    }

    /// TS `loadModels`/`setScope`'s selection rule (:373-375, :569-570):
    /// the current model re-selects inside the filtered active list, else
    /// the top.
    pub(super) fn select_current_or_top(&mut self) {
        self.selected = self
            .filtered
            .iter()
            .position(|&index| self.is_current(&self.all_models[index]))
            .unwrap_or(0);
    }
}
