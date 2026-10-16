//! The read-only transcript of an ended environment run, opened from the
//! Environments runs panel. The run's saved conversation replaces the chat
//! and this banner holds the editor's place, so nothing can be sent to the
//! run or done with it: the transcript scrolls, its cards expand, and the
//! close keys return to the builder session. It is a fork-only surface (no
//! TS counterpart) fed wire data only.

use pa_types::environments::{InstanceRun, RunPhase};

use crate::keybindings::KeybindingsManager;
use crate::menu_panel::{hint_row, key_hint, no_match_row, rule_row};
use crate::theme::{Theme, ThemeColor};
use crate::width::truncate_line;
use crate::{Line, Span};

/// What the session does with one key the transcript view consumed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EnvironmentRunTranscriptAction {
    /// Consumed without effect: the run takes no input.
    None,
    Close,
    ScrollLines(isize),
    PageUp,
    PageDown,
    Top,
    Follow,
    CycleDetail,
}

/// Whether the run's saved conversation has replaced the chat yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TranscriptState {
    /// The fetch is in flight; the chat still shows the builder session.
    Loading,
    Shown,
}

pub(crate) struct EnvironmentRunTranscript {
    run: InstanceRun,
    state: TranscriptState,
}

impl EnvironmentRunTranscript {
    /// The view of `run` while its transcript loads.
    pub(crate) fn new(run: InstanceRun) -> Self {
        Self {
            run,
            state: TranscriptState::Loading,
        }
    }

    pub(crate) fn run(&self) -> &InstanceRun {
        &self.run
    }

    /// The run's conversation now fills the chat.
    pub(crate) fn mark_shown(&mut self) {
        self.state = TranscriptState::Shown;
    }

    pub(crate) fn is_shown(&self) -> bool {
        self.state == TranscriptState::Shown
    }

    /// The action for one key: the close keys always act; the rest act on
    /// the run's transcript once it is shown, never on the builder's chat
    /// still showing beneath a loading view.
    pub(crate) fn handle_key(
        &self,
        key: &str,
        kb: &KeybindingsManager,
    ) -> EnvironmentRunTranscriptAction {
        if key == "ctrl+c"
            || kb.matches(key, "tui.select.cancel")
            || kb.matches(key, "app.modal.back")
        {
            EnvironmentRunTranscriptAction::Close
        } else if !self.is_shown() {
            EnvironmentRunTranscriptAction::None
        } else if kb.matches(key, "tui.viewport.pageUp") {
            EnvironmentRunTranscriptAction::PageUp
        } else if kb.matches(key, "tui.viewport.pageDown") {
            EnvironmentRunTranscriptAction::PageDown
        } else if kb.matches(key, "tui.viewport.top") {
            EnvironmentRunTranscriptAction::Top
        } else if kb.matches(key, "tui.viewport.follow") {
            EnvironmentRunTranscriptAction::Follow
        } else if kb.matches(key, "tui.select.up") {
            EnvironmentRunTranscriptAction::ScrollLines(-1)
        } else if kb.matches(key, "tui.select.down") {
            EnvironmentRunTranscriptAction::ScrollLines(1)
        } else if kb.matches(key, "app.tools.expand") {
            EnvironmentRunTranscriptAction::CycleDetail
        } else {
            EnvironmentRunTranscriptAction::None
        }
    }

