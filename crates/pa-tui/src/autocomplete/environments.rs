//! Environment command discovery. Participant completion never searches host files.
use super::{
    apply_file_completion, get_slash_command_context, AutocompleteProvider, CompletionItem,
    CompletionResult, SlashKind, SuggestionKind, SuggestionLookup, Suggestions,
};
use pa_types::environments::{
    ActionDescriptor, Definition, Instance, InstanceRun, RoleName, RunPhase,
};
use serde_json::{json, Value};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    Builder,
    Participant,
}

struct Command {
    name: &'static str,
    arguments: &'static str,
    description: &'static str,
    example: &'static str,
    participant: bool,
}

const COMMANDS: &[Command] = &[
    Command { name: "help", arguments: "[command]", description: "Show command usage and examples", example: "help act", participant: true },
    Command { name: "act", arguments: "[action] [JSON]", description: "Choose an action; omit JSON for guided input", example: "act", participant: true },
    Command { name: "observe", arguments: "", description: "Show your current observation", example: "observe", participant: true },
    Command { name: "status", arguments: "", description: "Inspect the instance and available actions", example: "status", participant: true },
    Command { name: "leave", arguments: "", description: "Leave the environment and return to the builder", example: "leave", participant: true },
    Command { name: "cancel", arguments: "", description: "Cancel the current action input form", example: "cancel", participant: true },
    Command { name: "seats", arguments: "", description: "Show seat identities and roles", example: "seats", participant: true },
    Command { name: "msg", arguments: "<seat> <text>", description: "Send a private message to a seat", example: "msg player-2 ready", participant: true },
    Command { name: "pause", arguments: "[instance-id]", description: "Pause this table, or a running instance from the builder", example: "pause", participant: true },
    Command { name: "resume_run", arguments: "", description: "Resume a paused run", example: "resume_run", participant: true },
    Command { name: "stop", arguments: "[instance-id]", description: "Stop this table permanently, or an instance from the builder", example: "stop", participant: true },
    Command { name: "runs", arguments: "", description: "List persisted environment runs", example: "runs", participant: false },
    Command { name: "act_for", arguments: "<instance-id> <seat> <action> [JSON]", description: "Take a stuck agent seat's turn for it", example: "act_for instance-id player-2 pass {}", participant: false },
    Command { name: "suspend", arguments: "<instance-id> <seat> [reason]", description: "Take an agent seat out of play until resumed", example: "suspend instance-id player-2 keeps calling unavailable actions", participant: false },
    Command { name: "resume_seat", arguments: "<instance-id> <seat>", description: "Return a suspended seat to play", example: "resume_seat instance-id player-2", participant: false },
    Command { name: "watch", arguments: "<instance>", description: "Watch a public agents-only table", example: "watch instance-id", participant: false },
    Command { name: "view", arguments: "[public] | <instance>", description: "Open the environment's live viewer in your browser", example: "view", participant: true },
    Command { name: "list", arguments: "", description: "List saved environment IDs and names", example: "list", participant: false },
    Command { name: "catalog", arguments: "", description: "Show the detailed saved environment catalog", example: "catalog", participant: false },
    Command { name: "create", arguments: "<description>", description: "Build a new environment draft", example: "create a blackjack game where a human is a dealer and it plays against up to six AI models.", participant: false },
    Command { name: "inspect", arguments: "<id>", description: "Inspect a saved definition", example: "inspect cards", participant: false },
    Command { name: "policy", arguments: "<draft> <JSON>", description: "Select draft execution permissions", example: "policy /path/to/draft {\"backend\":\"local\"}", participant: false },
    Command { name: "validate", arguments: "<draft>", description: "Run the draft's behavioral checks", example: "validate /path/to/draft", participant: false },
    Command { name: "save", arguments: "<draft> <id>", description: "Save a validated definition", example: "save /path/to/draft cards", participant: false },
    Command { name: "enter", arguments: "[id] [local|docker|JSON]", description: "Choose a saved environment, then select an execution policy", example: "enter cards local", participant: false },
    Command { name: "resume", arguments: "[instance-id]", description: "Resume this table, or a saved instance from the builder", example: "resume", participant: true },
    Command { name: "recover", arguments: "<instance-id> <JSON>", description: "Reconcile an uncertain operation using recovery evidence", example: "recover instance-id {\"snapshot\":{},\"receipts\":{},\"python_receipts\":{},\"reset_python\":true,\"evidence\":\"verified outcome\"}", participant: false },
];

