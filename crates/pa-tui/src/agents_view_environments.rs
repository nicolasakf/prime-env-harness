//! The agents view's Environments column: how many environment runs one
//! session provisioned, in total and by phase. A run belongs to the session
//! that provisioned it (the builder identity each run persists), so the tally
//! matches a session's saved `sessionId` against the runs' saved builders,
//! and its `activeSessionId` against older runs that recorded only that.

use pa_types::environments::{InstanceRun, RunPhase};
use serde_json::Value;

use crate::theme::{Theme, ThemeColor};
use crate::Line;

/// The cell of a session that provisioned no run.
const NO_RUNS: &str = "\u{2014}";

/// The runs one session provisioned, by phase, live and ended alike.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct EnvironmentTally {
    pub(crate) running: usize,
    pub(crate) paused: usize,
    pub(crate) stalled: usize,
    pub(crate) needs_recovery: usize,
    pub(crate) stopped: usize,
    pub(crate) finished: usize,
}

impl EnvironmentTally {
    /// The tally of the runs the session with roster `summary` provisioned.
    pub(crate) fn for_session(runs: &[InstanceRun], summary: &Value) -> Self {
        let id = |key: &str| summary.get(key).and_then(Value::as_str);
        let (saved, active) = (id("sessionId"), id("activeSessionId"));
        let mut tally = Self::default();
        // The saved session id decides when the run recorded one; only
        // older runs fall back to the active id.
        for run in runs
            .iter()
            .filter(|run| match &run.builder_saved_session_id {
                Some(builder) => saved == Some(builder.as_str()),
                None => active.is_some() && run.builder_session_id.as_deref() == active,
            })
        {
            match run.phase {
                RunPhase::Running => tally.running += 1,
                RunPhase::Paused => tally.paused += 1,
                RunPhase::Stalled => tally.stalled += 1,
                RunPhase::NeedsRecovery => tally.needs_recovery += 1,
                RunPhase::Stopped => tally.stopped += 1,
                RunPhase::Finished => tally.finished += 1,
            }
        }
        tally
    }

    fn total(&self) -> usize {
        self.running
            + self.paused
            + self.stalled
            + self.needs_recovery
            + self.stopped
            + self.finished
    }

    /// The non-empty phases in display order, live before ended, each as
    /// `"{count} {label}"` with the color its phase carries in the runs
    /// panel.
    fn segments(&self) -> Vec<(ThemeColor, String)> {
        [
            (ThemeColor::Success, self.running, RunPhase::Running),
            (ThemeColor::Warning, self.paused, RunPhase::Paused),
            (ThemeColor::Error, self.stalled, RunPhase::Stalled),
            (
                ThemeColor::Error,
                self.needs_recovery,
                RunPhase::NeedsRecovery,
            ),
            (ThemeColor::Dim, self.stopped, RunPhase::Stopped),
            (ThemeColor::Dim, self.finished, RunPhase::Finished),
        ]
        .into_iter()
        .filter(|(_, count, _)| *count > 0)
        .map(|(color, count, phase)| (color, format!("{count} {}", phase.label())))
        .collect()
    }

    /// The cell text, e.g. `3 (2 running, 1 stopped)`.
    pub(crate) fn text(&self) -> String {
        let segments = self.segments();
        if segments.is_empty() {
            return NO_RUNS.to_string();
        }
        let phases = segments
            .into_iter()
            .map(|(_, segment)| segment)
            .collect::<Vec<_>>()
            .join(", ");
        format!("{} ({phases})", self.total())
    }

    /// The cell with each phase in its own color and the empty cell dim.
    pub(crate) fn line(&self, theme: &Theme) -> Line {
        let segments = self.segments();
        if segments.is_empty() {
            return vec![theme.fg(ThemeColor::Dim, NO_RUNS)];
        }
        let mut line: Line = vec![
            theme.fg(ThemeColor::Text, self.total().to_string()),
            theme.fg(ThemeColor::Dim, " ("),
        ];
        for (index, (color, segment)) in segments.into_iter().enumerate() {
            if index > 0 {
                line.push(theme.fg(ThemeColor::Dim, ", "));
            }
            line.push(theme.fg(color, segment));
        }
        line.push(theme.fg(ThemeColor::Dim, ")"));
        line
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn run(builder: Option<&str>, saved: Option<&str>, phase: &str) -> InstanceRun {
        serde_json::from_value(json!({
            "instance": {
                "id": "i1", "definition_id": "d1", "version": "v1",
                "policy": { "backend": "local" },
            },
            "phase": phase,
            "reason": null,
            "builder_session_id": builder,
            "builder_saved_session_id": saved,
        }))
        .expect("run")
    }

    /// A session's runs match on its saved session id, which survives a
    /// restart, or for older runs on its current active id.
    #[test]
    fn tally_counts_every_phase_of_the_sessions_runs() {
        let runs = vec![
            run(Some("a1"), None, "running"),
            run(Some("old"), Some("s1"), "running"),
            run(Some("old"), Some("s1"), "paused"),
            run(Some("old"), Some("s1"), "stalled"),
            run(Some("old"), Some("s1"), "stopped"),
            run(Some("old"), Some("s1"), "finished"),
            run(Some("old"), Some("s2"), "running"),
            run(Some("other"), None, "running"),
            run(None, None, "running"),
        ];
        let tally = EnvironmentTally::for_session(
            &runs,
            &json!({"sessionId": "s1", "activeSessionId": "a1"}),
        );
        assert_eq!(
            tally,
            EnvironmentTally {
                running: 2,
                paused: 1,
                stalled: 1,
                needs_recovery: 0,
                stopped: 1,
                finished: 1,
            }
        );
        assert_eq!(
            tally.text(),
            "6 (2 running, 1 paused, 1 stalled, 1 stopped, 1 finished)"
        );
        assert_eq!(
            EnvironmentTally::for_session(&runs, &json!({"sessionId": "s2"})).text(),
            "1 (1 running)"
        );
        assert_eq!(
            EnvironmentTally::for_session(&runs, &json!({"sessionId": "s3"})).text(),
            "\u{2014}"
        );
    }
}
