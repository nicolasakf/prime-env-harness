//! The interactive client-settings seam implementation: every call opens
//! the file-backed settings manager over the run's directories (the store
//! is a pair of small JSON files, so the re-read is the same freshness
//! the TS manager's `reload` produces) and applies the one setting.

use anyhow::Result;
use std::path::PathBuf;
use std::sync::Arc;

use pa_tui::client_settings::ClientSettings;

/// The seam handle the interactive run carries (TS injects the same
/// settings manager into the interactive mode).
#[derive(Clone)]
pub struct CliClientSettings {
    cwd: PathBuf,
    agent_dir: PathBuf,
}

impl CliClientSettings {
    pub fn new(cwd: PathBuf, agent_dir: PathBuf) -> Arc<Self> {
        Arc::new(Self { cwd, agent_dir })
    }

    fn manager(&self) -> pa_core::settings::SettingsManager {
        pa_core::settings::SettingsManager::create(&self.cwd, &self.agent_dir)
    }
}

macro_rules! setting {
    ($get:ident, $set:ident, $getter:ident, $setter:ident, $ty:ty) => {
        fn $get(&self) -> $ty {
            self.manager().$getter()
        }

        fn $set(&self, value: $ty) -> Result<()> {
            self.manager().$setter(value)
        }
    };
}

macro_rules! str_setting {
    ($get:ident, $set:ident, $getter:ident, $setter:ident) => {
        fn $get(&self) -> String {
            self.manager().$getter().to_string()
        }

        fn $set(&self, value: &str) -> Result<()> {
            self.manager().$setter(value)
        }
    };
}

impl ClientSettings for CliClientSettings {
    fn theme(&self) -> Option<String> {
        self.manager().get_theme().map(str::to_string)
    }

    fn set_theme(&self, theme: &str) -> Result<()> {
        self.manager().set_theme(theme.to_string())
    }

    fn default_service_tier(&self) -> String {
        // The wire name of the persisted default tier (TS
        // `getDefaultServiceTier()`), "default" when unset or unreadable.
        serde_json::to_value(self.manager().get_default_service_tier())
            .ok()
            .and_then(|value| value.as_str().map(str::to_string))
            .unwrap_or_else(|| "default".to_string())
    }

    fn set_default_service_tier(&self, tier: &str) -> Result<()> {
        let parsed: pa_types::ai::ServiceTier =
            serde_json::from_value(serde_json::Value::String(tier.to_string()))
                .map_err(|error| anyhow::anyhow!("Invalid service tier \"{tier}\": {error}"))?;
        self.manager().set_default_service_tier(parsed)
    }

    setting!(
        show_images,
        set_show_images,
        get_show_images,
        set_show_images,
        bool
    );
    setting!(
        clear_on_shrink,
        set_clear_on_shrink,
        get_clear_on_shrink,
        set_clear_on_shrink,
        bool
    );
    setting!(
        show_terminal_progress,
        set_show_terminal_progress,
        get_show_terminal_progress,
        set_show_terminal_progress,
        bool
    );
    setting!(
        image_auto_resize,
        set_image_auto_resize,
        get_image_auto_resize,
        set_image_auto_resize,
        bool
    );
    setting!(
        block_images,
        set_block_images,
        get_block_images,
        set_block_images,
        bool
    );

    fn image_model(&self) -> Option<String> {
        self.manager().get_image_model()
    }
    setting!(
        enable_skill_commands,
        set_enable_skill_commands,
        get_enable_skill_commands,
        set_enable_skill_commands,
        bool
    );
    setting!(
        enable_builtin_skills,
        set_enable_builtin_skills,
        get_enable_builtin_skills,
        set_enable_builtin_skills,
        bool
    );
    setting!(
        show_hardware_cursor,
        set_show_hardware_cursor,
        get_show_hardware_cursor,
        set_show_hardware_cursor,
        bool
    );
    setting!(
        editor_padding_x,
        set_editor_padding_x,
        get_editor_padding_x,
        set_editor_padding_x,
        u64
    );
    setting!(
        autocomplete_max_visible,
        set_autocomplete_max_visible,
        get_autocomplete_max_visible,
        set_autocomplete_max_visible,
        u64
    );
    setting!(
        quiet_startup,
        set_quiet_startup,
        get_quiet_startup,
        set_quiet_startup,
        bool
    );
    str_setting!(
        idle_eviction_minutes,
        set_idle_eviction_minutes,
        get_idle_eviction_minutes,
        set_idle_eviction_minutes
    );
    str_setting!(
        mermaid_rendering_mode,
        set_mermaid_rendering_mode,
        get_mermaid_rendering_mode,
        set_mermaid_rendering_mode
    );
    str_setting!(
        tree_filter_mode,
        set_tree_filter_mode,
        get_tree_filter_mode,
        set_tree_filter_mode
    );
    str_setting!(
        chat_detail,
        set_chat_detail,
        get_chat_detail,
        set_chat_detail
    );
    setting!(
        warnings_anthropic_extra_usage,
        set_warnings_anthropic_extra_usage,
        get_warnings_anthropic_extra_usage,
        set_warnings_anthropic_extra_usage,
        bool
    );

