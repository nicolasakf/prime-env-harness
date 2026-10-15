//! Pinned sessions: the pin key, the pinned block above the status
//! sections, and the empty-session carve-out.

use super::*;
use crate::agents_view_state::DisplaySection;
use crate::client_settings::ClientSettings;
use std::sync::{Arc, Mutex};

/// The settings seam the agents view sees: only the pins are read.
struct PinStore {
    pinned: Mutex<Vec<String>>,
    fail: bool,
}

impl PinStore {
    fn new(pinned: &[&str]) -> Arc<Self> {
        Arc::new(Self {
            pinned: Mutex::new(pinned.iter().map(ToString::to_string).collect()),
            fail: false,
        })
    }
}

impl ClientSettings for PinStore {
    fn pinned_session_ids(&self) -> Vec<String> {
        self.pinned.lock().unwrap().clone()
    }
    fn toggle_session_pinned(&self, session_id: &str) -> anyhow::Result<bool> {
        if self.fail {
            anyhow::bail!("the settings file is read-only");
        }
        let mut pinned = self.pinned.lock().unwrap();
        let was_pinned = pinned.iter().any(|existing| existing == session_id);
        pinned.retain(|existing| existing != session_id);
        if !was_pinned {
            pinned.push(session_id.to_string());
        }
        Ok(!was_pinned)
    }
    fn theme(&self) -> Option<String> {
        unreachable!("the agents view reads only the pins")
    }
    fn set_theme(&self, _theme: &str) -> Result<()> {
        unreachable!("the agents view reads only the pins")
    }
    fn show_images(&self) -> bool {
        unreachable!("the agents view reads only the pins")
    }
    fn set_show_images(&self, _enabled: bool) -> Result<()> {
        unreachable!("the agents view reads only the pins")
    }
    fn clear_on_shrink(&self) -> bool {
        unreachable!("the agents view reads only the pins")
    }
    fn set_clear_on_shrink(&self, _enabled: bool) -> Result<()> {
        unreachable!("the agents view reads only the pins")
    }
    fn show_terminal_progress(&self) -> bool {
        unreachable!("the agents view reads only the pins")
    }
    fn set_show_terminal_progress(&self, _enabled: bool) -> Result<()> {
        unreachable!("the agents view reads only the pins")
    }
    fn image_auto_resize(&self) -> bool {
        unreachable!("the agents view reads only the pins")
    }
    fn set_image_auto_resize(&self, _enabled: bool) -> Result<()> {
        unreachable!("the agents view reads only the pins")
    }
    fn block_images(&self) -> bool {
        unreachable!("the agents view reads only the pins")
    }
    fn set_block_images(&self, _blocked: bool) -> Result<()> {
        unreachable!("the agents view reads only the pins")
    }
    fn image_model(&self) -> Option<String> {
        unreachable!("the agents view reads only the pins")
    }
    fn enable_skill_commands(&self) -> bool {
        unreachable!("the agents view reads only the pins")
    }
    fn set_enable_skill_commands(&self, _enabled: bool) -> Result<()> {
        unreachable!("the agents view reads only the pins")
    }
    fn enable_builtin_skills(&self) -> bool {
        unreachable!("the agents view reads only the pins")
    }
    fn set_enable_builtin_skills(&self, _enabled: bool) -> Result<()> {
        unreachable!("the agents view reads only the pins")
    }
    fn show_hardware_cursor(&self) -> bool {
        unreachable!("the agents view reads only the pins")
    }
    fn set_show_hardware_cursor(&self, _enabled: bool) -> Result<()> {
        unreachable!("the agents view reads only the pins")
    }
    fn editor_padding_x(&self) -> u64 {
        unreachable!("the agents view reads only the pins")
    }
    fn set_editor_padding_x(&self, _padding: u64) -> Result<()> {
        unreachable!("the agents view reads only the pins")
    }
    fn autocomplete_max_visible(&self) -> u64 {
        unreachable!("the agents view reads only the pins")
    }
    fn set_autocomplete_max_visible(&self, _max_visible: u64) -> Result<()> {
        unreachable!("the agents view reads only the pins")
    }
    fn quiet_startup(&self) -> bool {
        unreachable!("the agents view reads only the pins")
    }
    fn set_quiet_startup(&self, _quiet: bool) -> Result<()> {
        unreachable!("the agents view reads only the pins")
    }
    fn idle_eviction_minutes(&self) -> String {
        unreachable!("the agents view reads only the pins")
    }
    fn set_idle_eviction_minutes(&self, _value: &str) -> Result<()> {
        unreachable!("the agents view reads only the pins")
    }
    fn mermaid_rendering_mode(&self) -> String {
        unreachable!("the agents view reads only the pins")
    }
    fn set_mermaid_rendering_mode(&self, _mode: &str) -> Result<()> {
        unreachable!("the agents view reads only the pins")
    }
    fn tree_filter_mode(&self) -> String {
        unreachable!("the agents view reads only the pins")
    }
    fn set_tree_filter_mode(&self, _mode: &str) -> Result<()> {
        unreachable!("the agents view reads only the pins")
    }
    fn default_service_tier(&self) -> String {
        unreachable!("the agents view reads only the pins")
    }
    fn set_default_service_tier(&self, _tier: &str) -> Result<()> {
        unreachable!("the agents view reads only the pins")
    }
    fn chat_detail(&self) -> String {
        unreachable!("the agents view reads only the pins")
    }
    fn set_chat_detail(&self, _detail: &str) -> Result<()> {
        unreachable!("the agents view reads only the pins")
    }
    fn warnings_anthropic_extra_usage(&self) -> bool {
        unreachable!("the agents view reads only the pins")
    }
    fn set_warnings_anthropic_extra_usage(&self, _enabled: bool) -> Result<()> {
        unreachable!("the agents view reads only the pins")
    }
    fn title_model(&self) -> Option<String> {
        unreachable!("the agents view reads only the pins")
    }
    fn set_title_model(&self, _model: Option<&str>) -> Result<()> {
        unreachable!("the agents view reads only the pins")
    }
    fn tool_call_preview(&self) -> String {
        unreachable!("the agents view reads only the pins")
    }
    fn set_tool_call_preview(&self, _mode: &str) -> Result<()> {
        unreachable!("the agents view reads only the pins")
    }
    fn tool_call_description_max_chars(&self) -> Option<u64> {
        unreachable!("the agents view reads only the pins")
    }
    fn set_tool_call_description_max_chars(&self, _max_chars: Option<u64>) -> Result<()> {
        unreachable!("the agents view reads only the pins")
    }
    fn ensure_global_settings_file(&self) -> Result<std::path::PathBuf> {
        unreachable!("the agents view reads only the pins")
    }
    fn update_channel(&self) -> Option<String> {
        unreachable!("the agents view reads only the pins")
    }
    fn set_update_channel(&self, _channel: &str) -> Result<()> {
        unreachable!("the agents view reads only the pins")
    }
    fn effective_update_channel(&self, _version: &str) -> String {
        unreachable!("the agents view reads only the pins")
    }
}

