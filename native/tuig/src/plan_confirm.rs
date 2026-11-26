//! Plan confirmation: one /confirm resumes only the plan Arbora issued.
//!
//! The terminal never grants the effect locally. It re-submits the plan's
//! reference, subject, operation, parameters digest, registry digest and
//! lease through the A2A transport; Arbora alone authorizes the effect.
//! Structural and expiry checks fail closed here, before any effect
//! request, and the server re-checks every field and the lease, so a
//! tampered, expired or replayed confirmation is refused.

use crate::{ArboraClient, AuthMode, TaskCorrelation, TaskSnapshot, parse_returned_task};
use serde_json::Value;

const MAX_PLAN_FIELD: usize = 512;
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

/// The plan Arbora issued for a task, as carried in the task's metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanBinding {
    reference: String,
    subject: String,
    op: String,
    params_digest: String,
    registry_digest: String,
    expires_at_ms: u64,
    lease: String,
}

impl PlanBinding {
    /// Parse the issued plan from a task's metadata; absent or malformed
    /// plans are refused, so there is nothing to confirm.
    pub fn from_task(task: &Value) -> Result<Self, &'static str> {
        let plan = task
            .pointer("/metadata/arbora/plan")
            .and_then(Value::as_object)
            .ok_or("No issued plan in the task")?;
        Ok(Self {
            reference: identifier(plan.get("reference"))?,
            subject: identifier(plan.get("subject"))?,
            op: identifier(plan.get("op"))?,
            params_digest: digest(plan.get("paramsDigest"))?,
            registry_digest: digest(plan.get("registryDigest"))?,
            expires_at_ms: plan
                .get("expiresAt")
                .and_then(Value::as_u64)
                .filter(|expires| *expires > 0)
                .ok_or("Invalid plan expiry")?,
            lease: digest(plan.get("lease"))?,
        })
    }

    pub fn reference(&self) -> &str {
        &self.reference
    }

    pub fn subject(&self) -> &str {
        &self.subject
    }

    pub fn op(&self) -> &str {
        &self.op
    }

    pub fn params_digest(&self) -> &str {
        &self.params_digest
    }

    pub fn registry_digest(&self) -> &str {
        &self.registry_digest
    }

    pub fn expires_at_ms(&self) -> u64 {
        self.expires_at_ms
    }

    pub fn lease(&self) -> &str {
        &self.lease
    }

    /// A plan whose expiry is reached or past can no longer be resumed.
    pub fn is_expired(&self, now: u64) -> bool {
        self.expires_at_ms <= now
    }
}

fn identifier(value: Option<&Value>) -> Result<String, &'static str> {
    let text = value
        .and_then(Value::as_str)
        .filter(|field| {
            !field.trim().is_empty()
                && field.chars().count() <= MAX_PLAN_FIELD
                && !field.chars().any(char::is_control)
        })
        .ok_or("Invalid plan field")?;
    Ok(text.to_owned())
}

fn digest(value: Option<&Value>) -> Result<String, &'static str> {
    let text = value
        .and_then(Value::as_str)
        .filter(|field| field.len() == 64 && field.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .ok_or("Invalid plan digest")?;
    Ok(text.to_owned())
}

fn plan_request_id() -> Result<String, &'static str> {
    let mut identity = [0_u8; 16];
    getrandom::getrandom(&mut identity)
        .map_err(|_| "Cannot generate a plan confirmation identity")?;
    let id = identity
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(format!("plan-confirm-{id}"))
}

impl ArboraClient {
    /// Resume exactly the plan Arbora issued for this task's binding.
    ///
    /// The expiry is checked before the request goes out; the server
    /// re-checks every field and the single-use lease, so a tampered or
    /// replayed confirmation is refused before any effect.
    pub async fn confirm_plan(
        &self,
        binding: &TaskCorrelation,
        plan: &PlanBinding,
    ) -> Result<TaskSnapshot, &'static str> {
        if !self.authenticated && self.auth_mode != AuthMode::None {
            return Err("Plan confirmation requires a caller credential");
        }
        if plan.is_expired(self.now_ms()) {
            return Err("Plan expired; no effect was requested");
        }
        let id = plan_request_id()?;
        let mut response = self.send_confirmation(&id, binding, plan).await?;
        let bytes = read_confirm_response(&mut response).await?;
        let snapshot = parse_returned_task(&id, &bytes)
            .map_err(|_| "Invalid or refused Arbora plan confirmation response")?;
        if binding.validate_binding(&snapshot.correlation).is_err() {
            return Err("Task correlation mismatch");
        }
        Ok(snapshot)
    }

    async fn send_confirmation(
        &self,
        id: &str,
        binding: &TaskCorrelation,
        plan: &PlanBinding,
    ) -> Result<reqwest::Response, &'static str> {
        let message = plan_confirm_message(id, binding, plan);
        let mut url = self.card_url.clone();
        url.set_path("/a2a");
        self.http
            .post(url)
            .json(&serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": "message/send",
                "params": {"configuration": {"blocking": false}, "message": message},
            }))
            .send()
            .await
            .map_err(|_| "Plan confirmation request could not be sent")
    }
}