    fn title_model(&self) -> Option<String> {
        let model = self.manager().get_title_model()?;
        Some(format!("{}/{}", model.provider, model.id))
    }

    fn set_title_model(&self, model: Option<&str>) -> Result<()> {
        let model = model
            .map(|reference| {
                reference
                    .split_once('/')
                    .map(|(provider, id)| pa_core::settings::TitleModelSetting {
                        provider: provider.to_string(),
                        id: id.to_string(),
                    })
                    .ok_or_else(|| {
                        anyhow::anyhow!("Title model \"{reference}\" must be provider/id")
                    })
            })
            .transpose()?;
        self.manager().set_title_model(model)
    }

    fn tool_call_preview(&self) -> String {
        self.manager().get_tool_call_preview().to_string()
    }

    fn set_tool_call_preview(&self, mode: &str) -> Result<()> {
        self.manager().set_tool_call_preview(mode)
    }

    fn tool_call_description_max_chars(&self) -> Option<u64> {
        self.manager().get_tool_call_description_max_chars()
    }

    fn set_tool_call_description_max_chars(&self, max_chars: Option<u64>) -> Result<()> {
        self.manager()
            .set_tool_call_description_max_chars(max_chars)
    }

    fn pinned_session_ids(&self) -> Vec<String> {
        self.manager().get_pinned_session_ids()
    }

    fn toggle_session_pinned(&self, session_id: &str) -> Result<bool> {
        self.manager().toggle_session_pinned(session_id)
    }

