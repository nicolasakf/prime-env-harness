use super::*;

fn panel(schema: &Value) -> EnvironmentPanel {
    let actions: Vec<ActionDescriptor> = serde_json::from_value(json!([{"name":"act","description":"test","input_schema":schema,"output_schema":{},"participants":["human"]}])).unwrap();
    let mut panel = EnvironmentPanel::actions(&actions, &KeybindingsManager::new());
    panel.select_action("act").unwrap();
    panel
}

fn submitted(panel: &mut EnvironmentPanel) -> Value {
    match panel.activate(Control::Submit) {
        PanelOutcome::Submit { arguments, .. } => arguments,
        PanelOutcome::None | PanelOutcome::Cancel => panic!("form did not submit"),
    }
}

#[test]
fn keyboard_and_clicks_serialize_typed_values_without_submitting_on_field_enter() {
    let mut panel = panel(
        &json!({"type":"object","properties":{"message":{"type":"string"},"quantity":{"type":"integer","default":2},"enabled":{"type":"boolean","default":false},"choice":{"enum":[4,7]},"cards":{"type":"array","uniqueItems":true,"items":{"enum":["a","b"]}}},"required":["message","quantity","enabled","choice","cards"]}),
    );
    let names: Vec<_> = panel
        .form
        .as_ref()
        .unwrap()
        .fields
        .iter()
        .map(|field| field.name.clone().unwrap())
        .collect();
    let index = |name: &str| names.iter().position(|value| value == name).unwrap();
    panel.activate(Control::Field(index("message")));
    panel.paste("/a path\nwith text");
    assert!(matches!(panel.handle_key("enter"), PanelOutcome::None));
    assert!(!panel.pending);
    panel.activate(Control::Field(index("quantity")));
    panel.handle_key("up");
    panel.activate(Control::Field(index("enabled")));
    panel.activate(Control::Choice {
        field: index("choice"),
        choice: 1,
    });
    panel.activate(Control::Choice {
        field: index("cards"),
        choice: 1,
    });
    assert_eq!(
        submitted(&mut panel),
        json!({"message":"/a path\nwith text","quantity":3,"enabled":true,"choice":7,"cards":["b"]})
    );
    assert!(matches!(
        panel.activate(Control::Submit),
        PanelOutcome::None
    ));
}

#[test]
fn invalid_bounds_preserve_draft_and_optional_false_zero_and_empty_are_distinct_from_omission() {
    let mut panel = panel(
        &json!({"type":"object","properties":{"a":{"type":"integer","minimum":1},"empty":{"type":"string","default":""},"flag":{"type":"boolean","default":false},"omit":{"type":"integer"},"zero":{"type":"integer","default":0}},"required":["a"]}),
    );
    panel.paste("0");
    assert!(matches!(
        panel.activate(Control::Submit),
        PanelOutcome::None
    ));
    let form = panel.form.as_ref().unwrap();
    assert_eq!(form.fields[0].editor.get_text(), "0");
    assert!(form.error.as_deref().unwrap().contains("minimum"));
    panel.handle_key("up");
    assert_eq!(
        submitted(&mut panel),
        json!({"a":1,"empty":"","flag":false,"zero":0})
    );
}

#[test]
fn setup_backend_cannot_collide_with_environment_parameters_and_choices_require_selection() {
    let definition: pa_types::environments::Definition = serde_json::from_value(json!({"id":"cards","version":"v1","manifest":{"protocol_version":1,"name":"Cards","purpose":"test","instructions":"test","language":"python","setup_schema":{"type":"object","properties":{"backend":{"type":"string","default":"game"},"cards":{"type":"integer","default":5}},"required":["cards"]}},"validation":{"content_hash":"v1","passed":true,"policy":{},"diagnostics":[],"actions":[]}})).unwrap();
    let mut panel = EnvironmentPanel::setup(&definition, None, &KeybindingsManager::new());
    assert!(matches!(
        panel.activate(Control::Submit),
        PanelOutcome::None
    ));
    panel.activate(Control::Choice {
        field: 0,
        choice: 0,
    });
    assert_eq!(
        submitted(&mut panel),
        json!({"parameters":{"backend":"game","cards":5},"backend":"local"})
    );
}

#[test]
fn complex_field_falls_back_to_json_without_forcing_siblings_into_json() {
    let mut panel = panel(
        &json!({"type":"object","properties":{"nested":{"type":"object"},"text":{"type":"string"}},"required":["nested","text"]}),
    );
    panel.handle_key("ctrl+u");
    panel.paste("{\"a\":[1,2]}");
    panel.handle_key("tab");
    panel.paste("hello");
    assert_eq!(
        submitted(&mut panel),
        json!({"nested":{"a":[1,2]},"text":"hello"})
    );
}

#[test]
fn narrow_frame_keeps_focused_field_and_mouse_submit_target_visible() {
    let schema = json!({"type":"object","properties":(0..20).map(|index| (format!("field_{index:02}"), json!({"type":"string","default":""}))).collect::<Map<_,_>>()});
    let mut panel = panel(&schema);
    panel.activate(Control::Field(19));
    let theme = crate::theme::Theme::builtin("prime", crate::theme::ColorMode::TrueColor);
    let rendered = panel.render(&theme, 32, 18);
    assert!(rendered.rows.len() <= 18);
    assert!(rendered
        .rows
        .iter()
        .any(|row| row.iter().any(|span| span.content.contains("Field 19"))));
    assert!(rendered
        .regions
        .iter()
        .any(|(row, cols, control)| *row < rendered.rows.len()
            && cols.end <= 32
            && *control == Control::Submit));
}
