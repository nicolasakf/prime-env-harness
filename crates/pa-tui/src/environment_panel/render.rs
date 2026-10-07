//! Bounded dock rendering and matching mouse hit targets.
use super::{label, Control, EnvironmentPanel, FieldKind, Intent};
use crate::menu_panel::menu_row;
use crate::theme::{Theme, ThemeColor};
use crate::width::{str_width, truncate_line};
use crate::{Line, Span};
use std::ops::Range;

pub(crate) struct RenderedPanel {
    pub(crate) rows: Vec<Line>,
    pub(crate) regions: Vec<(usize, Range<usize>, Control)>,
}

impl EnvironmentPanel {
    pub(crate) fn render(&mut self, theme: &Theme, width: usize, height: usize) -> RenderedPanel {
        let mut body: Vec<(Line, Control)> = Vec::new();
        let mut anchor = 0;
        let (title, description) = if let Some(form) = &mut self.form {
            for (index, field) in form.fields.iter_mut().enumerate() {
                let focused = form.focus == index;
                if focused {
                    anchor = body.len();
                }
                let value = if field.enabled {
                    match &field.kind {
                        FieldKind::Text | FieldKind::Number | FieldKind::Json => {
                            field.editor.get_text().replace('\n', " ↵ ")
                        }
                        FieldKind::Boolean => {
                            if field.value == true {
                                "[x] Yes".into()
                            } else {
                                "[ ] No".into()
                            }
                        }
                        FieldKind::Choice(_) => field.value.as_str().map_or_else(
                            || {
                                if field.value.is_null() {
                                    "Choose…".into()
                                } else {
                                    field.value.to_string()
                                }
                            },
                            label,
                        ),
                        FieldKind::Multiple(_) => field.value.to_string(),
                    }
                } else {
                    "omitted".into()
                };
                let suffix = if field.required {
                    ""
                } else if field.enabled {
                    " (optional · included)"
                } else {
                    " (optional)"
                };
                body.push((
                    menu_row(
                        theme,
                        width,
                        vec![Span::raw(format!("{}{suffix}: {value}", field.label))],
                        &[],
                        focused,
                    ),
                    Control::Field(index),
                ));
                if focused {
                    if let Some(help) = field.schema["description"].as_str() {
                        body.push((
                            vec![theme.fg_span(ThemeColor::Muted, format!("  {help}"))],
                            Control::Field(index),
                        ));
                    }
                    if !field.required {
                        body.push((
                            vec![Span::raw(if field.enabled {
                                "  [ Omit this field ]"
                            } else {
                                "  [ Include this field ]"
                            })],
                            Control::Include(index),
                        ));
                    }
                    if field.enabled {
                        match &field.kind {
                            FieldKind::Text | FieldKind::Number | FieldKind::Json => {
                                let rendered = crate::view::editor_surface::render(
                                    &mut field.editor,
                                    theme,
                                    width,
                                    8,
                                    None,
                                    None,
                                );
                                body.extend(
                                    rendered
                                        .rows
                                        .into_iter()
                                        .map(|row| (row, Control::Field(index))),
                                );
                            }
                            FieldKind::Choice(choices) | FieldKind::Multiple(choices) => {
                                let start = field.choice_cursor.saturating_sub(2);
                                let end = (start + 5).min(choices.len());
                                if start > 0 {
                                    body.push((
                                        vec![theme.fg_span(ThemeColor::Muted, "  ↑ more choices")],
                                        Control::Field(index),
                                    ));
                                }
                                for (choice, value) in
                                    choices.iter().enumerate().take(end).skip(start)
                                {
                                    let selected = if matches!(field.kind, FieldKind::Multiple(_)) {
                                        field
                                            .value
                                            .as_array()
                                            .is_some_and(|items| items.contains(value))
                                    } else {
                                        &field.value == value
                                    };
                                    let mark = if selected { "[x]" } else { "[ ]" };
                                    let text =
                                        value.as_str().map_or_else(|| value.to_string(), label);
                                    body.push((
                                        menu_row(
                                            theme,
                                            width,
                                            vec![Span::raw(format!("  {mark} {text}"))],
                                            &[],
                                            choice == field.choice_cursor,
                                        ),
                                        Control::Choice {
                                            field: index,
                                            choice,
                                        },
                                    ));
                                }
                                if end < choices.len() {
                                    body.push((
                                        vec![theme.fg_span(ThemeColor::Muted, "  ↓ more choices")],
                                        Control::Field(index),
                                    ));
                                }
                            }
                            FieldKind::Boolean => {}
                        }
                    }
                }
            }
            (form.title.clone(), form.description.clone())
        } else {
            for (index, action) in self.actions.iter().enumerate() {
                if index == self.selected {
                    anchor = body.len();
                }
                body.push((
                    menu_row(
                        theme,
                        width,
                        vec![Span::raw(format!("[ {} ]", label(&action.name)))],
                        &[],
                        index == self.selected,
                    ),
                    Control::Action(index),
                ));
            }
            if self.actions.is_empty() {
                body.push((
                    vec![theme.fg_span(
                        ThemeColor::Muted,
                        "  No human actions. Use /env observe or /env leave.",
                    )],
                    Control::Cancel,
                ));
            }
            (
                "Environment actions".into(),
                self.actions
                    .get(self.selected)
                    .map_or_else(String::new, |action| action.description.clone()),
            )
        };
        let mut rows = vec![
            vec![theme.fg_span(ThemeColor::Border, "─".repeat(width))],
            vec![Span::raw(format!("  {title}"))],
            truncate_line(
                &vec![theme.fg_span(ThemeColor::Muted, format!("  {description}"))],
                width,
                "…",
            ),
        ];
        let mut regions = Vec::new();
        let budget = height.saturating_sub(9).max(1);
        let start = anchor
            .saturating_sub(budget / 3)
            .min(body.len().saturating_sub(budget));
        let end = (start + budget).min(body.len());
        if start > 0 {
            rows.push(vec![theme.fg_span(ThemeColor::Muted, "  ↑ more fields")]);
        }
        for (line, control) in body.into_iter().take(end).skip(start) {
            regions.push((rows.len(), 0..width, control));
            rows.push(truncate_line(&line, width, "…"));
        }
        if let Some(form) = &self.form {
            if let Some(error) = &form.error {
                rows.push(truncate_line(
                    &vec![theme.fg_span(ThemeColor::Error, format!("  {error}"))],
                    width,
                    "…",
                ));
            }
            let submit = if self.pending {
                "Working…"
            } else {
                match &form.intent {
                    Intent::Act(_) => &form.title,
                    Intent::Enter { .. } => "Start environment",
                }
            };
            let buttons = [
                (format!("[ {submit} ]"), Control::Submit),
                ("[ Cancel ]".into(), Control::Cancel),
            ];
            // Stack buttons at narrow widths, keeping both visible and clickable.
            for (text, control) in buttons {
                let focused = if control == Control::Submit {
                    form.focus == form.fields.len()
                } else {
                    form.focus == form.fields.len() + 1
                };
                regions.push((
                    rows.len(),
                    0..str_width(&text).saturating_add(4).min(width),
                    control,
                ));
                rows.push(menu_row(theme, width, vec![Span::raw(text)], &[], focused));
            }
            if matches!(form.intent, Intent::Act(_)) {
                regions.push((rows.len(), 0..width, Control::Back));
                rows.push(vec![theme.fg_span(ThemeColor::Muted, "  [ Other actions ]")]);
            }
        } else {
            regions.push((rows.len(), 0..width, Control::Cancel));
            rows.push(vec![theme.fg_span(ThemeColor::Muted, "  [ Close ]")]);
        }
        let hint = if self.form.is_some() {
            "Tab fields · ↑/↓ choices/numbers · Space toggle · Enter next/submit · Shift+Enter newline"
        } else {
            "↑/↓ choose · Enter open"
        };
        rows.push(truncate_line(
            &vec![theme.fg_span(
                ThemeColor::Dim,
                format!("  {} close · {hint}", self.kb.key_text("tui.select.cancel")),
            )],
            width,
            "…",
        ));
        RenderedPanel { rows, regions }
    }
}
