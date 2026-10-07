//! Human/public table rows. Private seat transcripts are never decoded here.
use super::{ChatEntry, CustomPanelRow};
use serde_json::Value;

pub(super) fn entries(kind: &str, message: &Value) -> Vec<ChatEntry> {
    let event = &message["details"]["event"];
    let delivery = &event["delivery"];
    let label = match kind {
        "environment_notice" => "Environment notice".to_owned(),
        "environment_direct_message" => format!(
            "Message from {}",
            delivery["from"].as_str().unwrap_or("seat")
        ),
        "environment_observation" => "Environment observation".to_owned(),
        "environment_action" => "Environment action".to_owned(),
        "environment_status" => "Environment status".to_owned(),
        _ => return Vec::new(),
    };
    let value = if kind == "environment_observation" {
        &event["observation"]
    } else if kind == "environment_action" {
        &event["action"]
    } else if kind == "environment_status" && event.get("run").is_some() {
        &event["run"]
    } else {
        &delivery["message"]
    };
    let content = value.as_str().map_or_else(
        || serde_json::to_string_pretty(value).unwrap_or_default(),
        str::to_owned,
    );
    vec![ChatEntry::CustomPanel(Box::new(CustomPanelRow {
        custom_type: label,
        content,
    }))]
}

pub(crate) fn is_observation(row: &CustomPanelRow) -> bool {
    row.custom_type == "Environment observation"
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn public_rows_render_targeted_messages_and_filtered_observations() {
        let rows = entries(
            "environment_direct_message",
            &json!({"details":{"event":{"delivery":{"from":"alice","message":"Ready"}}}}),
        );
        let ChatEntry::CustomPanel(row) = &rows[0] else {
            panic!("expected table panel");
        };
        assert_eq!(
            (row.custom_type.as_str(), row.content.as_str()),
            ("Message from alice", "Ready")
        );
        let rows = entries(
            "environment_observation",
            &json!({"details":{"event":{"observation":{"your_hand":["visible-card"]}}}}),
        );
        let ChatEntry::CustomPanel(row) = &rows[0] else {
            panic!("expected observation panel");
        };
        assert!(is_observation(row));
        assert!(row.content.contains("visible-card"));
        assert!(entries("environment_wake", &json!({"content":"PRIVATE"})).is_empty());
    }
}
