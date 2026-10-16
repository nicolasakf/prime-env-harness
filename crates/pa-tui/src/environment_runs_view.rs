//! The Environments dock group's panel: the environment runs the builder
//! session provisioned, one row per run rather than per seat, then an
//! "Other runs" group for runs no session can be matched to. In each group
//! live runs come first; ended runs (stopped, finished, needing recovery)
//! follow and open only as a read-only transcript. It is a fork-only surface
//! (no TS counterpart), rendered inline-picker style like the `/heartbeats`
//! and bash views and fed wire data only.

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
    /// Show an ended run's transcript, read-only.
    View(InstanceRun),
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

/// The sessions whose runs count as the panel's own: the builder session
/// and its subagents, by saved session id and by current active id.
pub(crate) struct RunOwners<'a> {
    pub(crate) saved_session_ids: Vec<&'a str>,
    pub(crate) active_session_ids: Vec<&'a str>,
}

/// The runs the panel lists, split by whether they can be matched to the
/// builder session.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct ScopedRuns {
    /// The runs the builder session or one of its subagents provisioned.
    pub(crate) own: Vec<InstanceRun>,
    /// The runs no session can be matched to: they were saved before runs
    /// recorded their builder's saved session id, and the active id they
    /// recorded instead changes whenever that session reopens.
    pub(crate) other: Vec<InstanceRun>,
}

impl ScopedRuns {
    /// Split `runs` for `owners`. A run keeps the builder that created it,
    /// so a run another session resumed or stopped still belongs to its
    /// creator; a run whose saved builder is some other session is left
    /// out.
    pub(crate) fn of(runs: &[InstanceRun], owners: &RunOwners<'_>) -> Self {
        let mut scoped = Self::default();
        for run in runs {
            // The saved session id decides when the run recorded one; only
            // older runs fall back to the active id.
            match run.builder_saved_session_id.as_deref() {
                Some(saved) if owners.saved_session_ids.contains(&saved) => {
                    scoped.own.push(run.clone());
                }
                Some(_) => {}
                None if run
                    .builder_session_id
                    .as_deref()
                    .is_some_and(|active| owners.active_session_ids.contains(&active)) =>
                {
                    scoped.own.push(run.clone());
                }
                None => scoped.other.push(run.clone()),
            }
        }
        scoped
    }
}

/// Live runs first, then the ended ones, each group keeping its order.
fn live_first(mut runs: Vec<InstanceRun>) -> Vec<InstanceRun> {
    runs.sort_by_key(|run| run.phase.is_terminal());
    runs
}

/// One row of the run list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ListRow {
    /// The session has no runs of its own, above the other runs.
    NoOwnRuns,
    /// The heading of the other runs.
    OtherRuns,
    /// The run at this index of `runs`.
    Run(usize),
}

pub(crate) struct EnvironmentRunsView {
    /// The session's own runs, then the other runs, each live first.
    runs: Vec<InstanceRun>,
    /// How many of `runs` are the session's own.
    own: usize,
    selected_id: Option<String>,
    error: Option<String>,
    viewport_rows: usize,
}

impl EnvironmentRunsView {
    pub(crate) fn new(runs: ScopedRuns, viewport_rows: usize) -> Self {
        let mut view = Self {
            runs: Vec::new(),
            own: 0,
            selected_id: None,
            error: None,
            viewport_rows,
        };
        view.apply_runs(runs);
        view
    }

