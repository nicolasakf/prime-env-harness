//! The agents view's Environments column: how many environment runs one
//! session provisioned, by phase. A run belongs to the session that
//! provisioned it (the builder identity each run persists), so the tally
//! matches a session's `activeSessionId` against the runs' builders.

use pa_types::environments::{InstanceRun, RunPhase};

use crate::theme::{Theme, ThemeColor};
use crate::Line;

/// The cell of a session that provisioned no live run.
const NO_RUNS: &str = "\u{2014}";

/// The live runs one session provisioned, by phase. Terminal runs
/// (finished, stopped, needing recovery) are not live and never count.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct EnvironmentTally {
    pub(crate) running: usize,
    pub(crate) paused: usize,
    pub(crate) stalled: usize,
}

impl EnvironmentTally {
    /// The tally of the runs `builder` provisioned.
    pub(crate) fn for_builder(runs: &[InstanceRun], builder: &str) -> Self {
        let mut tally = Self::default();
        for run in runs
            .iter()
            .filter(|run| run.builder_session_id.as_deref() == Some(builder))
        {
            match run.phase {
                RunPhase::Running => tally.running += 1,
                RunPhase::Paused => tally.paused += 1,
                RunPhase::Stalled => tally.stalled += 1,
                RunPhase::NeedsRecovery | RunPhase::Stopped | RunPhase::Finished => {}
            }
        }
        tally
    }

    /// The non-empty phases in display order, each as `"{count} {label}"`
    /// with the color its phase carries in the runs panel.
    fn segments(&self) -> Vec<(ThemeColor, String)> {
        [
            (ThemeColor::Success, self.running, RunPhase::Running),
            (ThemeColor::Warning, self.paused, RunPhase::Paused),
            (ThemeColor::Error, self.stalled, RunPhase::Stalled),
        ]
        .into_iter()
        .filter(|(_, count, _)| *count > 0)
        .map(|(color, count, phase)| (color, format!("{count} {}", phase.label())))
        .collect()
    }

    /// The cell text, e.g. `2 running, 1 paused`.
    pub(crate) fn text(&self) -> String {
        let segments = self.segments();
        if segments.is_empty() {
            return NO_RUNS.to_string();
        }
        segments
            .into_iter()
            .map(|(_, segment)| segment)
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// The cell with each phase in its own color and the empty cell dim.
    pub(crate) fn line(&self, theme: &Theme) -> Line {
        let segments = self.segments();
        if segments.is_empty() {
            return vec![theme.fg(ThemeColor::Dim, NO_RUNS)];
        }
        let mut line: Line = Vec::new();
        for (index, (color, segment)) in segments.into_iter().enumerate() {
            if index > 0 {
                line.push(theme.fg(ThemeColor::Dim, ", "));
            }
            line.push(theme.fg(color, segment));
        }
        line
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn run(builder: Option<&str>, phase: &str) -> InstanceRun {
        serde_json::from_value(json!({
            "instance": {
                "id": "i1", "definition_id": "d1", "version": "v1",
                "policy": { "backend": "local" },
            },
            "phase": phase,
            "reason": null,
            "builder_session_id": builder,
        }))
        .expect("run")
    }

    #[test]
    fn tally_counts_each_live_phase_of_the_builders_runs() {
        let runs = vec![
            run(Some("s1"), "running"),
            run(Some("s1"), "running"),
            run(Some("s1"), "paused"),
            run(Some("s1"), "stalled"),
            run(Some("s1"), "finished"),
            run(Some("s2"), "running"),
            run(None, "running"),
        ];
        let tally = EnvironmentTally::for_builder(&runs, "s1");
        assert_eq!(
            tally,
            EnvironmentTally {
                running: 2,
                paused: 1,
                stalled: 1,
            }
        );
        assert_eq!(tally.text(), "2 running, 1 paused, 1 stalled");
        assert_eq!(
            EnvironmentTally::for_builder(&runs, "s3").text(),
            "\u{2014}"
        );
    }
}
