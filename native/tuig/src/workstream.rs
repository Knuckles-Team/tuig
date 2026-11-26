//! Workstream peer: a coding session attaches to the caller's workstream,
//! shows what the other sessions left, and writes one checkpoint at exit.
//! The terminal holds no workstream state beyond its own session identity;
//! a refused or unserved call is shown and never blocks the session.

use crate::{ArboraClient, run_inspector::ReadRefusal};
use serde_json::{Value, json};

/// The context shown above the first prompt is at most this many bytes.
pub const CONTEXT_LIMIT: usize = 4096;
const HARNESS: &str = "tuig";

/// One attached coding session of this terminal.
pub struct Workstream {
    session_id: String,
}

impl Workstream {
    pub fn session_id(&self) -> &str {
        &self.session_id
    }
}

/// Bound a served context to printable text of at most [`CONTEXT_LIMIT`] bytes.
pub fn context_text(value: &Value) -> String {
    let mut text = String::new();
    let served = value.get("context").and_then(Value::as_str).unwrap_or("");
    for c in served.chars().filter(|c| !c.is_control() || *c == '\n') {
        if text.len() + c.len_utf8() > CONTEXT_LIMIT {
            break;
        }
        text.push(c);
    }
    text
}

fn refusal_notice(operation: &str, refusal: &ReadRefusal) -> String {
    let cause = match refusal {
        ReadRefusal::Denied => "refused this caller",
        ReadRefusal::Timeout => "did not answer in time",
        ReadRefusal::Unavailable | ReadRefusal::WrongRun => "does not serve it to this caller",
    };
    format!("Workstream unavailable: Arbora {cause}; operation {operation}.")
}

fn session_identity() -> Option<String> {
    let mut bytes = [0_u8; 16];
    getrandom::getrandom(&mut bytes).ok()?;
    Some(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

impl ArboraClient {
    // contract: TUIG-RUNTIME-R013.1
    pub async fn workstream_attach(
        &self,
        session_id: &str,
        workspace_root: &str,
    ) -> Result<String, ReadRefusal> {
        let payload = json!({
            "harness": HARNESS,
            "harness_session_id": session_id,
            "workspace_root": workspace_root,
        });
        let value = self
            .invoke_operation("workstream.attach", &payload.to_string())
            .await?;
        Ok(context_text(&value))
    }

    pub async fn workstream_close(&self, session_id: &str) -> Result<(), ReadRefusal> {
        let checkpoint = json!({
            "harness_session_id": session_id,
            "summary": "Terminal session ended.",
            "run_ids": [],
            "trigger": "exit",
        });
        self.invoke_operation("workstream.checkpoint", &checkpoint.to_string())
            .await?;
        let detach = json!({"harness_session_id": session_id});
        self.invoke_operation("workstream.detach", &detach.to_string())
            .await
            .map(|_| ())
    }
}

/// Attach and return the session with the text to show above the first prompt.
pub async fn open(client: &ArboraClient, workspace_root: &str) -> (Option<Workstream>, String) {
    let Some(session_id) = session_identity() else {
        return (
            None,
            "Workstream unavailable: no session identity.".to_owned(),
        );
    };
    match client.workstream_attach(&session_id, workspace_root).await {
        Ok(context) => (Some(Workstream { session_id }), context),
        Err(refusal) => (None, refusal_notice("workstream.attach", &refusal)),
    }
}

/// Write the exit checkpoint and detach; the notice is empty on success.
pub async fn close(client: &ArboraClient, stream: Workstream) -> String {
    match client.workstream_close(stream.session_id()).await {
        Ok(()) => String::new(),
        Err(refusal) => refusal_notice("workstream.checkpoint", &refusal),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_is_bounded_printable_and_drops_controls() {
        assert_eq!(context_text(&json!({})), "");
        assert_eq!(context_text(&json!({"context": "a\u{1b}b\nc"})), "ab\nc");
        let long = "é".repeat(5000);
        let bounded = context_text(&json!({ "context": long }));
        assert!(bounded.len() <= CONTEXT_LIMIT && bounded.len() >= CONTEXT_LIMIT - 1);
    }

    #[test]
    fn refusals_name_the_operation_and_the_cause() {
        for (refusal, words) in [
            (ReadRefusal::Denied, "refused"),
            (ReadRefusal::Timeout, "in time"),
            (ReadRefusal::Unavailable, "does not serve"),
        ] {
            let notice = refusal_notice("workstream.attach", &refusal);
            assert!(notice.contains(words) && notice.contains("workstream.attach"));
        }
    }

    #[test]
    fn each_session_identity_is_distinct_hex() {
        let (a, b) = (session_identity().unwrap(), session_identity().unwrap());
        assert_ne!(a, b);
        assert!(a.len() == 32 && a.bytes().all(|byte| byte.is_ascii_hexdigit()));
    }
}
