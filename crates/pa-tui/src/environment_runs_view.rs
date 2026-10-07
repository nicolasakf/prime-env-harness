//! The Environments dock group's panel: the environment runs the builder
//! session provisioned, one row per run rather than per seat. It is a
//! fork-only surface (no TS counterpart), rendered inline-picker style like
//! the `/heartbeats` and bash views and fed wire data only.

use pa_types::environments::{InstanceRun, OccupantKind, RunPhase};

use crate::keybindings::KeybindingsManager;
use crate::menu_panel::{
    hint_row, key_hint, menu_list_layout, menu_row, no_match_row, rule_row, scroll_row, MenuSegment,
};
use crate::theme::{Theme, ThemeColor};
use crate::width::truncate_line;
use crate::{Line, Span};

/// The preferred visible rows of the list.
const PREFERRED_VISIBLE: usize = 8;

/// Rows the panel reserves outside its runs: rule, title, blank, the
/// selected run's detail line, blank, hint, blank.
const FRAME_ROWS: usize = 7;

/// What the session does with one key the panel consumed.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum EnvironmentRunsAction {
    None,
    Close,
    /// Attach to the run: a spectator table for an agents-only run, the
    /// saved instance session for a run with a human seat.
    Open(InstanceRun),
    Stop(InstanceRun),
}

/// Whether every seat of the run is an agent, so the builder can only
/// spectate it (`/env watch` is available for agents-only runs).
pub(crate) fn is_agents_only(run: &InstanceRun) -> bool {
    run.instance.roster.as_ref().is_some_and(|roster| {
        roster
            .seats
            .iter()
            .all(|seat| seat.occupant == OccupantKind::Agent)
    })
}

/// The runs persisted with one of `builders` as their provisioning session.
/// A run keeps the builder that created it, so a run another session resumed
/// or stopped still belongs to its creator, and a run without a recorded
/// builder belongs to none.
pub(crate) fn runs_of_builders(runs: &[InstanceRun], builders: &[&str]) -> Vec<InstanceRun> {
    runs.iter()
        .filter(|run| {
            run.builder_session_id
                .as_deref()
                .is_some_and(|builder| builders.contains(&builder))
        })
        .cloned()
        .collect()
}

pub(crate) struct EnvironmentRunsView {
    runs: Vec<InstanceRun>,
    selected_id: Option<String>,
    error: Option<String>,
    viewport_rows: usize,
}

impl EnvironmentRunsView {
    pub(crate) fn new(runs: Vec<InstanceRun>, viewport_rows: usize) -> Self {
        let selected_id = runs.first().map(|run| run.instance.id.clone());
        Self {
            runs,
            selected_id,
            error: None,
            viewport_rows,
        }
    }

    /// Fold a refreshed run list in; the selection follows its run by id
    /// and falls back to the first row when that run left the list.
    pub(crate) fn apply_runs(&mut self, runs: Vec<InstanceRun>) {
        self.runs = runs;
        if !self
            .selected_id
            .as_ref()
            .is_some_and(|id| self.runs.iter().any(|run| &run.instance.id == id))
        {
            self.selected_id = self.runs.first().map(|run| run.instance.id.clone());
        }
    }

    pub(crate) fn set_error(&mut self, error: Option<String>) {
        self.error = error;
    }

    fn selected_index(&self) -> usize {
        self.selected_id
            .as_ref()
            .and_then(|id| self.runs.iter().position(|run| &run.instance.id == id))
            .unwrap_or(0)
    }

    fn selected_run(&self) -> Option<&InstanceRun> {
        self.runs.get(self.selected_index())
    }

    pub(crate) fn handle_key(
        &mut self,
        key: &str,
        kb: &KeybindingsManager,
    ) -> EnvironmentRunsAction {
        if key == "ctrl+c"
            || kb.matches(key, "tui.select.cancel")
            || kb.matches(key, "app.modal.back")
        {
            return EnvironmentRunsAction::Close;
        }
        let up = kb.matches(key, "tui.select.up");
        if up || kb.matches(key, "tui.select.down") {
            if !self.runs.is_empty() {
                let index = self.selected_index();
                let next = if up {
                    index.saturating_sub(1)
                } else {
                    (index + 1).min(self.runs.len() - 1)
                };
                self.selected_id = Some(self.runs[next].instance.id.clone());
            }
            return EnvironmentRunsAction::None;
        }
        if kb.matches(key, "tui.select.confirm") {
            return self
                .selected_run()
                .cloned()
                .map_or(EnvironmentRunsAction::None, EnvironmentRunsAction::Open);
        }
        if key == "x" {
            return self
                .selected_run()
                .cloned()
                .map_or(EnvironmentRunsAction::None, EnvironmentRunsAction::Stop);
        }
        EnvironmentRunsAction::None
    }

