//! Private environment action chooser and reusable schema form.
mod render;

use crate::editor::Editor;
use crate::keybindings::KeybindingsManager;
use pa_types::environments::{ActionDescriptor, RoleName};
use serde_json::{json, Map, Value};

#[derive(Clone)]
pub(crate) enum Intent {
    Act(String),
    Enter {
        definition: String,
        policy: Option<Value>,
    },
}

impl Intent {
    pub(crate) fn feature(&self) -> &'static str {
        match self {
            Self::Act(_) => "environment_action_form",
            Self::Enter { .. } => "environment_setup_form",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Control {
    Action(usize),
    Field(usize),
    Choice { field: usize, choice: usize },
    Include(usize),
    Submit,
    Cancel,
    Back,
}

pub(crate) enum PanelOutcome {
    None,
    Cancel,
    Submit { intent: Intent, arguments: Value },
}

enum FieldKind {
    Text,
    Number,
    Boolean,
    Choice(Vec<Value>),
    Multiple(Vec<Value>),
    Json,
}

struct Field {
    name: Option<String>,
    label: String,
    schema: Value,
    required: bool,
    enabled: bool,
    kind: FieldKind,
    editor: Editor,
    value: Value,
    choice_cursor: usize,
}

impl Field {
    fn new(name: Option<String>, schema: Value, required: bool, kb: &KeybindingsManager) -> Self {
        let label = schema["title"].as_str().map_or_else(
            || name.as_deref().map_or_else(|| "Input (JSON)".into(), label),
            str::to_owned,
        );
        let kind = if let Some(choices) = schema["enum"].as_array().filter(|choices| {
            !choices.is_empty()
                && choices
                    .iter()
                    .all(|value| !value.is_object() && !value.is_array())
        }) {
            FieldKind::Choice(choices.clone())
        } else {
            match schema["type"].as_str() {
                Some("string") => FieldKind::Text,
                Some("integer" | "number") => FieldKind::Number,
                Some("boolean") => FieldKind::Boolean,
                Some("array")
                    if schema["uniqueItems"] == true && schema["items"]["enum"].is_array() =>
                {
                    FieldKind::Multiple(
                        schema["items"]["enum"]
                            .as_array()
                            .cloned()
                            .unwrap_or_default(),
                    )
                }
                Some(_) | None => FieldKind::Json,
            }
        };
        let default = schema.get("default").cloned();
        let value = default.clone().unwrap_or_else(|| match &kind {
            FieldKind::Boolean => json!(false),
            FieldKind::Multiple(_) => json!([]),
            FieldKind::Json if schema["type"] == "object" => json!({}),
            FieldKind::Json if schema["type"] == "array" => json!([]),
            FieldKind::Text | FieldKind::Number | FieldKind::Choice(_) | FieldKind::Json => {
                Value::Null
            }
        });
        let mut editor = Editor::new();
        editor.clear_autocomplete_provider();
        editor.set_keybindings(kb.clone());
        let text = match &kind {
            FieldKind::Text => value.as_str().unwrap_or("").to_owned(),
            FieldKind::Number if value.is_null() => String::new(),
            FieldKind::Number | FieldKind::Json => value.to_string(),
            FieldKind::Boolean | FieldKind::Choice(_) | FieldKind::Multiple(_) => String::new(),
        };
        editor.set_text(&text);
        Self {
            name,
            label,
            schema,
            required,
            enabled: required || default.is_some(),
            kind,
            editor,
            value,
            choice_cursor: 0,
        }
    }

    fn parsed(&self) -> anyhow::Result<Value> {
        let value = match &self.kind {
            FieldKind::Text => json!(self.editor.get_text()),
            FieldKind::Number | FieldKind::Json => serde_json::from_str(&self.editor.get_text())
                .map_err(|error| anyhow::anyhow!("{}: {error}", self.label))?,
            FieldKind::Boolean | FieldKind::Choice(_) | FieldKind::Multiple(_) => {
                self.value.clone()
            }
        };
        match &self.kind {
            FieldKind::Number => {
                let number = value
                    .as_f64()
                    .ok_or_else(|| anyhow::anyhow!("{}: enter a number", self.label))?;
                if self.schema["type"] == "integer" && !(value.is_i64() || value.is_u64()) {
                    anyhow::bail!("{}: enter a whole number", self.label);
                }
                for (bound, lower) in [
                    ("minimum", true),
                    ("maximum", false),
                    ("exclusiveMinimum", true),
                    ("exclusiveMaximum", false),
                ] {
                    if let Some(limit) = self.schema[bound].as_f64().filter(|limit| {
                        if lower {
                            if bound.starts_with("exclusive") {
                                number <= *limit
                            } else {
                                number < *limit
                            }
                        } else {
                            if bound.starts_with("exclusive") {
                                number >= *limit
                            } else {
                                number > *limit
                            }
                        }
                    }) {
                        anyhow::bail!("{}: {bound} is {limit}", self.label);
                    }
                }
            }
            FieldKind::Choice(choices) if !choices.contains(&value) => {
                anyhow::bail!("{}: choose a value", self.label)
            }
            FieldKind::Text | FieldKind::Multiple(_) => {
                let length = match &value {
                    Value::String(text) => text.chars().count(),
                    Value::Array(items) => items.len(),
                    Value::Null | Value::Bool(_) | Value::Number(_) | Value::Object(_) => 0,
                };
                let (minimum, maximum) = if value.is_array() {
                    ("minItems", "maxItems")
                } else {
                    ("minLength", "maxLength")
                };
                if self.schema[minimum]
                    .as_u64()
                    .is_some_and(|limit| (length as u64) < limit)
                    || self.schema[maximum]
                        .as_u64()
                        .is_some_and(|limit| (length as u64) > limit)
                {
                    anyhow::bail!(
                        "{}: length must satisfy {minimum} {} and {maximum} {}",
                        self.label,
                        self.schema[minimum],
                        self.schema[maximum]
                    );
                }
            }
            FieldKind::Boolean | FieldKind::Choice(_) | FieldKind::Json => {}
        }
        Ok(value)
    }

    fn choose(&mut self, choice: usize) {
        match &self.kind {
            FieldKind::Choice(choices) => {
                if let Some(value) = choices.get(choice) {
                    self.value = value.clone();
                }
            }
            FieldKind::Multiple(choices) => {
                if let (Some(value), Some(selected)) =
                    (choices.get(choice), self.value.as_array_mut())
                {
                    if let Some(index) = selected.iter().position(|item| item == value) {
                        selected.remove(index);
                    } else {
                        selected.push(value.clone());
                    }
                }
            }
            FieldKind::Text | FieldKind::Number | FieldKind::Boolean | FieldKind::Json => {}
        }
        self.enabled = true;
        self.choice_cursor = choice;
    }
}

struct Form {
    intent: Intent,
    title: String,
    description: String,
    fields: Vec<Field>,
    focus: usize,
    error: Option<String>,
}

impl Form {
    fn new(
        intent: Intent,
        title: String,
        description: String,
        schema: &Value,
        kb: &KeybindingsManager,
    ) -> Self {
        let fields = if let Some(properties) = schema["properties"].as_object().filter(|_| {
            schema["type"] == "object"
                && !["oneOf", "anyOf", "allOf", "$ref"]
                    .iter()
                    .any(|key| schema.get(key).is_some())
        }) {
            let required = schema["required"].as_array();
            let mut fields: Vec<_> = properties
                .iter()
                .map(|(name, property)| {
                    Field::new(
                        Some(name.clone()),
                        property.clone(),
                        required.is_some_and(|names| names.contains(&json!(name))),
                        kb,
                    )
                })
                .collect();
            fields.sort_by_key(|field| !field.required);
            fields
        } else if schema["type"] == "object"
            && schema.get("properties").is_none()
            && schema["additionalProperties"] == false
        {
            Vec::new()
        } else {
            vec![Field::new(None, schema.clone(), /*required*/ true, kb)]
        };
        Self {
            intent,
            title,
            description,
            fields,
            focus: 0,
            error: None,
        }
    }

    fn arguments(&mut self) -> anyhow::Result<Value> {
        let mut values = Map::new();
        let setup = matches!(self.intent, Intent::Enter { .. });
        let needs_backend = matches!(self.intent, Intent::Enter { policy: None, .. });
        let mut backend = Value::Null;
        for (index, field) in self
            .fields
            .iter()
            .enumerate()
            .filter(|(_, field)| field.enabled)
        {
            let value = match field.parsed() {
                Ok(value) => value,
                Err(error) => {
                    self.focus = index;
                    return Err(error);
                }
            };
            if needs_backend && index == 0 {
                backend = value;
            } else if let Some(name) = &field.name {
                values.insert(name.clone(), value);
            } else {
                return Ok(if setup {
                    json!({"parameters":value,"backend":backend})
                } else {
                    value
                });
            }
        }
        Ok(if setup {
            json!({"parameters":values,"backend":backend})
        } else {
            Value::Object(values)
        })
    }
}

pub(crate) struct EnvironmentPanel {
    actions: Vec<ActionDescriptor>,
    selected: usize,
    form: Option<Form>,
    pub(crate) pending: bool,
    pub(crate) uncertain: bool,
    /// Version of the environment state this panel was opened over; its submissions
    /// are bound to it, so a state that moved on is refused, never silently adopted.
    pub(crate) view_version: Option<u64>,
    kb: KeybindingsManager,
}

impl EnvironmentPanel {
    pub(crate) fn actions(actions: &[ActionDescriptor], kb: &KeybindingsManager) -> Self {
        Self {
            actions: actions
                .iter()
                .filter(|action| action.participants.contains(&RoleName::legacy_human()))
                .cloned()
                .collect(),
            selected: 0,
            form: None,
            pending: false,
            uncertain: false,
            view_version: None,
            kb: kb.clone(),
        }
    }

    pub(crate) fn setup(
        definition: &pa_types::environments::Definition,
        policy: Option<Value>,
        kb: &KeybindingsManager,
    ) -> Self {
        let schema = definition.manifest.setup_schema.clone().unwrap_or_else(
            || json!({"type":"object","properties":{},"additionalProperties":false}),
        );
        let needs_backend = policy.is_none();
        let mut form = Form::new(
            Intent::Enter {
                definition: definition.id.clone(),
                policy,
            },
            format!("Set up {}", definition.manifest.name),
            definition.manifest.purpose.clone(),
            &schema,
            kb,
        );
        if needs_backend {
            form.fields.insert(0, Field::new(None, json!({"title":"Run environment","enum":["local","docker"],"description":"Local runs with host-user permissions. Docker is optional."}), /*required*/ true, kb));
        }
        let mut panel = Self::actions(&[], kb);
        panel.form = Some(form);
        panel
    }

    pub(crate) fn select_action(&mut self, name: &str) -> anyhow::Result<()> {
        let index = self
            .actions
            .iter()
            .position(|action| action.name == name)
            .ok_or_else(|| anyhow::anyhow!("Unknown human action: {name}"))?;
        let _ = self.activate(Control::Action(index));
        Ok(())
    }

    pub(crate) fn feature(&self) -> &'static str {
        self.form
            .as_ref()
            .map_or("environment_action_form", |form| form.intent.feature())
    }

    pub(crate) fn fail(&mut self, error: String) {
        self.pending = false;
        if let Some(form) = &mut self.form {
            form.error = Some(error);
        }
    }

    pub(crate) fn paste(&mut self, text: &str) {
        if let Some(field) = self
            .form
            .as_mut()
            .filter(|_| !self.pending)
            .and_then(|form| form.fields.get_mut(form.focus))
            .filter(|field| {
                matches!(
                    field.kind,
                    FieldKind::Text | FieldKind::Number | FieldKind::Json
                )
            })
        {
            field.enabled = true;
            let _ = field.editor.handle_paste(text);
        }
    }

    pub(crate) fn handle_key(&mut self, id: &str) -> PanelOutcome {
        if self.kb.matches(id, "tui.select.cancel") || id == "ctrl+c" {
            return PanelOutcome::Cancel;
        }
        if self.pending {
            return PanelOutcome::None;
        }
        let Some(form) = &mut self.form else {
            if self.kb.matches(id, "tui.select.up") {
                self.selected = self.selected.saturating_sub(1);
            } else if self.kb.matches(id, "tui.select.down") {
                self.selected = (self.selected + 1).min(self.actions.len().saturating_sub(1));
            } else if self.kb.matches(id, "tui.select.confirm") {
                return self.activate(Control::Action(self.selected));
            }
            return PanelOutcome::None;
        };
        if id == "tab" {
            form.focus = (form.focus + 1) % (form.fields.len() + 2);
        } else if id == "shift+tab" {
            form.focus = (form.focus + form.fields.len() + 1) % (form.fields.len() + 2);
        } else if self.kb.matches(id, "tui.select.confirm") {
            if form.focus == form.fields.len() {
                return self.activate(Control::Submit);
            }
            if form.focus == form.fields.len() + 1 {
                return PanelOutcome::Cancel;
            }
            form.focus += 1;
        } else if let Some(field) = form.fields.get_mut(form.focus) {
            match &field.kind {
                FieldKind::Boolean if id == "space" || id == " " => {
                    field.value = json!(!field.value.as_bool().unwrap_or(false));
                    field.enabled = true;
                }
                FieldKind::Choice(choices) | FieldKind::Multiple(choices)
                    if matches!(id, "left" | "right" | "up" | "down") =>
                {
                    let count = choices.len();
                    if count > 0 {
                        field.choice_cursor = if id == "left" || id == "up" {
                            (field.choice_cursor + count - 1) % count
                        } else {
                            (field.choice_cursor + 1) % count
                        };
                        if matches!(field.kind, FieldKind::Choice(_)) {
                            field.choose(field.choice_cursor);
                        }
                    }
                }
                FieldKind::Choice(_) | FieldKind::Multiple(_) if id == "space" || id == " " => {
                    field.choose(field.choice_cursor);
                }
                FieldKind::Number if matches!(id, "up" | "down") => {
                    let number = field.editor.get_text().parse::<f64>().unwrap_or(0.0);
                    let step = field.schema["multipleOf"].as_f64().unwrap_or(1.0);
                    field
                        .editor
                        .set_text(&(number + if id == "up" { step } else { -step }).to_string());
                    field.enabled = true;
                }
                FieldKind::Text | FieldKind::Number | FieldKind::Json => {
                    if id == "shift+enter" || id == "alt+enter" {
                        if !matches!(field.kind, FieldKind::Number) {
                            field.editor.insert_text_at_cursor("\n");
                        }
                    } else {
                        field.editor.handle_input(id);
                    }
                    field.enabled = true;
                }
                FieldKind::Boolean | FieldKind::Choice(_) | FieldKind::Multiple(_) => {}
            }
        }
        PanelOutcome::None
    }

    pub(crate) fn activate(&mut self, control: Control) -> PanelOutcome {
        if control == Control::Cancel {
            return PanelOutcome::Cancel;
        }
        if self.pending {
            return PanelOutcome::None;
        }
        match control {
            Control::Action(index) => {
                if let Some(action) = self.actions.get(index) {
                    self.selected = index;
                    self.form = Some(Form::new(
                        Intent::Act(action.name.clone()),
                        label(&action.name),
                        action.description.clone(),
                        &action.input_schema,
                        &self.kb,
                    ));
                }
            }
            Control::Back => {
                self.form = None;
            }
            Control::Field(index) => {
                if let Some(form) = &mut self.form {
                    form.focus = index;
                    if let Some(field) = form
                        .fields
                        .get_mut(index)
                        .filter(|field| matches!(field.kind, FieldKind::Boolean))
                    {
                        field.value = json!(!field.value.as_bool().unwrap_or(false));
                        field.enabled = true;
                    }
                }
            }
            Control::Include(index) => {
                if let Some(form) = &mut self.form {
                    if let Some(field) = form.fields.get_mut(index).filter(|field| !field.required)
                    {
                        field.enabled = !field.enabled;
                    }
                    form.focus = index;
                }
            }
            Control::Choice { field, choice } => {
                if let Some(form) = &mut self.form {
                    if let Some(input) = form.fields.get_mut(field) {
                        input.choose(choice);
                    }
                    form.focus = field;
                }
            }
            Control::Submit => {
                if self.uncertain {
                    return PanelOutcome::None;
                }
                if let Some(form) = &mut self.form {
                    match form.arguments() {
                        Ok(arguments) => {
                            self.pending = true;
                            return PanelOutcome::Submit {
                                intent: form.intent.clone(),
                                arguments,
                            };
                        }
                        Err(error) => {
                            form.error = Some(error.to_string());
                        }
                    }
                }
            }
            Control::Cancel => return PanelOutcome::Cancel,
        }
        PanelOutcome::None
    }
}

fn label(name: &str) -> String {
    let text = name.replace('_', " ");
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().collect::<String>() + chars.as_str()
    })
}

#[cfg(test)]
mod tests;
