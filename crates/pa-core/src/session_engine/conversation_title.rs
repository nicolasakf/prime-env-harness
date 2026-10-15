//! Conversation titles: a small model call that names a session from its
//! first exchange. The `titleModel` setting picks the model; the daemon
//! runs it once after the first settled turn of a top-level session and on
//! demand for `/name` with no argument.

use serde_json::Value;

use crate::settings::TitleModelSetting;

const TITLE_CONTEXT_MESSAGES: usize = 4;
const TITLE_MAX_CHARS_PER_MESSAGE: usize = 800;
const TITLE_MAX_LENGTH: usize = 80;
const TITLE_MAX_TOKENS: u64 = 80;
/// Reasoning models spend part of the budget thinking, so the answer needs
/// the larger allowance.
const TITLE_REASONING_MAX_TOKENS: u64 = 1024;

pub const CONVERSATION_TITLE_SYSTEM_PROMPT: &str = "Create a concise title for an AI coding-agent conversation.\n\nOutput only the title. Use 3-8 words, no quotes, no label, and no trailing punctuation.";

/// The model-facing transcript excerpt: the first few user and assistant
/// messages, whitespace-normalized and clipped. `None` until the
/// conversation holds a user message with text.
#[must_use]
pub fn build_conversation_title_context(messages: &[Value]) -> Option<String> {
    let lines: Vec<String> = messages
        .iter()
        .filter_map(|message| {
            let role = message.get("role").and_then(Value::as_str)?;
            matches!(role, "user" | "assistant").then_some((role, message))
        })
        .take(TITLE_CONTEXT_MESSAGES)
        .filter_map(|(role, message)| {
            // A persisted `content` is a string, or typed blocks of which
            // only the text blocks count.
            let text = match message.get("content")? {
                Value::String(text) => text.clone(),
                Value::Array(blocks) => blocks
                    .iter()
                    .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
                    .filter_map(|block| block.get("text").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join("\n"),
                _ => String::new(),
            };
            let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
            let text = if normalized.chars().count() > TITLE_MAX_CHARS_PER_MESSAGE {
                let clipped: String = normalized
                    .chars()
                    .take(TITLE_MAX_CHARS_PER_MESSAGE)
                    .collect();
                format!("{clipped}…")
            } else {
                normalized
            };
            (!text.is_empty()).then(|| format!("{role}: {text}"))
        })
        .collect();
    lines
        .iter()
        .any(|line| line.starts_with("user:"))
        .then(|| lines.join("\n"))
}

/// Reduce the model output to a bare title: the last `<title>` tag wins,
/// else the first non-empty line; labels, quotes, ANSI, and trailing
/// punctuation drop out and the result is capped at a word boundary.
#[must_use]
pub fn clean_conversation_title(raw: &str) -> Option<String> {
    let tagged = raw
        .match_indices("<title>")
        .filter_map(|(start, _)| {
            let body = &raw[start + "<title>".len()..];
            body.find("</title>").map(|end| &body[..end])
        })
        .last();
    let first_line = tagged
        .unwrap_or(raw)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())?;

    let stripped = crate::tools::render_utils::strip_ansi(first_line);
    let without_label = {
        let lower = stripped.to_lowercase();
        match lower.strip_prefix("title") {
            Some(rest) if rest.trim_start().starts_with(':') => {
                let colon = stripped.find(':').unwrap_or(0);
                stripped[colon + 1..].to_string()
            }
            _ => stripped,
        }
    };
    let quotes: &[char] = &['`', '"', '\'', '“', '”'];
    let normalized = without_label
        .trim_start()
        .trim_matches(quotes)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let normalized = normalized
        .trim_end_matches(['.', '!', '?', ';', ':'])
        .trim()
        .to_string();
    if normalized.is_empty() || normalized.starts_with('<') {
        return None;
    }
    if normalized.chars().count() <= TITLE_MAX_LENGTH {
        return Some(normalized);
    }
    let truncated: String = normalized.chars().take(TITLE_MAX_LENGTH + 1).collect();
    let cut = truncated
        .rfind(' ')
        .filter(|index| *index > 0)
        .or_else(|| {
            truncated
                .char_indices()
                .nth(TITLE_MAX_LENGTH)
                .map(|(index, _)| index)
        })
        .unwrap_or(truncated.len());
    Some(truncated[..cut].trim().to_string())
}