    fn ensure_global_settings_file(&self) -> Result<PathBuf> {
        let path = self.agent_dir.join("settings.json");
        if !path.exists() {
            std::fs::create_dir_all(&self.agent_dir)?;
            std::fs::write(&path, "{}\n")?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
            }
        }
        Ok(path)
    }

    /// `updateChannel`: the settings enum's wire value; unset reads as
    /// `None` (TS's global-only `getUpdateChannel`).
    fn update_channel(&self) -> Option<String> {
        let channel = self.manager().get_update_channel()?;
        Some(
            match channel {
                pa_core::settings::UpdateChannel::Stable => "stable",
                pa_core::settings::UpdateChannel::Nightly => "nightly",
            }
            .to_string(),
        )
    }

    fn set_update_channel(&self, channel: &str) -> Result<()> {
        let channel = match channel {
            "stable" => pa_core::settings::UpdateChannel::Stable,
            "nightly" => pa_core::settings::UpdateChannel::Nightly,
            _ => anyhow::bail!("unknown update channel: {channel}"),
        };
        self.manager().set_update_channel(channel)
    }

    fn effective_update_channel(&self, version: &str) -> String {
        let preferred = self
            .manager()
            .get_update_channel()
            .map(|channel| match channel {
                pa_core::settings::UpdateChannel::Stable => {
                    pa_core::update::version::UpdateChannel::Stable
                }
                pa_core::settings::UpdateChannel::Nightly => {
                    pa_core::update::version::UpdateChannel::Nightly
                }
            });
        pa_core::update::version::resolve_update_channel(version, preferred)
            .wire_name()
            .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The seam over the real settings store: every write persists through
    /// the pa-core manager and the next read (a fresh manager over the same
    /// dirs, exactly what every call does) sees it.
    #[test]
    fn seam_round_trips_through_the_settings_store() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let agent_dir = dir.path().join("agent");
        std::fs::create_dir_all(&agent_dir).expect("agent dir");
        let settings = CliClientSettings::new(dir.path().to_path_buf(), agent_dir.clone());

        // The TS defaults read first.
        assert!(settings.show_images());
        assert!(!settings.quiet_startup());
        assert_eq!(settings.idle_eviction_minutes(), "90");
        assert_eq!(settings.mermaid_rendering_mode(), "streaming");
        assert_eq!(settings.tree_filter_mode(), "user-only");
        assert!(settings.warnings_anthropic_extra_usage());

        // Writes persist (the settings file lands in the agent dir).
        settings.set_theme("dark").expect("theme");
        settings.set_idle_eviction_minutes("off").expect("idle");
        settings.set_tree_filter_mode("all").expect("tree filter");
        settings.set_show_images(false).expect("show images");

        assert_eq!(settings.theme().as_deref(), Some("dark"));
        assert_eq!(settings.idle_eviction_minutes(), "off");
        assert_eq!(settings.tree_filter_mode(), "all");
        assert!(!settings.show_images());

        // The persisted file the real consumers read.
        let content =
            std::fs::read_to_string(agent_dir.join("settings.json")).expect("settings file");
        let value: serde_json::Value = serde_json::from_str(&content).expect("parse");
        assert_eq!(value["theme"], "dark");
        assert_eq!(value["idleEvictionMinutes"], "off");
        assert_eq!(value["treeFilterMode"], "all");

        // The update channel starts unset (the version infers it) and
        // pins through the /nightly off path's seam.
        assert_eq!(settings.update_channel(), None);
        settings.set_update_channel("stable").expect("channel");
        assert_eq!(settings.update_channel().as_deref(), Some("stable"));
        let content =
            std::fs::read_to_string(agent_dir.join("settings.json")).expect("settings file");
        let value: serde_json::Value = serde_json::from_str(&content).expect("parse");
        assert_eq!(value["updateChannel"], "stable");
        // ...and the /nightly on path's.
        settings.set_update_channel("nightly").expect("channel");
        assert_eq!(settings.update_channel().as_deref(), Some("nightly"));
    }

    /// The conversation-title, tool-call-preview, and pin rows persist
    /// through the same store, and the settings file is created on demand.
    #[test]
    fn seam_persists_titles_tool_previews_pins_and_the_settings_file() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let agent_dir = dir.path().join("agent");
        let settings = CliClientSettings::new(dir.path().to_path_buf(), agent_dir.clone());

        assert_eq!(settings.title_model(), None);
        assert_eq!(settings.tool_call_preview(), "code");
        assert_eq!(settings.tool_call_description_max_chars(), None);
        assert_eq!(settings.pinned_session_ids(), Vec::<String>::new());

        // A model id may itself contain slashes: the provider is the part
        // before the first one.
        settings
            .set_title_model(Some("openrouter/z-ai/glm-5.3"))
            .expect("title model");
        settings
            .set_tool_call_preview("description")
            .expect("preview");
        settings
            .set_tool_call_description_max_chars(Some(80))
            .expect("limit");
        assert!(settings.toggle_session_pinned("s1").expect("pin"));
        assert!(settings.toggle_session_pinned("s2").expect("pin"));
        assert!(!settings.toggle_session_pinned("s1").expect("unpin"));

        assert_eq!(
            settings.title_model().as_deref(),
            Some("openrouter/z-ai/glm-5.3")
        );
        assert_eq!(settings.tool_call_preview(), "description");
        assert_eq!(settings.tool_call_description_max_chars(), Some(80));
        assert_eq!(settings.pinned_session_ids(), vec!["s2".to_string()]);
        let written: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(agent_dir.join("settings.json")).expect("settings file"),
        )
        .expect("settings json");
        assert_eq!(
            written["titleModel"],
            serde_json::json!({ "provider": "openrouter", "id": "z-ai/glm-5.3" })
        );

        settings.set_title_model(None).expect("clear");
        assert_eq!(settings.title_model(), None);
        assert!(settings.set_title_model(Some("no-slash")).is_err());

        // An existing file is left alone; a missing one is created empty.
        let existing = settings.ensure_global_settings_file().expect("existing");
        assert!(std::fs::read_to_string(&existing)
            .expect("read")
            .contains("tool"));
        let fresh_dir = tempfile::TempDir::new().expect("temp dir");
        let fresh = CliClientSettings::new(
            fresh_dir.path().to_path_buf(),
            fresh_dir.path().join("agent"),
        );
        let created = fresh.ensure_global_settings_file().expect("created");
        assert_eq!(std::fs::read_to_string(created).expect("read"), "{}\n");
    }
}
