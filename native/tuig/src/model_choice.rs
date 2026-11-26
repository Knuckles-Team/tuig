//! Model choice over Arbora: the terminal lists the profiles `models.list`
//! serves and sends the caller's pick as a preference. Arbora decides which
//! profile runs, so the terminal displays the profile the task names.
//! // spec: TUIG-RUNTIME-R012.2.2

use crate::{ArboraClient, run_inspector::ReadRefusal};
use serde_json::Value;
use std::sync::{Arc, Mutex};

/// Message-metadata key that carries the untrusted profile preference.
pub const PREFERENCE_KEY: &str = "arboraModelProfile";
const MAX_PROFILES: usize = 256;
const MAX_PROFILE_BYTES: usize = 512;

/// The caller's current preference, shared with the spawned list-and-check read.
#[derive(Clone, Debug, Default)]
pub struct ModelChoice(Arc<Mutex<Option<String>>>);

impl ModelChoice {
    pub fn selected(&self) -> Option<String> {
        self.0.lock().ok().and_then(|selected| selected.clone())
    }

    pub fn set(&self, profile: Option<String>) {
        if let Ok(mut selected) = self.0.lock() {
            *selected = profile;
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModelAction {
    List,
    Select(String),
    Clear,
}

impl ModelAction {
    /// `/model` lists, `/model <profile>` selects and `/model -` clears.
    pub fn parse(text: &str) -> Option<Self> {
        let argument = text.trim().strip_prefix("/model")?;
        if !argument.is_empty() && !argument.starts_with(char::is_whitespace) {
            return None;
        }
        match argument.trim() {
            "" => Some(Self::List),
            "-" => Some(Self::Clear),
            profile => Some(Self::Select(profile.to_owned())),
        }
    }
}

fn printable(profile: &str) -> bool {
    !profile.trim().is_empty()
        && profile.len() <= MAX_PROFILE_BYTES
        && !profile.chars().any(char::is_control)
}

/// The profile ids of one `models.list` value; a malformed entry refuses the list.
pub fn profiles(value: &Value) -> Option<Vec<String>> {
    let models = value.get("models")?.as_array()?;
    if models.len() > MAX_PROFILES {
        return None;
    }
    models
        .iter()
        .map(|model| {
            let id = model.get("id")?.as_str()?;
            printable(id).then(|| id.to_owned())
        })
        .collect()
}

/// The profile a task names in its metadata, when it names a printable one.
pub fn displayed_profile(task: &Value) -> Option<String> {
    let profile = task.pointer("/metadata/arbora/modelProfile")?.as_str()?;
    printable(profile).then(|| profile.to_owned())
}

impl ArboraClient {
    // contract: TUIG-RUNTIME-R012.2
    pub async fn list_models(&self) -> Result<Vec<String>, ReadRefusal> {
        let value = self.invoke_operation("models.list", "{}").await?;
        profiles(&value).ok_or(ReadRefusal::Unavailable)
    }
}

/// Run one `/model` action and return the notice the terminal shows.
pub async fn run(client: &ArboraClient, choice: &ModelChoice, action: ModelAction) -> String {
    match action {
        ModelAction::Clear => {
            choice.set(None);
            "Model preference cleared; Arbora chooses the profile.".to_owned()
        }
        ModelAction::List | ModelAction::Select(_) => match client.list_models().await {
            Err(_) => "Model profiles unavailable: Arbora did not serve models.list.".to_owned(),
            Ok(profiles) => match action {
                ModelAction::Select(profile) if profiles.contains(&profile) => {
                    choice.set(Some(profile.clone()));
                    format!("Model preference set to {profile}. Arbora decides which profile runs.")
                }
                ModelAction::Select(profile) => format!(
                    "Model profile {} is not listed; preference unchanged.",
                    profile.escape_debug()
                ),
                _ => listing(&profiles, choice.selected().as_deref()),
            },
        },
    }
}

fn listing(profiles: &[String], selected: Option<&str>) -> String {
    if profiles.is_empty() {
        return "Arbora lists no model profiles.".to_owned();
    }
    let mut text = String::from("Model profiles:");
    for profile in profiles {
        let mark = if selected == Some(profile) { "*" } else { " " };
        text.push_str(&format!("\n{mark} {profile}"));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // spec: TUIG-RUNTIME-R012.2.2
    #[test]
    fn parses_list_select_and_clear_only_for_the_model_command() {
        assert_eq!(ModelAction::parse("/model"), Some(ModelAction::List));
        assert_eq!(ModelAction::parse(" /model  "), Some(ModelAction::List));
        assert_eq!(ModelAction::parse("/model -"), Some(ModelAction::Clear));
        assert_eq!(
            ModelAction::parse("/model fast one"),
            Some(ModelAction::Select("fast one".to_owned()))
        );
        assert_eq!(ModelAction::parse("/models"), None);
        assert_eq!(ModelAction::parse("model"), None);
    }

    // spec: TUIG-RUNTIME-R012.2.2
    #[test]
    fn profile_list_decodes_bounded_printable_ids_and_refuses_malformed_entries() {
        let good = json!({"kind": "chat", "models": [{"id": "a"}, {"id": "b"}]});
        assert_eq!(profiles(&good), Some(vec!["a".to_owned(), "b".to_owned()]));
        for bad in [
            json!({}),
            json!({"models": "a"}),
            json!({"models": [{"id": 1}]}),
            json!({"models": [{"id": ""}]}),
            json!({"models": [{"id": "a\u{1b}b"}]}),
            json!({"models": [{"id": "x".repeat(513)}]}),
            json!({"models": vec![json!({"id": "a"}); 257]}),
        ] {
            assert_eq!(profiles(&bad), None, "{bad}");
        }
    }

    // spec: TUIG-RUNTIME-R012.2.2
    #[test]
    fn displays_only_a_printable_profile_the_task_names() {
        let named = json!({"metadata": {"arbora": {"modelProfile": "served"}}});
        assert_eq!(displayed_profile(&named).as_deref(), Some("served"));
        for task in [
            json!({"metadata": {"arbora": {}}}),
            json!({"metadata": {"arbora": {"modelProfile": 3}}}),
            json!({"metadata": {"arbora": {"modelProfile": "a\nb"}}}),
            json!({}),
        ] {
            assert_eq!(displayed_profile(&task), None);
        }
    }

    // spec: TUIG-RUNTIME-R012.2.2
    #[test]
    fn listing_marks_the_selected_profile_and_reports_an_empty_list() {
        let profiles = vec!["a".to_owned(), "b".to_owned()];
        assert_eq!(listing(&profiles, Some("b")), "Model profiles:\n  a\n* b");
        assert_eq!(listing(&[], None), "Arbora lists no model profiles.");
    }

    // spec: TUIG-RUNTIME-R012.2.2
    #[test]
    fn the_choice_is_shared_between_clones() {
        let choice = ModelChoice::default();
        let other = choice.clone();
        other.set(Some("a".to_owned()));
        assert_eq!(choice.selected().as_deref(), Some("a"));
        choice.set(None);
        assert_eq!(other.selected(), None);
    }
}