/// Where the title call resolves its model and credentials.
pub struct ConversationTitleRequest<'a> {
    pub agent_dir: &'a std::path::Path,
    pub model: &'a TitleModelSetting,
    pub messages: &'a [Value],
}

/// Name the conversation with the configured title model. `None` when the
/// conversation has no user text yet, the model is not available with
/// configured auth, or the call fails; a title is never worth failing a
/// turn over.
pub async fn generate_conversation_title(request: ConversationTitleRequest<'_>) -> Option<String> {
    let context = build_conversation_title_context(request.messages)?;
    let auth = crate::auth::AuthStorage::create(request.agent_dir);
    let mut registry =
        crate::models::ModelRegistry::create(auth, request.agent_dir.join("models.json"));
    registry.load_private_authorization_from_cache();
    let model = registry
        .get_all()
        .iter()
        .find(|model| model.provider == request.model.provider && model.id == request.model.id)
        .filter(|model| registry.has_configured_auth(model))
        .cloned()?;
    let resolved = registry.get_api_key_and_headers(&model, model.headers.as_ref());
    if !resolved.ok {
        return None;
    }
    let max_tokens = if model.reasoning {
        TITLE_REASONING_MAX_TOKENS.min(model.max_tokens)
    } else {
        TITLE_MAX_TOKENS
    };
    let llm_context = pa_types::ai::Context {
        system_prompt: Some(CONVERSATION_TITLE_SYSTEM_PROMPT.to_string()),
        messages: vec![pa_types::ai::Message::User(pa_types::ai::UserMessage {
            content: pa_types::ai::UserContent::Text(context),
            timestamp: 0,
            rest: serde_json::Map::default(),
        })],
        tools: None,
    };
    let options = pa_ai::types::SimpleStreamOptions::from_base(pa_ai::types::StreamOptions {
        max_tokens: Some(max_tokens),
        api_key: resolved.api_key,
        headers: resolved
            .headers
            .map(|headers| headers.into_iter().collect()),
        ..Default::default()
    });
    let response = pa_ai::complete_simple(&model, &llm_context, Some(options))
        .await
        .ok()?;
    if matches!(
        response.stop_reason,
        pa_types::ai::StopReason::Error | pa_types::ai::StopReason::Aborted
    ) {
        return None;
    }
    let text = response
        .content
        .iter()
        .filter_map(|block| match block {
            pa_types::ai::AssistantContentBlock::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    clean_conversation_title(&text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn context_takes_the_first_four_user_and_assistant_messages() {
        let messages = vec![
            json!({"role": "user", "content": "  fix   the\nbuild "}),
            json!({"role": "toolResult", "content": "ignored"}),
            json!({"role": "assistant", "content": [
                {"type": "thinking", "thinking": "hidden"},
                {"type": "text", "text": "On it."}
            ]}),
            json!({"role": "user", "content": [{"type": "text", "text": "thanks"}]}),
            json!({"role": "assistant", "content": "done"}),
            json!({"role": "user", "content": "beyond the window"}),
        ];
        assert_eq!(
            build_conversation_title_context(&messages).as_deref(),
            Some("user: fix the build\nassistant: On it.\nuser: thanks\nassistant: done")
        );
    }

    #[test]
    fn context_needs_a_user_message_with_text() {
        assert_eq!(build_conversation_title_context(&[]), None);
        let assistant_only = vec![json!({"role": "assistant", "content": "hello"})];
        assert_eq!(build_conversation_title_context(&assistant_only), None);
        let blank_user = vec![json!({"role": "user", "content": "   "})];
        assert_eq!(build_conversation_title_context(&blank_user), None);
    }

    #[test]
    fn context_clips_long_messages() {
        let long = "word ".repeat(400);
        let context = build_conversation_title_context(&[json!({"role": "user", "content": long})])
            .expect("context");
        assert_eq!(context.chars().count(), "user: ".len() + 800 + 1);
        assert!(context.ends_with('…'));
    }

    #[test]
    fn cleaning_strips_labels_quotes_and_punctuation() {
        assert_eq!(
            clean_conversation_title("Title: \"Fix the flaky build.\"\nextra").as_deref(),
            Some("Fix the flaky build")
        );
        assert_eq!(
            clean_conversation_title("\n\n  `Port settings menu`  ").as_deref(),
            Some("Port settings menu")
        );
        assert_eq!(
            clean_conversation_title("<title>First</title> noise <title>Second try</title>")
                .as_deref(),
            Some("Second try")
        );
        assert_eq!(
            clean_conversation_title("\u{1b}[1mBold title\u{1b}[0m").as_deref(),
            Some("Bold title")
        );
    }

    #[test]
    fn cleaning_rejects_empty_and_markup_output() {
        assert_eq!(clean_conversation_title(""), None);
        assert_eq!(clean_conversation_title("   \n  "), None);
        assert_eq!(clean_conversation_title("<think>reasoning</think>"), None);
        assert_eq!(clean_conversation_title("Title:"), None);
    }

    #[test]
    fn cleaning_caps_long_titles_at_a_word_boundary() {
        let raw = "alpha ".repeat(30);
        let title = clean_conversation_title(&raw).expect("title");
        assert!(title.chars().count() <= 80);
        assert!(title.split(' ').all(|word| word == "alpha"));
    }

    /// A models.json provider on a private faux API, so the registry
    /// resolves the title model with configured auth and the request never
    /// leaves the process.
    fn titles_agent_dir(api: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("models.json"),
            json!({ "providers": { "titles": {
                "baseUrl": "http://127.0.0.1:9",
                "apiKey": "title-key",
                "api": api,
                "models": [{ "id": "title-1", "name": "Title 1", "contextWindow": 8000 }]
            } } })
            .to_string(),
        )
        .unwrap();
        dir
    }

    fn register_title_faux(api: &str, reply: &str) -> pa_ai::faux::FauxProviderRegistration {
        let faux = pa_ai::faux::register_faux_provider(pa_ai::faux::RegisterFauxProviderOptions {
            api: Some(api.to_string()),
            provider: Some("titles".to_string()),
            models: Some(vec![pa_ai::faux::FauxModelDefinition {
                id: "title-1".to_string(),
                ..Default::default()
            }]),
            ..Default::default()
        });
        faux.set_responses(vec![pa_ai::faux::FauxResponseStep::Message(
            pa_ai::faux::faux_assistant_text_message(
                reply,
                pa_ai::faux::FauxAssistantMessageOptions::default(),
            ),
        )]);
        faux
    }

    #[tokio::test]
    async fn generation_names_the_conversation_with_the_title_model() {
        let api = "faux-title-generation";
        let dir = titles_agent_dir(api);
        let faux = register_title_faux(api, "Title: \"Fix the flaky build.\"");
        let messages = vec![
            json!({"role": "user", "content": "the build is flaky"}),
            json!({"role": "assistant", "content": "Looking."}),
        ];
        let title = generate_conversation_title(ConversationTitleRequest {
            agent_dir: dir.path(),
            model: &TitleModelSetting {
                provider: "titles".to_string(),
                id: "title-1".to_string(),
            },
            messages: &messages,
        })
        .await;
        let calls = faux.call_count();
        faux.unregister();
        assert_eq!(title.as_deref(), Some("Fix the flaky build"));
        assert_eq!(calls, 1);
    }

    #[tokio::test]
    async fn generation_needs_a_resolvable_model_and_user_text() {
        let api = "faux-title-unresolved";
        let dir = titles_agent_dir(api);
        let faux = register_title_faux(api, "Unused");
        let user = vec![json!({"role": "user", "content": "hello"})];
        let missing_model = generate_conversation_title(ConversationTitleRequest {
            agent_dir: dir.path(),
            model: &TitleModelSetting {
                provider: "titles".to_string(),
                id: "absent".to_string(),
            },
            messages: &user,
        })
        .await;
        let no_user_text = generate_conversation_title(ConversationTitleRequest {
            agent_dir: dir.path(),
            model: &TitleModelSetting {
                provider: "titles".to_string(),
                id: "title-1".to_string(),
            },
            messages: &[],
        })
        .await;
        let calls = faux.call_count();
        faux.unregister();
        assert_eq!((missing_model, no_user_text, calls), (None, None, 0));
    }
}