pub(crate) fn help(command: &str, mode: Mode) -> String {
    let rows: Vec<_> = COMMANDS
        .iter()
        .filter(|entry| {
            (mode == Mode::Builder || entry.participant)
                && (command.is_empty() || entry.name == command)
        })
        .map(|entry| {
            format!(
                "/env {} {} — {}\n  Example: /env {}",
                entry.name, entry.arguments, entry.description, entry.example
            )
        })
        .collect();
    if rows.is_empty() {
        return format!(
            "Unknown environment command: {command}. Use /env help to see available commands."
        );
    }
    format!(
        "{}\n↑/↓ choose · Tab/Enter fills the command · Enter submits · Escape closes",
        rows.join("\n")
    )
}

/// Examples are editable suggestions, never automatically submitted or executed.
pub(crate) fn example_input(schema: &Value, depth: usize) -> Value {
    if depth > 8 {
        return Value::Null;
    }
    if let Some(value) = schema
        .get("default")
        .or_else(|| {
            schema["examples"]
                .as_array()
                .and_then(|values| values.first())
        })
        .or_else(|| schema.get("const"))
        .or_else(|| schema["enum"].as_array().and_then(|values| values.first()))
    {
        return value.clone();
    }
    match schema["type"].as_str() {
        Some("string") => json!("example"),
        Some("boolean") => json!(false),
        Some("integer" | "number") => schema.get("minimum").cloned().unwrap_or(json!(0)),
        Some("array") => json!([]),
        Some("null") => Value::Null,
        _ => Value::Object(
            schema["properties"]
                .as_object()
                .map(|properties| {
                    properties
                        .iter()
                        .filter(|(name, _)| {
                            schema["required"]
                                .as_array()
                                .is_some_and(|required| required.contains(&json!(name)))
                        })
                        .map(|(name, field)| (name.clone(), example_input(field, depth + 1)))
                        .collect()
                })
                .unwrap_or_default(),
        ),
    }
}

pub(crate) fn instance_items(instances: &[Instance]) -> Vec<CompletionItem> {
    instances
        .iter()
        .map(|instance| {
            let created = instance
                .created_at
                .and_then(|time| i64::try_from(time).ok())
                .map_or_else(
                    || "time unavailable".into(),
                    |time| {
                        format!(
                            "{} UTC",
                            crate::incident_notices::format_incident_notice_time(time, 0)
                        )
                    },
                );
            CompletionItem {
                value: format!("resume {}", instance.id),
                label: format!(
                    "{} — {} — Created {created}",
                    instance.definition_id, instance.id
                ),
                description: Some(format!("Pinned version {}", instance.version)),
                argument_hint: None,
                source_tag: None,
            }
        })
        .collect()
}

/// Builder picker for `/env pause`, `/env stop`, and the seat interventions: only
/// runs the operation can still affect. A paused or stalled run is idle already and
/// can only be stopped, or have its seats intervened in.
pub(crate) fn run_items(runs: &[InstanceRun]) -> Vec<CompletionItem> {
    let describe = |run: &InstanceRun, operation: &str| CompletionItem {
        value: format!("{operation} {}", run.instance.id),
        label: format!("{} — {}", run.instance.definition_id, run.instance.id),
        description: Some(format!(
            "{}{}",
            run.phase.label(),
            run.reason
                .as_ref()
                .map_or(String::new(), |reason| format!(" · {reason}"))
        )),
        argument_hint: None,
        source_tag: None,
    };
    // Ended runs take no control.
    let pausable = runs.iter().filter(|run| run.phase.is_pausable());
    let stoppable = runs.iter().filter(|run| !run.phase.is_terminal());
    let open = stoppable
        .clone()
        .filter(|run| run.phase != RunPhase::NeedsRecovery);
    pausable
        .map(|run| describe(run, "pause"))
        .chain(stoppable.map(|run| describe(run, "stop")))
        .chain(open.flat_map(|run| {
            ["act_for", "suspend", "resume_seat"].map(|operation| describe(run, operation))
        }))
        .collect()
}

