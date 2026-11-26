//! Slash commands that reach served Arbora operations.
//!
//! Each command names one registry operation. The client dispatches only an
//! operation the caller's registry advertises, so an unserved command shows one
//! unavailable notice naming the operation and sends no request.
//! // spec: TUIG-RUNTIME-R012.4.3

use crate::model_choice::{self, ModelAction, ModelChoice};
use crate::{ArboraClient, run_inspector::ReadRefusal};
use serde_json::{Value, json};

const MAX_RENDERED_CHARS: usize = 4_000;

#[derive(Debug, PartialEq, Eq)]
pub struct CommandSpec {
    pub name: &'static str,
    pub operation: &'static str,
    /// The command's argument becomes this parameter when present.
    pub argument: Option<&'static str>,
}

pub const MATRIX: [CommandSpec; 8] = [
    CommandSpec {
        name: "/tools",
        operation: "fleet.catalog.list",
        argument: None,
    },
    CommandSpec {
        name: "/mcp",
        operation: "fleet.status",
        argument: None,
    },
    CommandSpec {
        name: "/usage",
        operation: "usage.read",
        argument: None,
    },
    CommandSpec {
        name: "/skills",
        operation: "skills.list",
        argument: None,
    },
    CommandSpec {
        name: "/prompts",
        operation: "prompts.list",
        argument: None,
    },
    CommandSpec {
        name: "/kb",
        operation: "retrieval.search",
        argument: Some("query"),
    },
    CommandSpec {
        name: "/cron",
        operation: "cron.list",
        argument: None,
    },
    CommandSpec {
        name: "/pipeline",
        operation: "pipelines.list",
        argument: None,
    },
];

#[derive(Debug, PartialEq, Eq)]
pub enum ServedCommand {
    Model(ModelAction),
    Read(&'static CommandSpec, String),
}

impl ServedCommand {
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        if let Some(action) = ModelAction::parse(text) {
            return Some(Self::Model(action));
        }
        let (name, argument) = text
            .split_once(char::is_whitespace)
            .map_or((text, ""), |(name, rest)| (name, rest.trim()));
        MATRIX
            .iter()
            .find(|spec| spec.name == name)
            .map(|spec| Self::Read(spec, argument.to_owned()))
    }

    /// Run the command and return the notice the terminal shows.
    pub async fn run(self, client: &ArboraClient, choice: &ModelChoice, turn: &str) -> String {
        match self {
            Self::Model(action) => model_choice::run(client, choice, action).await,
            Self::Read(spec, argument) => read(client, spec, &argument, turn).await,
        }
    }
}

fn parameters(spec: &CommandSpec, argument: &str) -> Option<String> {
    match (spec.argument, argument.is_empty()) {
        (Some(_), true) => None,
        (Some(key), false) => Some(json!({ key: argument }).to_string()),
        (None, _) => Some("{}".to_owned()),
    }
}

async fn read(client: &ArboraClient, spec: &CommandSpec, argument: &str, turn: &str) -> String {
    let Some(payload) = parameters(spec, argument) else {
        return format!("{} needs an argument; no request was sent.", spec.name);
    };
    match client.invoke_operation(spec.operation, &payload).await {
        Ok(value) if spec.operation == "usage.read" => usage(&value, turn),
        Ok(value) => format!("{} ({}):\n{}", spec.name, spec.operation, render(&value)),
        Err(refusal) if spec.operation == "usage.read" => usage_unavailable(turn, &refusal),
        Err(refusal) => unavailable(spec, &refusal),
    }
}

fn unavailable(spec: &CommandSpec, refusal: &ReadRefusal) -> String {
    let cause = match refusal {
        ReadRefusal::Denied => "refused this caller",
        ReadRefusal::Timeout => "did not answer in time",
        _ => "does not serve it to this caller",
    };
    format!(
        "{} unavailable: Arbora {cause}; operation {}.",
        spec.name, spec.operation
    )
}

fn render(value: &Value) -> String {
    let text = format!("{value:#}");
    let mut shown: String = text
        .chars()
        .filter(|c| !c.is_control() || *c == '\n')
        .take(MAX_RENDERED_CHARS)
        .collect();
    if text.chars().count() > MAX_RENDERED_CHARS {
        shown.push_str("\n[truncated]");
    }
    shown
}

/// Per-turn and per-session usage; a figure Arbora does not report reads unavailable.
fn usage(value: &Value, turn: &str) -> String {
    let totals = value.get("totals").and_then(Value::as_object);
    let figure = |key: &str| {
        totals
            .and_then(|totals| totals.get(key))
            .and_then(Value::as_f64)
            .map_or_else(|| "unavailable".to_owned(), |number| number.to_string())
    };
    format!(
        "Turn:\n{turn}\n\nSession (usage.read):\nTokens: {}\nCost: {}",
        figure("total_tokens"),
        figure("cost")
    )
}

fn usage_unavailable(turn: &str, refusal: &ReadRefusal) -> String {
    let notice = unavailable(&MATRIX[2], refusal);
    format!("{notice}\n\nTurn:\n{turn}\n\nSession: Tokens: unavailable; Cost: unavailable")
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: TUIG-RUNTIME-R012.4.3
    #[test]
    fn every_registered_command_parses_to_its_operation() {
        for spec in &MATRIX {
            let parsed = ServedCommand::parse(&format!("  {} arg ", spec.name));
            assert_eq!(parsed, Some(ServedCommand::Read(spec, "arg".to_owned())));
        }
        assert_eq!(
            ServedCommand::parse("/model x"),
            Some(ServedCommand::Model(ModelAction::Select("x".to_owned())))
        );
        for text in ["/toolsy", "tools", "/history", "", "/op x {}"] {
            assert_eq!(ServedCommand::parse(text), None, "{text:?}");
        }
    }

    // spec: TUIG-RUNTIME-R012.4.3
    #[test]
    fn an_argument_command_refuses_an_empty_argument_before_any_request() {
        let kb = &MATRIX[5];
        assert_eq!(parameters(kb, ""), None);
        assert_eq!(parameters(kb, "q").as_deref(), Some(r#"{"query":"q"}"#));
        assert_eq!(parameters(&MATRIX[0], "ignored").as_deref(), Some("{}"));
    }

    // spec: TUIG-RUNTIME-R012.4.3
    #[test]
    fn rendered_values_drop_controls_and_truncate() {
        let shown = render(&json!({"a": "x\u{1b}y"}));
        assert!(!shown.contains('\u{1b}'));
        let long = render(&json!({"a": "z".repeat(5_000)}));
        assert!(long.ends_with("[truncated]"));
    }

    // spec: TUIG-RUNTIME-R012.4.3
    #[test]
    fn refusal_causes_are_named_without_leaking_detail() {
        for (refusal, words) in [
            (ReadRefusal::Denied, "refused this caller"),
            (ReadRefusal::Timeout, "in time"),
            (ReadRefusal::Unavailable, "does not serve"),
        ] {
            let text = unavailable(&MATRIX[3], &refusal);
            assert!(
                text.contains(words) && text.contains("skills.list"),
                "{text}"
            );
        }
    }

    #[test]
    fn missing_usage_figures_read_unavailable_never_zero() {
        let none = usage(&json!({}), "Requests: unavailable");
        assert!(none.contains("Tokens: unavailable") && none.contains("Cost: unavailable"));
        let some = usage(&json!({"totals": {"total_tokens": 12.0, "cost": 0.5}}), "t");
        assert!(some.contains("Tokens: 12") && some.contains("Cost: 0.5"));
        assert!(usage_unavailable("t", &ReadRefusal::Unavailable).contains("usage.read"));
    }
}
