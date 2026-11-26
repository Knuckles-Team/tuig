//! One governed tool task submitted with `--one-shot`.
//!
//! The request is one JSON object read from standard input. Arbora owns task
//! admission and the preapproval scope receipt. This module only validates the
//! request shape, reads the producer's attestation back from the task metadata
//! and refuses to claim that a permission prompt was suppressed without it.
//! The metadata keys are a producer prerequisite recorded in plan.md.

use crate::{TaskSnapshot, correlation::TaskState};
use serde_json::{Map, Value};

pub const TOOL_TASK_EXTENSION: &str =
    "https://knuckles-team.github.io/arbora/extensions/tool-task/v1";
pub const TOOL_TASK_UNAVAILABLE: &str =
    "Governed tool-task submission is unavailable; nothing was submitted";
pub const MAX_REQUEST_BYTES: usize = 65_536;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OneShotRequest {
    operation: String,
    parameters: Map<String, Value>,
    idempotency_key: String,
    preapproval_ref: Option<String>,
}

fn token(value: &str, extra: &[char], lengths: std::ops::RangeInclusive<usize>) -> bool {
    lengths.contains(&value.len())
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || extra.contains(&c))
}

fn generated_key() -> Result<String, &'static str> {
    let mut identity = [0_u8; 16];
    getrandom::getrandom(&mut identity).map_err(|_| "Cannot generate submission identity")?;
    Ok(identity.iter().map(|byte| format!("{byte:02x}")).collect())
}

impl OneShotRequest {
    // contract: TUIG-RUNTIME-R002.8.1
    pub fn parse(input: &[u8]) -> Result<Self, &'static str> {
        if input.len() > MAX_REQUEST_BYTES {
            return Err("One-shot request must be at most 65536 bytes");
        }
        let text = std::str::from_utf8(input).map_err(|_| "One-shot request must be UTF-8")?;
        let Value::Object(mut fields) = serde_json::from_str::<Value>(text)
            .map_err(|_| "One-shot request must be one JSON object")?
        else {
            return Err("One-shot request must be one JSON object");
        };
        let operation = Self::text_field(&mut fields, "operation")?
            .filter(|value| token(value, &['.', '_', '-'], 1..=128))
            .ok_or("One-shot request needs a registered operation identifier")?;
        let Some(Value::Object(parameters)) = fields.remove("parameters") else {
            return Err("One-shot request needs a parameters object");
        };
        let idempotency_key = match Self::text_field(&mut fields, "idempotency_key")? {
            Some(key) if token(&key, &['_', '-'], 16..=64) => key,
            Some(_) => return Err("One-shot idempotency key must be 16 to 64 safe characters"),
            None => generated_key()?,
        };
        let preapproval_ref = Self::text_field(&mut fields, "preapproval_ref")?;
        if preapproval_ref
            .as_deref()
            .is_some_and(|value| !token(value, &['.', '_', '-', ':'], 8..=256))
        {
            return Err("One-shot preapproval reference is malformed");
        }
        if !fields.is_empty() {
            return Err("One-shot request has an unknown field");
        }
        Ok(Self {
            operation,
            parameters,
            idempotency_key,
            preapproval_ref,
        })
    }

    fn text_field(
        fields: &mut Map<String, Value>,
        name: &str,
    ) -> Result<Option<String>, &'static str> {
        match fields.remove(name) {
            None => Ok(None),
            Some(Value::String(value)) => Ok(Some(value)),
            Some(_) => Err("One-shot request field has the wrong type"),
        }
    }

    pub fn operation(&self) -> &str {
        &self.operation
    }

    pub fn idempotency_key(&self) -> &str {
        &self.idempotency_key
    }

    pub fn preapproval_ref(&self) -> Option<&str> {
        self.preapproval_ref.as_deref()
    }

    /// The typed part that Arbora admits as one governed tool task.
    pub fn message_part(&self) -> Value {
        let mut tool_task = serde_json::json!({
            "operation": self.operation,
            "parameters": self.parameters,
        });
        if let Some(reference) = &self.preapproval_ref {
            tool_task["preapprovalRef"] = Value::String(reference.clone());
        }
        serde_json::json!({"kind": "data", "data": {"toolTask": tool_task}})
    }
}

/// What the producer reported about the preapproval bound to a task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreapprovalClaim {
    reference: String,
    status: String,
    task_id: String,
    expires_at_ms: u64,
}

/// Producer-owned tool-task facts read from the task metadata.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolTaskMetadata {
    preapproval: Option<PreapprovalClaim>,
    admission_receipt: Option<String>,
    backend_effect_count: Option<u64>,
}

fn text(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty() && !text.chars().any(char::is_control))
        .map(str::to_owned)
}