pub(crate) fn definition_items(definitions: &[Definition]) -> Vec<CompletionItem> {
    let entries: std::collections::BTreeSet<_> = definitions
        .iter()
        .map(|definition| (&definition.id, &definition.manifest.name))
        .collect();
    entries
        .into_iter()
        .map(|(id, name)| CompletionItem {
            value: format!("enter {id}"),
            label: format!("{id} — {name}"),
            description: Some("Start a new instance; select its execution policy next".into()),
            argument_hint: None,
            source_tag: None,
        })
        .collect()
}

pub(crate) fn action_items(actions: &[ActionDescriptor]) -> Vec<CompletionItem> {
    actions
        .iter()
        .filter(|action| action.participants.contains(&RoleName::legacy_human()))
        .map(|action| {
            let example = example_input(&action.input_schema, 0);
            CompletionItem {
                value: format!("act {} ", action.name),
                label: action.name.clone(),
                description: Some(format!(
                    "{} · Example: /env act {} {example}",
                    action.description, action.name
                )),
                argument_hint: Some("Enter for guided input".into()),
                source_tag: None,
            }
        })
        .collect()
}

pub(crate) fn action_help(action: &ActionDescriptor) -> String {
    let mut rows = vec![format!("{} — {}", action.name, action.description)];
    if let Some(properties) = action.input_schema["properties"].as_object() {
        for (name, schema) in properties {
            let required = action.input_schema["required"]
                .as_array()
                .is_some_and(|required| required.contains(&json!(name)));
            rows.push(format!(
                "  {name}: {} ({}){}{}{}",
                schema["type"].as_str().unwrap_or("JSON"),
                if required { "required" } else { "optional" },
                schema["description"]
                    .as_str()
                    .map_or(String::new(), |text| format!(" — {text}")),
                schema
                    .get("enum")
                    .map_or(String::new(), |choices| format!(" · choices: {choices}")),
                schema
                    .get("default")
                    .map_or(String::new(), |default| format!(" · default: {default}"))
            ));
        }
    } else {
        rows.push(format!("  Input schema: {}", action.input_schema));
    }
    rows.push(format!("  Example: /env act {} {}\n  Use /env act {} for guided input. Examples may need editing to satisfy the environment's rules.", action.name, example_input(&action.input_schema, 0), action.name));
    rows.join("\n")
}

pub(crate) fn suggestions(
    prefix: &str,
    completions: &[CompletionItem],
    mode: Mode,
) -> Option<SuggestionLookup> {
    let trimmed = prefix.trim_start();
    let (operation, rest) = trimmed
        .split_once(char::is_whitespace)
        .unwrap_or((trimmed, ""));
    let items: Vec<CompletionItem> =
        if (matches!(operation, "act" | "enter" | "resume" | "msg" | "watch")
            || (mode == Mode::Builder
                && matches!(
                    operation,
                    "pause" | "stop" | "act_for" | "suspend" | "resume_seat"
                )))
            && trimmed.contains(char::is_whitespace)
        {
            if rest.trim_start().contains(char::is_whitespace) {
                return None;
            }
            let term = format!("{operation} {}", rest.trim());
            completions
                .iter()
                .filter(|item| item.value.trim_end().starts_with(&term))
                .cloned()
                .collect()
        } else if operation == "help" && !rest.is_empty() || trimmed == "help " {
            COMMANDS
                .iter()
                .filter(|entry| {
                    (mode == Mode::Builder || entry.participant)
                        && entry.name.starts_with(rest.trim())
                })
                .map(|entry| CompletionItem {
                    value: format!("help {}", entry.name),
                    label: entry.name.into(),
                    description: Some(entry.description.into()),
                    argument_hint: Some(entry.arguments.into()),
                    source_tag: None,
                })
                .collect()
        } else if trimmed.contains(char::is_whitespace) {
            COMMANDS
                .iter()
                .filter(|entry| {
                    entry.name == operation
                        && (mode == Mode::Builder || entry.participant)
                        && rest.trim().is_empty()
                        && !entry.arguments.is_empty()
                })
                .map(|entry| CompletionItem {
                    value: entry.example.into(),
                    label: format!("Example: /env {}", entry.example),
                    description: Some(
                        "Tab/Enter copies this example for editing; submit separately.".into(),
                    ),
                    argument_hint: Some(entry.arguments.into()),
                    source_tag: None,
                })
                .collect()
        } else {
            COMMANDS
                .iter()
                .filter(|entry| {
                    (mode == Mode::Builder || entry.participant)
                        && entry.name.starts_with(operation)
                })
                .map(|entry| CompletionItem {
                    value: format!(
                        "{}{}",
                        entry.name,
                        if entry.arguments.is_empty() { "" } else { " " }
                    ),
                    label: entry.name.into(),
                    description: Some(format!(
                        "{} · Example: /env {}",
                        entry.description, entry.example
                    )),
                    argument_hint: Some(entry.arguments.into()),
                    source_tag: None,
                })
                .collect()
        };
    if items.is_empty() {
        return None;
    }
    Some(SuggestionLookup::Ready(Suggestions {
        prefix: prefix.into(),
        kind: Some(SuggestionKind::SlashCommand),
        items,
    }))
}