    pub(crate) fn render(&self, theme: &Theme, width: usize, kb: &KeybindingsManager) -> Vec<Line> {
        let color = match self.run.phase {
            RunPhase::Running => ThemeColor::Success,
            RunPhase::Paused => ThemeColor::Warning,
            RunPhase::Stalled | RunPhase::NeedsRecovery => ThemeColor::Error,
            RunPhase::Stopped | RunPhase::Finished => ThemeColor::Dim,
        };
        let title = vec![
            Span::raw("  "),
            theme.fg_span(ThemeColor::Text, "Transcript".to_string()),
            theme.fg_span(
                ThemeColor::Muted,
                format!(
                    "  {} \u{2014} {}  ",
                    self.run.instance.definition_id, self.run.instance.id
                ),
            ),
            theme.fg_span(color, self.run.phase.label().to_string()),
        ];
        let mut lines = vec![rule_row(theme, width), truncate_line(&title, width, "")];
        if let Some(reason) = self.run.reason.as_deref() {
            lines.push(no_match_row(theme, width, reason));
        }
        lines.push(Vec::new());
        lines.push(no_match_row(
            theme,
            width,
            match self.state {
                TranscriptState::Loading => "Loading the transcript\u{2026}",
                TranscriptState::Shown => {
                    "Read-only: this run has ended, so it takes no messages or actions."
                }
            },
        ));
        lines.push(Vec::new());
        let hint = [
            key_hint(
                kb,
                &["tui.viewport.pageUp", "tui.viewport.pageDown"],
                "scroll",
            ),
            key_hint(kb, &["app.tools.expand"], "detail"),
            key_hint(kb, &["app.modal.back", "tui.select.cancel"], "back"),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" \u{b7} ");
        lines.push(hint_row(theme, width, &hint));
        lines.push(Vec::new());
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::ColorMode;
    use serde_json::json;

    fn stopped_run() -> InstanceRun {
        serde_json::from_value(json!({
            "instance": {
                "id": "i1", "definition_id": "d1", "version": "v1",
                "policy": { "backend": "local" },
            },
            "phase": "stopped",
            "reason": "human requested stop",
        }))
        .expect("run")
    }

    /// The transcript scrolls and closes; every other key, typing
    /// included, does nothing. While it loads, only the close keys act.
    #[test]
    fn only_scrolling_and_closing_keys_act() {
        let mut transcript = EnvironmentRunTranscript::new(stopped_run());
        let kb = KeybindingsManager::new();
        let keys = ["up", "down", "pageUp", "pageDown", "a", "enter", "escape"];
        let actions = |transcript: &EnvironmentRunTranscript| -> Vec<_> {
            keys.into_iter()
                .map(|key| transcript.handle_key(key, &kb))
                .collect()
        };
        let mut loading = vec![EnvironmentRunTranscriptAction::None; keys.len() - 1];
        loading.push(EnvironmentRunTranscriptAction::Close);
        assert_eq!(actions(&transcript), loading);
        transcript.mark_shown();
        let actions = actions(&transcript);
        assert_eq!(
            actions,
            vec![
                EnvironmentRunTranscriptAction::ScrollLines(-1),
                EnvironmentRunTranscriptAction::ScrollLines(1),
                EnvironmentRunTranscriptAction::PageUp,
                EnvironmentRunTranscriptAction::PageDown,
                EnvironmentRunTranscriptAction::None,
                EnvironmentRunTranscriptAction::None,
                EnvironmentRunTranscriptAction::Close,
            ]
        );
    }

    /// The banner names the run, its phase and why it ended, says it is
    /// loading until the transcript lands, and then that the run is
    /// read-only.
    #[test]
    fn the_banner_names_the_run_and_says_it_is_read_only() {
        let mut transcript = EnvironmentRunTranscript::new(stopped_run());
        let theme = Theme::builtin("prime", ColorMode::TrueColor);
        let kb = KeybindingsManager::new();
        let text = |transcript: &EnvironmentRunTranscript| -> Vec<String> {
            transcript
                .render(&theme, 100, &kb)
                .iter()
                .map(|line| line.iter().map(|span| span.content.as_str()).collect())
                .collect()
        };
        let loading = text(&transcript);
        assert!(
            loading[1].contains("Transcript  d1 \u{2014} i1  stopped"),
            "{loading:?}"
        );
        assert!(loading[2].contains("human requested stop"), "{loading:?}");
        assert!(
            loading.iter().any(|row| row.contains("Loading")),
            "{loading:?}"
        );
        transcript.mark_shown();
        let shown = text(&transcript);
        assert!(
            shown.iter().any(|row| row.contains("Read-only")),
            "{shown:?}"
        );
    }
}