impl ToolTaskMetadata {
    pub fn from_task(task: &Value) -> Self {
        let Some(arbora) = task.pointer("/metadata/arbora") else {
            return Self::default();
        };
        let preapproval = arbora.get("preapproval").and_then(|claim| {
            Some(PreapprovalClaim {
                reference: text(claim, "ref")?,
                status: text(claim, "status")?,
                task_id: text(claim, "taskId")?,
                expires_at_ms: claim.get("expiresAtMs").and_then(Value::as_u64)?,
            })
        });
        Self {
            preapproval,
            admission_receipt: text(arbora, "admissionReceipt"),
            backend_effect_count: arbora.get("backendEffectCount").and_then(Value::as_u64),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Attestation {
    NotRequested,
    Unavailable,
    Attested,
    Refused(&'static str),
}

impl Attestation {
    // contract: TUIG-RUNTIME-R002.8.4
    pub fn evaluate(request: &OneShotRequest, snapshot: &TaskSnapshot, now_ms: u64) -> Self {
        let claim = snapshot.tool_task.preapproval.as_ref();
        let Some(reference) = request.preapproval_ref() else {
            return match claim {
                Some(_) => {
                    Self::Refused("a preapproval was attested that this request did not name")
                }
                None => Self::NotRequested,
            };
        };
        let Some(claim) = claim else {
            return Self::Unavailable;
        };
        if claim.reference != reference {
            Self::Refused("the attested preapproval is not the requested one")
        } else if claim.task_id != snapshot.correlation.task_id() {
            Self::Refused("the preapproval is bound to another task")
        } else if claim.status != "attested" {
            Self::Refused("the preapproval is revoked or not attested")
        } else if claim.expires_at_ms <= now_ms {
            Self::Refused("the preapproval expired")
        } else {
            Self::Attested
        }
    }

    pub fn text(&self) -> String {
        match self {
            Self::NotRequested => {
                "Preapproval: not requested; the permission prompt applies.".into()
            }
            Self::Unavailable => {
                "Preapproval: attestation unavailable; the permission prompt is retained.".into()
            }
            Self::Attested => "Preapproval: attested by Arbora for this task.".into(),
            Self::Refused(reason) => {
                format!("Preapproval: refused ({reason}); the permission prompt is retained.")
            }
        }
    }
}

impl TaskState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Submitted => "submitted",
            Self::Working => "working",
            Self::InputRequired => "input-required",
            Self::AuthRequired => "auth-required",
            Self::Completed => "completed",
            Self::Canceled => "canceled",
            Self::Failed => "failed",
            Self::Rejected => "rejected",
        }
    }
}

fn count(value: Option<u64>) -> String {
    value.map_or_else(|| "unavailable".to_owned(), |count| count.to_string())
}

// contract: TUIG-RUNTIME-R002.8.5
pub fn evidence(
    request: &OneShotRequest,
    snapshot: &TaskSnapshot,
    attestation: &Attestation,
) -> String {
    let metadata = &snapshot.tool_task;
    format!(
        "One-shot evidence\nSchema: tool-task/v1\nClient: tuig {}\nOperation: {}\nIdempotency key: {}\nTask ID: {}\nContext ID: {}\nRun ID: {}\nOutcome: {}\nEvent resume position: unavailable\nRequest count: 1\nBackend effect count: {}\nAdmission receipt: {}\n{}",
        env!("CARGO_PKG_VERSION"),
        request.operation(),
        request.idempotency_key(),
        snapshot.correlation.task_id(),
        snapshot.correlation.context_id(),
        snapshot.correlation.run_id().unwrap_or("unavailable"),
        snapshot.state.label(),
        count(metadata.backend_effect_count),
        metadata
            .admission_receipt
            .as_deref()
            .unwrap_or("unavailable"),
        attestation.text(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::correlation::TaskCorrelation;
    use serde_json::json;

    const KEY: &str = "one-shot-key-0123456789";

    fn request(extra: Value) -> Result<OneShotRequest, &'static str> {
        let mut body = json!({
            "operation": "records.write",
            "parameters": {"name": "alpha"},
            "idempotency_key": KEY,
            "preapproval_ref": "scope:receipt-0001",
        });
        for (key, value) in extra.as_object().unwrap() {
            body[key] = value.clone();
        }
        OneShotRequest::parse(body.to_string().as_bytes())
    }

    fn snapshot(preapproval: Option<Value>) -> TaskSnapshot {
        let mut arbora = json!({"admissionReceipt": "admit-7", "backendEffectCount": 1});
        if let Some(claim) = preapproval {
            arbora["preapproval"] = claim;
        }
        let task = json!({
            "kind": "task", "id": "task-1", "contextId": "context-1",
            "metadata": {"arbora": arbora}, "status": {"state": "completed"},
        });
        TaskSnapshot {
            correlation: TaskCorrelation::from_task(&task).unwrap(),
            state: TaskState::Completed,
            terminal_event: Default::default(),
            output: Default::default(),
            usage: Default::default(),
            plan: None,
            tool_task: ToolTaskMetadata::from_task(&task),
        }
    }

    fn claim(changes: Value) -> Value {
        let mut claim = json!({
            "ref": "scope:receipt-0001", "status": "attested",
            "taskId": "task-1", "expiresAtMs": 2_000,
        });
        for (key, value) in changes.as_object().unwrap() {
            claim[key] = value.clone();
        }
        claim
    }

    // spec: TUIG-RUNTIME-R002.8.1
    #[test]
    fn the_request_model_accepts_one_typed_object_and_generates_a_missing_key() {
        let parsed = request(json!({})).unwrap();
        assert_eq!(parsed.operation(), "records.write");
        assert_eq!(parsed.idempotency_key(), KEY);
        assert_eq!(parsed.preapproval_ref(), Some("scope:receipt-0001"));
        let bare = OneShotRequest::parse(br#"{"operation":"a.b","parameters":{}}"#).unwrap();
        assert_eq!(bare.idempotency_key().len(), 32);
        assert_eq!(bare.preapproval_ref(), None);
        assert_ne!(
            bare.idempotency_key(),
            OneShotRequest::parse(br#"{"operation":"a.b","parameters":{}}"#)
                .unwrap()
                .idempotency_key()
        );
    }

    // spec: TUIG-RUNTIME-R002.8.1
    #[test]
    fn malformed_requests_are_refused_before_submission() {
        for extra in [
            json!({"operation": ""}),
            json!({"operation": "has space"}),
            json!({"operation": 4}),
            json!({"parameters": []}),
            json!({"idempotency_key": "short"}),
            json!({"preapproval_ref": "x"}),
            json!({"task_id": "server-owned"}),
        ] {
            assert!(request(extra.clone()).is_err(), "{extra}");
        }
        let oversized = vec![b' '; 70_000];
        let inputs: [&[u8]; 5] = [b"", b"[]", b"not json", b"\xff", &oversized];
        for raw in inputs {
            assert!(OneShotRequest::parse(raw).is_err());
        }
    }

    // spec: TUIG-RUNTIME-R002.8.3
    #[test]
    fn the_message_part_carries_one_typed_tool_task() {
        let part = request(json!({})).unwrap().message_part();
        assert_eq!(part["kind"], "data");
        assert_eq!(part["data"]["toolTask"]["operation"], "records.write");
        assert_eq!(
            part["data"]["toolTask"]["preapprovalRef"],
            "scope:receipt-0001"
        );
        let without = request(json!({"preapproval_ref": null}));
        assert!(without.is_err());
        let bare = OneShotRequest::parse(br#"{"operation":"a.b","parameters":{}}"#).unwrap();
        assert!(
            bare.message_part()["data"]["toolTask"]
                .get("preapprovalRef")
                .is_none()
        );
    }

    // spec: TUIG-RUNTIME-R002.8.4
    #[test]
    fn only_a_matching_current_attestation_is_reported() {
        let request = request(json!({})).unwrap();
        let evaluate =
            |claim: Option<Value>| Attestation::evaluate(&request, &snapshot(claim), 1_000);
        assert_eq!(evaluate(Some(claim(json!({})))), Attestation::Attested);
        assert_eq!(evaluate(None), Attestation::Unavailable);
        for (changes, reason) in [
            (
                json!({"status": "revoked"}),
                "the preapproval is revoked or not attested",
            ),
            (json!({"expiresAtMs": 1_000}), "the preapproval expired"),
            (
                json!({"taskId": "task-2"}),
                "the preapproval is bound to another task",
            ),
            (
                json!({"ref": "scope:foreign-0001"}),
                "the attested preapproval is not the requested one",
            ),
        ] {
            assert_eq!(evaluate(Some(claim(changes))), Attestation::Refused(reason));
        }
        let malformed = json!({"ref": "scope:receipt-0001", "status": "attested"});
        assert_eq!(evaluate(Some(malformed)), Attestation::Unavailable);
        let unrequested = OneShotRequest::parse(br#"{"operation":"a.b","parameters":{}}"#).unwrap();
        assert_eq!(
            Attestation::evaluate(&unrequested, &snapshot(None), 1_000),
            Attestation::NotRequested
        );
        assert!(matches!(
            Attestation::evaluate(&unrequested, &snapshot(Some(claim(json!({})))), 1_000),
            Attestation::Refused(_)
        ));
    }

    // spec: TUIG-RUNTIME-R002.8.5
    #[test]
    fn evidence_names_identifiers_and_marks_missing_fields_unavailable() {
        let request = request(json!({})).unwrap();
        let attested = snapshot(Some(claim(json!({}))));
        let text = evidence(&request, &attested, &Attestation::Attested);
        for expected in [
            "Schema: tool-task/v1",
            "Idempotency key: one-shot-key-0123456789",
            "Task ID: task-1",
            "Context ID: context-1",
            "Run ID: unavailable",
            "Outcome: completed",
            "Event resume position: unavailable",
            "Request count: 1",
            "Backend effect count: 1",
            "Admission receipt: admit-7",
        ] {
            assert!(text.contains(expected), "{expected}");
        }
        assert!(!text.contains("alpha"));
        assert!(!text.contains("scope:receipt-0001"));
        let bare = evidence(&request, &snapshot(None), &Attestation::Unavailable);
        assert!(bare.contains("attestation unavailable"));
    }
}