pub(crate) struct EnvironmentAutocompleteProvider {
    actions: Vec<CompletionItem>,
}

impl EnvironmentAutocompleteProvider {
    pub(crate) fn new(actions: &[ActionDescriptor]) -> Self {
        Self {
            actions: action_items(actions),
        }
    }
}

impl AutocompleteProvider for EnvironmentAutocompleteProvider {
    fn get_suggestions(
        &self,
        lines: &[String],
        cursor_line: usize,
        cursor_col: usize,
        _force: bool,
    ) -> Option<SuggestionLookup> {
        let context = get_slash_command_context(lines, cursor_line, cursor_col)?;
        if !context.at_prompt_start {
            return None;
        }
        match context.kind {
            SlashKind::Name => {
                let commands = [
                    ("/env ", "/env", "Environment commands and actions"),
                    ("/tutorial", "/tutorial", "Getting started with Prime Env"),
                    ("/id", "/id", "Copy the current session id to clipboard"),
                ];
                let items: Vec<CompletionItem> = commands
                    .into_iter()
                    .filter(|(_, label, _)| label.starts_with(&context.prefix))
                    .map(|(value, label, description)| CompletionItem {
                        value: value.into(),
                        label: label.into(),
                        description: Some(description.into()),
                        argument_hint: None,
                        source_tag: None,
                    })
                    .collect();
                if items.is_empty() {
                    None
                } else {
                    Some(SuggestionLookup::Ready(Suggestions {
                        prefix: context.prefix,
                        kind: Some(SuggestionKind::SlashCommand),
                        items,
                    }))
                }
            }
            SlashKind::Argument if context.command_name.as_deref() == Some("env") => {
                suggestions(&context.prefix, &self.actions, Mode::Participant)
            }
            SlashKind::Argument => None,
        }
    }

    fn apply_completion(
        &self,
        lines: &[String],
        cursor_line: usize,
        cursor_col: usize,
        item: &CompletionItem,
        prefix: &str,
    ) -> CompletionResult {
        apply_file_completion(lines, cursor_line, cursor_col, item, prefix)
    }

    fn should_trigger_file_completion(
        &self,
        lines: &[String],
        cursor_line: usize,
        cursor_col: usize,
    ) -> bool {
        get_slash_command_context(lines, cursor_line, cursor_col).is_some_and(|context| {
            context.at_prompt_start && context.command_name.as_deref() == Some("env")
        })
    }

