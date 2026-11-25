//! Optional producer-reported turn counts, never inferred from reply text.
use crate::correlation::TaskCorrelation;
use serde_json::{Map, Value};

const MAX_COUNT: u64 = 9_007_199_254_740_991;

const KEYS: [&str; 5] = [
    "requests",
    "tool_calls",
    "input_tokens",
    "output_tokens",
    "total_tokens",
];
const LABELS: [&str; 5] = [
    "Requests",
    "Tool calls",
    "Input tokens",
    "Output tokens",
    "Total tokens",
];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum TurnUsage {
    #[default]
    Unavailable,
    Counts([Option<u64>; 5]),
}

impl TurnUsage {
    pub fn from_task(task: &Value, binding: &TaskCorrelation) -> Self {
        Self::parse(task, binding).unwrap_or_default()
    }

    fn parse(task: &Value, binding: &TaskCorrelation) -> Option<Self> {
        let run = binding.run_id()?;
        if !Self::identity_matches(task, binding, run) {
            return None;
        }
        let usage = task.pointer("/metadata/arbora/usage")?.as_object()?;
        if !Self::usage_scoped(usage, binding, run) {
            return None;
        }
        let counts = usage.get("counts")?.as_object()?;
        Some(Self::Counts(Self::decode_counts(counts)?))
    }

    fn identity_matches(task: &Value, binding: &TaskCorrelation, run: &str) -> bool {
        task.get("id").and_then(|value| value.as_str()) == Some(binding.task_id())
            && task.get("contextId").and_then(|value| value.as_str()) == Some(binding.context_id())
            && task
                .pointer("/metadata/arbora/runId")
                .and_then(|value| value.as_str())
                == Some(run)
    }

    fn usage_scoped(usage: &Map<String, Value>, binding: &TaskCorrelation, run: &str) -> bool {
        usage.get("scope").and_then(|value| value.as_str()) == Some("turn")
            && usage.get("taskId").and_then(|value| value.as_str()) == Some(binding.task_id())
            && usage.get("contextId").and_then(|value| value.as_str()) == Some(binding.context_id())
            && usage.get("runId").and_then(|value| value.as_str()) == Some(run)
    }

    fn decode_counts(counts: &Map<String, Value>) -> Option<[Option<u64>; 5]> {
        if counts.keys().any(|key| !KEYS.contains(&key.as_str())) {
            return None;
        }
        let mut values = [None; 5];
        for (index, key) in KEYS.iter().enumerate() {
            if let Some(value) = counts.get(*key) {
                values[index] = Some(Self::decode_count(value)?);
            }
        }
        Some(values)
    }

    fn decode_count(value: &Value) -> Option<u64> {
        value.as_u64().filter(|count| *count <= MAX_COUNT)
    }

    pub fn text(&self) -> String {
        let mut lines = match self {
            Self::Unavailable => vec!["Turn usage: unavailable".to_owned()],
            Self::Counts(values) => {
                let mut lines = vec!["Turn usage:".to_owned()];
                for (label, value) in LABELS.iter().zip(values) {
                    lines.push(format!(
                        "{label}: {}",
                        value.map_or_else(|| "unavailable".to_owned(), |count| count.to_string())
                    ));
                }
                lines
            }
        };
        lines.push("Session usage: unavailable".to_owned());
        lines.push("Cost: unavailable".to_owned());
        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture() -> (Value, TaskCorrelation) {
        let task = json!({
            "kind":"task", "id":"task-one", "contextId":"context-two",
            "status":{"state":"completed"},
            "metadata":{"arbora":{"runId":"run-three","usage":{
                "runId":"run-three","taskId":"task-one","contextId":"context-two",
                "scope":"turn","counts":{"requests":0,"input_tokens":MAX_COUNT}
            }}}
        });
        let binding = TaskCorrelation::from_task(&task).unwrap();
        (task, binding)
    }

    // spec: TUIG-RUNTIME-R012.6.1
    #[test]
    fn zero_missing_and_maximum_counts_remain_distinct() {
        let (task, binding) = fixture();
        let usage = TurnUsage::from_task(&task, &binding);
        assert_eq!(
            usage,
            TurnUsage::Counts([Some(0), None, Some(MAX_COUNT), None, None])
        );
        let text = usage.text();
        for expected in [
            "Turn usage:",
            "Requests: 0",
            "Tool calls: unavailable",
            "Input tokens: 9007199254740991",
            "Output tokens: unavailable",
            "Total tokens: unavailable",
            "Session usage: unavailable",
            "Cost: unavailable",
        ] {
            assert!(text.contains(expected), "{expected}");
        }
    }

    // spec: TUIG-RUNTIME-R012.6.1
    #[test]
    fn missing_usage_and_empty_counts_never_fabricate_zero() {
        let (mut task, binding) = fixture();
        task["metadata"]["arbora"]["usage"]["counts"] = json!({});
        assert_eq!(
            TurnUsage::from_task(&task, &binding),
            TurnUsage::Counts([None; 5])
        );
        task["metadata"]["arbora"]
            .as_object_mut()
            .unwrap()
            .remove("usage");
        assert_eq!(
            TurnUsage::from_task(&task, &binding),
            TurnUsage::Unavailable
        );
        assert!(TurnUsage::Unavailable.text().contains("Cost: unavailable"));
    }

    // spec: TUIG-RUNTIME-R012.6.1
    #[test]
    fn foreign_or_missing_bindings_refuse_the_whole_usage() {
        for path in [
            "/id",
            "/contextId",
            "/metadata/arbora/runId",
            "/metadata/arbora/usage/taskId",
            "/metadata/arbora/usage/contextId",
            "/metadata/arbora/usage/runId",
            "/metadata/arbora/usage/scope",
        ] {
            for invalid in [json!("foreign"), Value::Null] {
                let (mut task, binding) = fixture();
                *task.pointer_mut(path).unwrap() = invalid;
                assert_eq!(
                    TurnUsage::from_task(&task, &binding),
                    TurnUsage::Unavailable,
                    "{path}"
                );
            }
        }
        let (task, _) = fixture();
        let mut unbound = task.clone();
        unbound["metadata"]["arbora"]
            .as_object_mut()
            .unwrap()
            .remove("runId");
        let binding = TaskCorrelation::from_task(&unbound).unwrap();
        assert_eq!(
            TurnUsage::from_task(&task, &binding),
            TurnUsage::Unavailable
        );
    }

    // spec: TUIG-RUNTIME-R012.6.1
    #[test]
    fn malformed_counts_refuse_without_partial_values() {
        for invalid in [
            json!(MAX_COUNT + 1),
            json!(-1),
            json!(true),
            json!(1.0),
            json!("1"),
            Value::Null,
            json!([]),
            json!({}),
            serde_json::from_str::<Value>("18446744073709551616").unwrap(),
        ] {
            let (mut task, binding) = fixture();
            task["metadata"]["arbora"]["usage"]["counts"]["tool_calls"] = invalid;
            assert_eq!(
                TurnUsage::from_task(&task, &binding),
                TurnUsage::Unavailable
            );
        }
        for counts in [json!({"requests":1,"cost":0}), json!([]), Value::Null] {
            let (mut task, binding) = fixture();
            task["metadata"]["arbora"]["usage"]["counts"] = counts;
            assert_eq!(
                TurnUsage::from_task(&task, &binding),
                TurnUsage::Unavailable
            );
        }
        let (mut task, binding) = fixture();
        task["metadata"]["arbora"]["usage"] = json!([]);
        assert_eq!(
            TurnUsage::from_task(&task, &binding),
            TurnUsage::Unavailable
        );
    }
}