fn plan_confirm_message(
    id: &str,
    binding: &TaskCorrelation,
    plan: &PlanBinding,
) -> serde_json::Value {
    serde_json::json!({
        "kind": "message",
        "role": "user",
        "messageId": id,
        "contextId": binding.context_id(),
        "parts": [{
            "kind": "data",
            "data": {
                "planConfirm": {
                    "reference": plan.reference(),
                    "subject": plan.subject(),
                    "op": plan.op(),
                    "paramsDigest": plan.params_digest(),
                    "registryDigest": plan.registry_digest(),
                    "lease": plan.lease(),
                }
            }
        }],
    })
}

async fn read_confirm_response(response: &mut reqwest::Response) -> Result<Vec<u8>, &'static str> {
    if !response.status().is_success() {
        return Err("Arbora plan confirmation refused or unavailable");
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "Arbora plan confirmation response interrupted")?
    {
        if chunk.len() > MAX_RESPONSE_BYTES.saturating_sub(bytes.len()) {
            return Err("Arbora plan confirmation response exceeds size limit");
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn issued(reference: &str) -> Value {
        json!({
            "kind": "task",
            "id": "task-1",
            "contextId": "context-1",
            "metadata": {"arbora": {"plan": {
                "reference": reference,
                "subject": "subject-1",
                "op": "ops.example/read",
                "paramsDigest": "a".repeat(64),
                "registryDigest": "b".repeat(64),
                "expiresAt": 4_102_444_800_000u64,
                "lease": "c".repeat(64),
            }}},
            "status": {"state": "input-required"},
        })
    }

    // spec: TUIG-RUNTIME-R001.3
    #[test]
    fn issued_plan_fields_parse_and_malformed_fields_refuse() {
        let plan = PlanBinding::from_task(&issued("plan-1")).unwrap();
        assert_eq!(plan.reference(), "plan-1");
        assert_eq!(plan.subject(), "subject-1");
        assert_eq!(plan.op(), "ops.example/read");
        assert_eq!(plan.params_digest(), "a".repeat(64));
        assert_eq!(plan.registry_digest(), "b".repeat(64));
        assert_eq!(plan.lease(), "c".repeat(64));
        assert!(!plan.is_expired(0));
        assert!(plan.is_expired(plan.expires_at_ms()));
        assert!(plan.is_expired(plan.expires_at_ms() + 1));

        let mut plan_value = issued("plan-1");
        plan_value["metadata"]["arbora"]["plan"]["paramsDigest"] = json!("a".repeat(63));
        assert!(PlanBinding::from_task(&plan_value).is_err());
        plan_value["metadata"]["arbora"]["plan"]["paramsDigest"] = json!("a".repeat(64) + "z");
        assert!(PlanBinding::from_task(&plan_value).is_err());
        plan_value["metadata"]["arbora"]["plan"]["paramsDigest"] = json!("a".repeat(64));
        plan_value["metadata"]["arbora"]["plan"]["op"] = json!("ops.example/read\x01");
        assert!(PlanBinding::from_task(&plan_value).is_err());
        plan_value["metadata"]["arbora"]["plan"]["expiresAt"] = json!(0);
        assert!(PlanBinding::from_task(&plan_value).is_err());
        plan_value["metadata"]["arbora"]["plan"]["subject"] = json!("  ");
        assert!(PlanBinding::from_task(&plan_value).is_err());
        plan_value["metadata"]["arbora"]["plan"]["lease"] = json!("c".repeat(64));
        plan_value["metadata"]["arbora"]["plan"]["subject"] = json!("subject-1");
        plan_value["metadata"]["arbora"]["plan"]["op"] = json!("ops.example/read");
        plan_value["metadata"]["arbora"]["plan"]["expiresAt"] = json!(4_102_444_800_000u64);
        plan_value["metadata"]["arbora"]["plan"]
            .as_object_mut()
            .unwrap()
            .remove("lease");
        assert!(PlanBinding::from_task(&plan_value).is_err());
    }

    // spec: TUIG-RUNTIME-R001.3
    #[test]
    fn a_task_without_a_parseable_plan_has_nothing_to_confirm() {
        let bare = json!({
            "kind": "task", "id": "t", "contextId": "c",
            "status": {"state": "working"},
        });
        assert!(PlanBinding::from_task(&bare).is_err());
        let not_an_object = json!({"metadata": {"arbora": {"plan": "issued"}}});
        assert!(PlanBinding::from_task(&not_an_object).is_err());
        let empty_reference = json!({
            "metadata": {"arbora": {"plan": {
                "reference": "", "subject": "s", "op": "o",
                "paramsDigest": "a".repeat(64), "registryDigest": "b".repeat(64),
                "expiresAt": 1, "lease": "c".repeat(64),
            }}},
        });
        assert!(PlanBinding::from_task(&empty_reference).is_err());
    }
}