    fn set_argument_completions(&mut self, command: &'static str, items: Vec<CompletionItem>) {
        if command == "env" {
            self.actions = items;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::autocomplete::CombinedAutocompleteProvider;
    use crate::editor::{Editor, EditorEvent};

    fn actions() -> Vec<ActionDescriptor> {
        vec![
            ActionDescriptor {
                name: "classify".into(),
                description: "Classify a document".into(),
                input_schema: json!({"type":"object","properties":{
                "category":{"type":"string","enum":["work","personal"],"description":"Document category"},
                "priority":{"type":"integer","default":1}},"required":["category"]}),
                output_schema: json!({}),
                participants: vec![RoleName::legacy_human()],
                roles: Vec::new(),
            },
            ActionDescriptor {
                name: "private_action".into(),
                description: "Agent-only action".into(),
                input_schema: json!({}),
                output_schema: json!({}),
                participants: vec![RoleName::legacy_agent()],
                roles: Vec::new(),
            },
        ]
    }

    fn lookup(provider: &dyn AutocompleteProvider, text: &str) -> Suggestions {
        match provider.get_suggestions(
            &[text.into()],
            0,
            text.chars().count(),
            /*force*/ false,
        ) {
            Some(SuggestionLookup::Ready(result)) => result,
            other => panic!("expected suggestions for {text:?}: {other:?}"),
        }
    }

    #[test]
    fn builder_completion_drills_into_usage_and_editable_examples() {
        let mut provider =
            CombinedAutocompleteProvider::from_registry(std::path::PathBuf::from("/"));
        provider.set_argument_completions("env", vec![]);
        let command = lookup(&provider, "/en")
            .items
            .into_iter()
            .find(|item| item.value == "env")
            .unwrap();
        assert_eq!(
            provider.apply_completion(&["/en".into()], 0, 3, &command, "/en"),
            CompletionResult {
                lines: vec!["/env ".into()],
                cursor_line: 0,
                cursor_col: 5,
            }
        );
        let choices = lookup(&provider, "/env cr");
        assert_eq!(choices.items.len(), 1);
        assert_eq!(
            choices.items[0].argument_hint.as_deref(),
            Some("<description>")
        );
        let applied = provider.apply_completion(
            &["/env cr".into()],
            0,
            7,
            &choices.items[0],
            &choices.prefix,
        );
        assert_eq!(
            applied,
            CompletionResult {
                lines: vec!["/env create ".into()],
                cursor_line: 0,
                cursor_col: 12
            }
        );
        let example = lookup(&provider, "/env create ");
        assert_eq!(
            example.items[0].value,
            "create a blackjack game where a human is a dealer and it plays against up to six AI models."
        );
        assert!(example.items[0]
            .description
            .as_deref()
            .unwrap()
            .contains("submit separately"));
    }

    #[test]
    fn list_completion_fills_a_command_that_can_be_submitted() {
        let mut editor = Editor::default();
        editor.set_autocomplete_argument_completions("env", vec![]);
        editor.set_text("/env li");
        editor.handle_input("tab");
        editor.materialize_autocomplete();
        editor.take_events();
        editor.handle_input("enter");
        assert_eq!(editor.get_text(), "/env list");
        assert!(!editor
            .take_events()
            .iter()
            .any(|event| matches!(event, EditorEvent::Submitted(_))));
        editor.handle_input("enter");
        assert!(editor
            .take_events()
            .iter()
            .any(|event| matches!(event, EditorEvent::Submitted(text) if text == "/env list")));
    }

    #[test]
    fn resume_space_lists_timestamped_sessions_and_filters_ids() {
        let instances: Vec<Instance> = serde_json::from_value(json!([
            {"id":"newest","definition_id":"cards","version":"v2","policy":{"backend":"local"},"created_at":1_759_392_000_000u64},
            {"id":"older","definition_id":"counter","version":"v1","policy":{"backend":"local"},"created_at":1_759_305_600_000u64}
        ])).unwrap();
        let items = instance_items(&instances);
        let mut provider =
            CombinedAutocompleteProvider::from_registry(std::path::PathBuf::from("/"));
        provider.set_argument_completions("env", items.clone());
        assert_eq!(lookup(&provider, "/env resume ").items, items);
        assert_eq!(
            lookup(&provider, "/env resume old").items,
            vec![items[1].clone()]
        );
        assert_eq!(items[0].label, "cards — newest — Created 25/10/2 08:00 UTC");
        assert_eq!(
            provider.apply_completion(&["/env resume ".into()], 0, 12, &items[0], "resume "),
            CompletionResult {
                lines: vec!["/env resume newest".into()],
                cursor_line: 0,
                cursor_col: 18
            }
        );
    }

    #[test]
    fn run_controls_list_only_the_runs_they_can_affect() {
        let runs: Vec<InstanceRun> = serde_json::from_value(json!([
            {"instance":{"id":"live","definition_id":"cards","version":"v1","policy":{"backend":"local"}},"phase":"running","reason":null},
            {"instance":{"id":"idle","definition_id":"counter","version":"v1","policy":{"backend":"local"}},"phase":"paused","reason":"human detached"},
            {"instance":{"id":"ended","definition_id":"counter","version":"v1","policy":{"backend":"local"}},"phase":"stopped","reason":"human requested stop"}
        ])).unwrap();
        let items = run_items(&runs);
        let mut provider =
            CombinedAutocompleteProvider::from_registry(std::path::PathBuf::from("/"));
        provider.set_argument_completions("env", items.clone());
        assert_eq!(
            lookup(&provider, "/env pause ").items,
            vec![items[0].clone()]
        );
        assert_eq!(
            lookup(&provider, "/env stop ").items,
            vec![items[1].clone(), items[2].clone()]
        );
        assert_eq!(
            lookup(&provider, "/env stop id").items,
            vec![items[2].clone()]
        );
        assert_eq!(
            items
                .iter()
                .map(|item| (
                    item.value.as_str(),
                    item.label.as_str(),
                    item.description.as_deref()
                ))
                .collect::<Vec<_>>(),
            vec![
                ("pause live", "cards — live", Some("running")),
                ("stop live", "cards — live", Some("running")),
                (
                    "stop idle",
                    "counter — idle",
                    Some("paused · human detached")
                ),
                ("act_for live", "cards — live", Some("running")),
                ("suspend live", "cards — live", Some("running")),
                ("resume_seat live", "cards — live", Some("running")),
                (
                    "act_for idle",
                    "counter — idle",
                    Some("paused · human detached")
                ),
                (
                    "suspend idle",
                    "counter — idle",
                    Some("paused · human detached")
                ),
                (
                    "resume_seat idle",
                    "counter — idle",
                    Some("paused · human detached")
                ),
            ]
        );
        assert_eq!(
            lookup(&provider, "/env suspend ").items,
            vec![items[4].clone(), items[7].clone()]
        );
    }

    #[test]
    fn enter_space_lists_saved_environments_and_filters_ids() {
        let cards: Definition = serde_json::from_value(json!({
            "id":"cards", "version":"v1",
            "manifest":{"protocol_version":1,"name":"Card game","purpose":"test","language":"python","instructions":"test"},
            "validation":{"content_hash":"v1","policy":{"backend":"local"},"passed":true,"diagnostics":[],"actions":[]}
        })).unwrap();
        let mut counter = cards.clone();
        counter.id = "counter".into();
        counter.manifest.name = "Counter".into();
        let mut other_version = cards.clone();
        other_version.version = "v2".into();
        let items = definition_items(&[counter, other_version, cards]);
        let mut provider =
            CombinedAutocompleteProvider::from_registry(std::path::PathBuf::from("/"));
        provider.set_argument_completions("env", items.clone());
        assert_eq!(lookup(&provider, "/env enter ").items, items);
        assert_eq!(
            lookup(&provider, "/env enter cou").items,
            vec![items[1].clone()]
        );
        assert_eq!(
            items
                .iter()
                .map(|item| (item.value.as_str(), item.label.as_str()))
                .collect::<Vec<_>>(),
            vec![
                ("enter cards", "cards — Card game"),
                ("enter counter", "counter — Counter"),
            ]
        );
    }

    #[test]
    fn instance_selection_fills_then_submits_the_selected_command() {
        for operation in ["enter", "resume", "pause", "stop"] {
            let command = format!("/env {operation} cards");
            let item = CompletionItem {
                value: format!("{operation} cards"),
                label: "cards — Card game".into(),
                description: None,
                argument_hint: None,
                source_tag: None,
            };
            let mut editor = Editor::default();
            editor.set_autocomplete_argument_completions("env", vec![item]);
            editor.set_text(&format!("/env {operation} "));
            editor.handle_input("tab");
            editor.materialize_autocomplete();
            editor.take_events();
            editor.handle_input("enter");
            assert_eq!(editor.get_text().trim(), command);
            assert!(!editor
                .take_events()
                .iter()
                .any(|event| matches!(event, EditorEvent::Submitted(_))));
            editor.handle_input("enter");
            assert!(editor.take_events().iter().any(
                |event| matches!(event, EditorEvent::Submitted(text) if text.trim() == command)
            ));
        }
    }

    #[test]
    fn participant_completes_id_without_submitting() {
        let provider = EnvironmentAutocompleteProvider::new(&actions());
        let choices = lookup(&provider, "/i");
        assert_eq!(choices.items.len(), 1);
        assert_eq!(choices.items[0].value, "/id");
        assert_eq!(
            provider.apply_completion(&["/i".into()], 0, 2, &choices.items[0], &choices.prefix),
            CompletionResult {
                lines: vec!["/id".into()],
                cursor_line: 0,
                cursor_col: 3,
            }
        );
    }

    #[test]
    fn participant_filters_actions_and_never_completes_host_files() {
        let provider = EnvironmentAutocompleteProvider::new(&actions());
        let choices = lookup(&provider, "/env act cl");
        assert_eq!(
            choices
                .items
                .iter()
                .map(|item| item.label.as_str())
                .collect::<Vec<_>>(),
            vec!["classify"]
        );
        assert!(choices.items[0]
            .description
            .as_deref()
            .unwrap()
            .contains(r#"{"category":"work"}"#));
        assert_eq!(lookup(&provider, "/env act ").items.len(), 1);
        assert!(lookup(&provider, "/env ")
            .items
            .iter()
            .all(|item| !item.value.starts_with("create")));
        for text in [
            "./",
            "@",
            "/model ",
            "/env create ",
            "/env act classify {",
            "mention /env",
        ] {
            for force in [false, true] {
                assert!(
                    provider
                        .get_suggestions(&[text.into()], 0, text.chars().count(), force)
                        .is_none(),
                    "{text:?}, force={force}"
                );
            }
        }
    }

    #[test]
    fn action_selection_fills_then_submits_on_a_separate_enter() {
        let mut editor = Editor::default();
        editor
            .set_autocomplete_provider(Box::new(EnvironmentAutocompleteProvider::new(&actions())));
        editor.set_text("/env ac");
        editor.handle_input("tab");
        editor.materialize_autocomplete();
        assert!(editor.is_showing_autocomplete());
        editor.handle_input("enter");
        assert_eq!(editor.get_text(), "/env act ");
        editor.materialize_autocomplete();
        assert!(editor.is_showing_autocomplete());
        editor.take_events();
        editor.handle_input("enter");
        assert_eq!(editor.get_text(), "/env act classify ");
        editor.materialize_autocomplete();
        assert!(!editor.is_showing_autocomplete());
        assert!(!editor
            .take_events()
            .iter()
            .any(|event| matches!(event, EditorEvent::Submitted(_))));
        editor.handle_input("enter");
        assert!(editor.take_events().iter().any(
            |event| matches!(event, EditorEvent::Submitted(text) if text == "/env act classify")
        ));
    }

    #[test]
    fn action_help_explains_schema_and_required_example_values() {
        let action = actions().remove(0);
        let text = action_help(&action);
        for expected in [
            "category: string (required)",
            "Document category",
            "priority: integer (optional)",
            "default: 1",
            r#"Example: /env act classify {"category":"work"}"#,
        ] {
            assert!(text.contains(expected), "missing {expected:?}: {text}");
        }
        assert_eq!(
            example_input(
                &json!({"type":"object","properties":{"nested":{"type":"object","properties":{"count":{"type":"integer","minimum":3}},"required":["count"]}},"required":["nested"]}),
                0
            ),
            json!({"nested":{"count":3}})
        );
    }
}