    /// Fold a refreshed run list in; the selection follows its run by id
    /// and falls back to the first row when that run left the list.
    pub(crate) fn apply_runs(&mut self, runs: ScopedRuns) {
        self.own = runs.own.len();
        self.runs = live_first(runs.own);
        self.runs.extend(live_first(runs.other));
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

    /// Put the selection on the run `id`, when it is listed.
    pub(crate) fn select_run(&mut self, id: &str) {
        if self.runs.iter().any(|run| run.instance.id == id) {
            self.selected_id = Some(id.to_owned());
        }
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

    /// The list's rows: the own runs, then the heading and the other runs.
    fn list_rows(&self) -> Vec<ListRow> {
        let mut rows = Vec::new();
        if self.own == 0 && !self.runs.is_empty() {
            rows.push(ListRow::NoOwnRuns);
        }
        rows.extend((0..self.own).map(ListRow::Run));
        if self.own < self.runs.len() {
            rows.push(ListRow::OtherRuns);
            rows.extend((self.own..self.runs.len()).map(ListRow::Run));
        }
        rows
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
            return match self.selected_run().cloned() {
                Some(run) if run.phase.is_terminal() => EnvironmentRunsAction::View(run),
                Some(run) => EnvironmentRunsAction::Open(run),
                None => EnvironmentRunsAction::None,
            };
        }
        // An ended run has nothing left to stop.
        if key == "x" {
            return self
                .selected_run()
                .filter(|run| !run.phase.is_terminal())
                .cloned()
                .map_or(EnvironmentRunsAction::None, EnvironmentRunsAction::Stop);
        }
        EnvironmentRunsAction::None
    }

    pub(crate) fn render(&self, theme: &Theme, width: usize, kb: &KeybindingsManager) -> Vec<Line> {
        let noun = if self.own == 1 { "run" } else { "runs" };
        let title = vec![
            Span::raw("  "),
            theme.fg_span(ThemeColor::Text, "Environments".to_string()),
            theme.fg_span(ThemeColor::Muted, format!("  {} {noun}", self.own)),
        ];
        let mut lines = vec![rule_row(theme, width), truncate_line(&title, width, "")];
        lines.push(Vec::new());
        if self.runs.is_empty() {
            lines.push(no_match_row(theme, width, "No environment runs"));
        } else {
            let selected = self.selected_index();
            let rows = self.list_rows();
            let selected_row = rows
                .iter()
                .position(|row| *row == ListRow::Run(selected))
                .unwrap_or(0);
            let visible = menu_list_layout(
                Some(self.viewport_rows),
                PREFERRED_VISIBLE,
                rows.len(),
                FRAME_ROWS,
                1,
            );
            let start = selected_row
                .saturating_sub(visible / 2)
                .min(rows.len().saturating_sub(visible));
            let end = (start + visible).min(rows.len());
            for row in &rows[start..end] {
                let index = match *row {
                    ListRow::NoOwnRuns => {
                        lines.push(no_match_row(theme, width, "No runs from this session"));
                        continue;
                    }
                    ListRow::OtherRuns => {
                        let heading = vec![
                            Span::raw("  "),
                            theme.fg_span(ThemeColor::Text, "Other runs".to_string()),
                            theme.fg_span(
                                ThemeColor::Muted,
                                format!(
                                    "  {} \u{b7} not matched to a session",
                                    self.runs.len() - self.own
                                ),
                            ),
                        ];
                        lines.push(truncate_line(&heading, width, ""));
                        continue;
                    }
                    ListRow::Run(index) => index,
                };
                let run = &self.runs[index];
                let color = match run.phase {
                    RunPhase::Running => ThemeColor::Success,
                    RunPhase::Paused => ThemeColor::Warning,
                    RunPhase::Stalled | RunPhase::NeedsRecovery => ThemeColor::Error,
                    RunPhase::Stopped | RunPhase::Finished => ThemeColor::Dim,
                };
                let name_color = if run.phase.is_terminal() {
                    ThemeColor::Muted
                } else {
                    ThemeColor::Text
                };
                let primary = vec![
                    theme.fg_span(color, "\u{25cf} ".to_string()),
                    theme.fg_span(
                        name_color,
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
                    index == selected,
                ));
            }
            if start > 0 || end < rows.len() {
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
        let ended = self
            .selected_run()
            .is_some_and(|run| run.phase.is_terminal());
        let hint = [
            key_hint(kb, &["tui.select.up", "tui.select.down"], "move"),
            key_hint(
                kb,
                &["tui.select.confirm"],
                if ended { "view transcript" } else { "open" },
            ),
            (!ended).then(|| "x stop".to_string()),
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
