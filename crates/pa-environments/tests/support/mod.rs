use pa_environments::EnvironmentStore;
use pa_types::environments::{EnvironmentLanguage, ExecutionPolicy};
pub trait LegacyDraft {
    fn create_legacy_draft(
        &self,
        description: &str,
        language: EnvironmentLanguage,
        python: bool,
        policy: &ExecutionPolicy,
    ) -> anyhow::Result<std::path::PathBuf>;
}
impl LegacyDraft for EnvironmentStore {
    fn create_legacy_draft(
        &self,
        description: &str,
        language: EnvironmentLanguage,
        python: bool,
        policy: &ExecutionPolicy,
    ) -> anyhow::Result<std::path::PathBuf> {
        let path = self.create_draft(description, language, python, policy)?;
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path.join("environment.json"))?)?;
        manifest["protocol_version"] = serde_json::json!(1);
        manifest.as_object_mut().unwrap().remove("roles");
        std::fs::write(
            path.join("environment.json"),
            serde_json::to_vec(&manifest)?,
        )?;
        match language {
            EnvironmentLanguage::Python => std::fs::write(
                path.join("environment.py"),
                include_str!("../../fixtures/counter/environment.py"),
            )?,
            EnvironmentLanguage::TypeScript => std::fs::write(
                path.join("environment.ts"),
                include_str!("../../fixtures/counter/environment.ts"),
            )?,
        }
        Ok(path)
    }
}
