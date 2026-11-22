//! `/op <operation-id> <JSON object>` command parsing.
//!
//! The command is split at the first whitespace run; the remainder must be
//! one JSON object that reaches the generated registry client unchanged.
//! // spec: TUIG-RUNTIME-R001.1

use serde_json::Value;

pub const COMMAND: &str = "/op";

#[derive(Clone, Debug)]
pub struct OperationCommand {
    operation: String,
    payload: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperationCommandError {
    InvalidOperation,
    MalformedJson,
    NotAnObject,
}

impl OperationCommand {
    /// Recognize a composed draft as an operation command.
    pub fn parse_command(text: &str) -> Option<Self> {
        let rest = text.strip_prefix(COMMAND)?;
        if !rest.starts_with(|c: char| c.is_whitespace()) {
            return None;
        }
        Self::parse(rest).ok()
    }

    /// Split at the first whitespace run; the remainder stays unchanged.
    pub fn parse(input: &str) -> Result<Self, OperationCommandError> {
        let input = input.trim_start();
        if input.is_empty() {
            return Err(OperationCommandError::InvalidOperation);
        }
        let split = input
            .find(char::is_whitespace)
            .ok_or(OperationCommandError::MalformedJson)?;
        let operation = input[..split].to_owned();
        if operation.is_empty() || operation.len() > 512 || operation.chars().any(char::is_control)
        {
            return Err(OperationCommandError::InvalidOperation);
        }
        let payload = input[split..].trim_start().trim_end().to_owned();
        let parsed: Value =
            serde_json::from_str(&payload).map_err(|_| OperationCommandError::MalformedJson)?;
        if !parsed.is_object() {
            return Err(OperationCommandError::NotAnObject);
        }
        Ok(Self { operation, payload })
    }

    pub fn operation(&self) -> &str {
        &self.operation
    }

    pub fn payload(&self) -> &str {
        &self.payload
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: TUIG-RUNTIME-R001.1
    #[test]
    fn splits_the_command_and_keeps_the_payload_unchanged() {
        let command = OperationCommand::parse_command(
            "/op runs.get   {\"run_id\":\"r1\", \"nested\":{\"a\":[1,2]}}",
        )
        .unwrap();
        assert_eq!(command.operation(), "runs.get");
        assert_eq!(
            command.payload(),
            "{\"run_id\":\"r1\", \"nested\":{\"a\":[1,2]}}"
        );
        let parsed: Value = serde_json::from_str(command.payload()).unwrap();
        assert_eq!(parsed["nested"]["a"][1], 2);
    }

    // spec: TUIG-RUNTIME-R001.1
    #[test]
    fn refusal_cases_parse_to_a_typed_error() {
        assert_eq!(
            OperationCommand::parse("   ").err(),
            Some(OperationCommandError::InvalidOperation)
        );
        assert_eq!(
            OperationCommand::parse("runs.get").err(),
            Some(OperationCommandError::MalformedJson)
        );
        for (input, expected) in [
            (" runs.get [1,2]", OperationCommandError::NotAnObject),
            (" runs.get \"text\"", OperationCommandError::NotAnObject),
            (" runs.get 42", OperationCommandError::NotAnObject),
            (" runs.get null", OperationCommandError::NotAnObject),
            (" runs.get true", OperationCommandError::NotAnObject),
            (
                " runs.get {\"a\":NaN}",
                OperationCommandError::MalformedJson,
            ),
            (
                " runs.get {\"nested\":{\"b\":Infinity}}",
                OperationCommandError::MalformedJson,
            ),
            (" runs.get {a:1}", OperationCommandError::MalformedJson),
            (" runs.get ", OperationCommandError::MalformedJson),
            (
                " runs.with\u{1b}.control {}",
                OperationCommandError::InvalidOperation,
            ),
            (
                &format!(" {} {{}}", "o".repeat(513)),
                OperationCommandError::InvalidOperation,
            ),
        ] {
            assert_eq!(
                OperationCommand::parse(input).err(),
                Some(expected),
                "{input:?}"
            );
        }
    }

    // spec: TUIG-RUNTIME-R001.1
    #[test]
    fn non_operation_drafts_are_not_commands() {
        for text in [
            "/open a file",
            "/opt runs.get {}",
            "I typed /op once",
            "",
            "/op",
            "/op\t",
        ] {
            assert!(OperationCommand::parse_command(text).is_none(), "{text:?}");
        }
    }
}