/// A mode over a running session and two saved ones, with the store
/// behind its pins.
fn mode_with_pins(store: &Arc<PinStore>) -> AgentsViewMode {
    let mut mode = mode_with_anchor(
        None,
        vec![roster_entry("p-live", "running", &parent_summary("p-live"))],
    );
    mode.options.client_settings = Some(AgentsViewSettings(store.clone()));
    mode.pinned_session_ids = store.pinned_session_ids().into_iter().collect();
    mode.saved = vec![
        saved_catalog_row("/x/a.jsonl", "saved-a", "alpha saved"),
        saved_catalog_row("/x/b.jsonl", "saved-b", "beta saved"),
    ];
    mode.rebuild_rows();
    mode
}

fn select(mode: &mut AgentsViewMode, title: &str) {
    mode.selected = mode
        .rows
        .iter()
        .position(|row| row.kind == RowKind::Agent && row.title == title)
        .unwrap_or_else(|| panic!("no agent row {title}"));
    mode.sync_selected_row_state();
}

fn agent_titles(mode: &AgentsViewMode) -> Vec<(String, bool)> {
    mode.rows
        .iter()
        .filter(|row| row.kind == RowKind::Agent)
        .map(|row| (row.title.clone(), row.pinned))
        .collect()
}

#[test]
fn pinned_agents_lead_the_list_and_keep_their_status_icon() {
    let mut mode = mode_with_pins(&PinStore::new(&[]));
    let unpinned = agent_titles(&mode);
    assert_eq!(unpinned.len(), 3);
    assert!(unpinned.iter().all(|(_, pinned)| !pinned));
    let last = unpinned.last().expect("a row").0.clone();
    select(&mut mode, &last);
    mode.handle_key("alt+p");
    assert_eq!(
        agent_titles(&mode)[0],
        (last.clone(), true),
        "the pinned agent moves to the top"
    );
    let pinned = mode.rows[0].clone();
    assert_eq!(pinned.display_section(), DisplaySection::Pinned);
    assert_eq!(
        pinned.section,
        Section::Inactive,
        "the status section stays the row's own"
    );
    assert_eq!(mode.status_text(), Some(format!("Pinned {last}").as_str()));
    assert_eq!(mode.actions.last(), Some(&"pinned"));
    assert_eq!(
        mode.rows[mode.selected].title, last,
        "the selection follows the pinned agent"
    );
}