    pub(crate) fn render(&self, theme: &Theme, width: usize, kb: &KeybindingsManager) -> Vec<Line> {
        let noun = if self.runs.len() == 1 { "run" } else { "runs" };
        let title = vec![
            Span::raw("  "),
            theme.fg_span(ThemeColor::Text, "Environments".to_string()),
            theme.fg_span(ThemeColor::Muted, format!("  {} {noun}", self.runs.len())),
        ];
        let mut lines = vec![rule_row(theme, width), truncate_line(&title, width, "")];
        lines.push(Vec::new());
        if self.runs.is_empty() {
            lines.push(no_match_row(
                theme,
                width,
                "No running or paused environments",
            ));
        } else {
            let selected = self.selected_index();
            let visible = menu_list_layout(
                Some(self.viewport_rows),
                PREFERRED_VISIBLE,
                self.runs.len(),
                FRAME_ROWS,
                1,
            );
            let start = selected
                .saturating_sub(visible / 2)
                .min(self.runs.len().saturating_sub(visible));
            let end = (start + visible).min(self.runs.len());
            for (offset, run) in self.runs[start..end].iter().enumerate() {
                let color = match run.phase {
                    RunPhase::Running => ThemeColor::Success,
                    RunPhase::Paused => ThemeColor::Warning,
                    RunPhase::Stalled | RunPhase::NeedsRecovery => ThemeColor::Error,
                    RunPhase::Stopped | RunPhase::Finished => ThemeColor::Dim,
                };
                let primary = vec![
                    theme.fg_span(color, "\u{25cf} ".to_string()),
                    theme.fg_span(
                        ThemeColor::Text,
                        format!(
                            "{} \u{2014} {}",
                            run.instance.definition_id, run.instance.id
                        ),
                    ),
                ];
                let (humans, agents) = run.instance.roster.as_ref().map_or((0, 0), |roster| {
                    let humans = roster
                        .seats
                        .iter()
                        .filter(|seat| seat.occupant == OccupantKind::Human)
                        .count();
                    (humans, roster.seats.len() - humans)
                });
                let seats = [(humans, "human"), (agents, "agent")]
                    .into_iter()
                    .filter(|(count, _)| *count > 0)
                    .map(|(count, noun)| {
                        format!("{count} {noun}{}", if count == 1 { "" } else { "s" })
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                lines.push(menu_row(
                    theme,
                    width,
                    primary,
                    &[
                        MenuSegment::muted(&seats),
                        MenuSegment::themed(color, run.phase.label()),
                    ],
                    start + offset == selected,
                ));
            }
            if start > 0 || end < self.runs.len() {
                lines.push(scroll_row(theme, width, selected + 1, self.runs.len()));
            }
            if let Some(reason) = self.selected_run().and_then(|run| run.reason.as_deref()) {
                lines.push(no_match_row(theme, width, reason));
            }
        }
        if let Some(error) = &self.error {
            lines.push(vec![
                Span::raw("  "),
                theme.fg_span(
                    ThemeColor::Warning,
                    format!("Environment refresh failed: {error}"),
                ),
            ]);
        }
        lines.push(Vec::new());
        let hint = [
            key_hint(kb, &["tui.select.up", "tui.select.down"], "move"),
            key_hint(kb, &["tui.select.confirm"], "open"),
            Some("x stop".to_string()),
            key_hint(kb, &["app.modal.back", "tui.select.cancel"], "close"),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" \u{b7} ");
        lines.push(hint_row(theme, width, &hint));
        lines.push(Vec::new());
        lines.truncate(self.viewport_rows.max(1));
        lines
    }
}

#[cfg(test)]
mod tests;