#[test]
fn the_pin_key_toggles_and_persists_through_the_settings_seam() {
    let store = PinStore::new(&[]);
    let mut mode = mode_with_pins(&store);
    select(&mut mode, "alpha saved");
    mode.handle_key("alt+p");
    assert_eq!(store.pinned_session_ids(), vec!["saved-a".to_string()]);
    select(&mut mode, "alpha saved");
    mode.handle_key("alt+p");
    assert_eq!(store.pinned_session_ids(), Vec::<String>::new());
    assert_eq!(mode.status_text(), Some("Unpinned alpha saved"));
    assert_eq!(mode.actions.last(), Some(&"unpinned"));
    assert!(agent_titles(&mode).iter().all(|(_, pinned)| !pinned));
}

#[test]
fn pins_from_the_store_open_with_the_view() {
    let mode = mode_with_pins(&PinStore::new(&["saved-b"]));
    assert_eq!(agent_titles(&mode)[0], ("beta saved".to_string(), true));
}

#[test]
fn a_failed_write_changes_nothing() {
    let store = Arc::new(PinStore {
        pinned: Mutex::new(Vec::new()),
        fail: true,
    });
    let mut mode = mode_with_pins(&store);
    let before = agent_titles(&mode);
    select(&mut mode, "alpha saved");
    mode.handle_key("alt+p");
    assert_eq!(agent_titles(&mode), before);
    assert_eq!(
        mode.status_text(),
        Some("Failed to update pin: the settings file is read-only")
    );
}

#[test]
fn the_pin_key_ignores_non_agent_rows_a_search_and_a_missing_seam() {
    let store = PinStore::new(&[]);
    let mut mode = mode_with_pins(&store);
    mode.options.client_settings = None;
    select(&mut mode, "alpha saved");
    mode.handle_key("alt+p");
    assert_eq!(store.pinned_session_ids(), Vec::<String>::new(), "no seam");
    mode.options.client_settings = Some(AgentsViewSettings(store.clone()));
    mode.handle_key("x");
    mode.handle_key("alt+p");
    assert_eq!(
        store.pinned_session_ids(),
        Vec::<String>::new(),
        "a query leaves the key to the search field"
    );
}

#[test]
fn a_pinned_session_without_messages_stays_listed() {
    let mut empty = saved_catalog_row("/x/empty.jsonl", "saved-empty", "");
    empty["messageCount"] = serde_json::json!(0);
    empty["firstMessage"] = serde_json::json!("(no messages)");
    empty.as_object_mut().expect("row").remove("name");
    let mut mode = mode_with_pins(&PinStore::new(&[]));
    mode.saved.push(empty.clone());
    mode.rebuild_rows();
    let before = mode.rows.len();
    mode.pinned_session_ids.insert("saved-empty".to_string());
    mode.rebuild_rows();
    assert_eq!(
        mode.rows.len(),
        before + 1,
        "pinning keeps the empty session in the list"
    );
    assert!(mode
        .rows
        .iter()
        .any(|row| row.pinned && row.summary["sessionId"] == "saved-empty"));
}

#[test]
fn the_list_shows_a_pinned_block_above_the_status_sections() {
    let mut mode = mode_with_pins(&PinStore::new(&[]));
    select(&mut mode, "beta saved");
    mode.handle_key("alt+p");
    let theme = mode.theme.clone();
    let text: Vec<String> = mode.render_list(100, 24, 0).iter().map(flat).collect();
    let _ = theme;
    let headings: Vec<&str> = text
        .iter()
        .map(String::as_str)
        .filter(|row| {
            ["Pinned (", "Running (", "Idle (", "Inactive ("]
                .iter()
                .any(|prefix| row.trim_start().starts_with(prefix))
        })
        .collect();
    assert_eq!(headings, vec!["Pinned (1)", "Running (1)", "Inactive (1)"]);
}

#[test]
fn the_hint_names_the_pin_action_for_the_selected_row() {
    let mut mode = mode_with_pins(&PinStore::new(&[]));
    // The key label follows the platform (Alt+P, or Option+P on macOS).
    let key = mode.keybindings.key_text("app.agents.pin");
    select(&mut mode, "alpha saved");
    let hints = flat(&mode.render_hints(160, None));
    assert!(hints.contains(&format!("{key} pin")), "{hints}");
    mode.handle_key("alt+p");
    // The status line replaces the hints until it clears.
    mode.status = None;
    let hints = flat(&mode.render_hints(160, None));
    assert!(hints.contains(&format!("{key} unpin")), "{hints}");
    mode.options.client_settings = None;
    assert!(!flat(&mode.render_hints(160, None)).contains(&key));
}
