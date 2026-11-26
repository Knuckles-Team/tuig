use std::{io::Write, time::Duration};
use tuig_native::{
    ArboraClient, TaskSnapshot, TaskSubmission,
    correlation::{TaskCorrelation, TaskState},
    one_shot::{Attestation, OneShotRequest, evidence},
};

fn result(state: TaskState) -> Result<bool, &'static str> {
    match state {
        TaskState::Submitted | TaskState::Working => Ok(false),
        TaskState::Completed => Ok(true),
        TaskState::InputRequired => Err("Task requires input or approval; no approval was sent"),
        TaskState::AuthRequired => Err("Task requires authentication; no credentials were changed"),
        TaskState::Canceled | TaskState::Failed | TaskState::Rejected => {
            Err("Task did not complete successfully")
        }
    }
}

pub async fn run(
    client: &ArboraClient,
    task_id: Option<&str>,
    submission: Option<&TaskSubmission>,
) -> Result<(), &'static str> {
    tokio::time::timeout(
        Duration::from_secs(4 * 60 * 60),
        observe(client, task_id, submission),
    )
    .await
    .map_err(|_| "Observation timed out. No remote task was canceled.")?
}

async fn observe(
    client: &ArboraClient,
    task_id: Option<&str>,
    submission: Option<&TaskSubmission>,
) -> Result<(), &'static str> {
    let mut snapshot = start(client, task_id, submission).await?;
    let binding = snapshot.correlation.clone();
    let mut previous = String::new();
    loop {
        check_binding(&snapshot, &binding)?;
        if finished(&mut previous, &snapshot)? {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
        snapshot = client.refresh_task(&binding).await?;
    }
}

async fn start(
    client: &ArboraClient,
    task_id: Option<&str>,
    submission: Option<&TaskSubmission>,
) -> Result<TaskSnapshot, &'static str> {
    match submission {
        Some(submission) => client.submit_task(submission).await,
        None => {
            client
                .get_task(task_id.ok_or("Missing task identifier")?)
                .await
        }
    }
}

// contract: TUIG-RUNTIME-R002.8.2
pub async fn one_shot(client: &ArboraClient, request: &OneShotRequest) -> Result<(), &'static str> {
    let submission = TaskSubmission::tool_task(request);
    tokio::select! {
        biased;
        _ = tokio::signal::ctrl_c() => Err("Detached from the one-shot task; no remote task was canceled."),
        result = tokio::time::timeout(
            Duration::from_secs(4 * 60 * 60),
            observe_one_shot(client, request, &submission),
        ) => result.map_err(|_| "Observation timed out. No remote task was canceled.")?,
    }
}

async fn observe_one_shot(
    client: &ArboraClient,
    request: &OneShotRequest,
    submission: &TaskSubmission,
) -> Result<(), &'static str> {
    let mut snapshot = client.submit_task(submission).await?;
    let binding = snapshot.correlation.clone();
    let mut previous = String::new();
    loop {
        check_binding(&snapshot, &binding)?;
        let attestation = Attestation::evaluate(request, &snapshot, client.now_ms());
        let display = format!(
            "{}\n{}",
            super::snapshot_status(&snapshot),
            attestation.text()
        );
        if display != previous {
            write_status(&display)?;
            previous = display;
        }
        let outcome = result(snapshot.state);
        if outcome != Ok(false) {
            write_status(&evidence(request, &snapshot, &attestation))?;
            return outcome.map(|_| ());
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
        snapshot = client.refresh_task(&binding).await?;
    }
}

fn check_binding(snapshot: &TaskSnapshot, binding: &TaskCorrelation) -> Result<(), &'static str> {
    binding.validate_binding(&snapshot.correlation)?;
    snapshot.terminal_event.validate_binding(binding)
}

fn finished(previous: &mut String, snapshot: &TaskSnapshot) -> Result<bool, &'static str> {
    let display = super::snapshot_status(snapshot);
    if display != *previous {
        write_status(&display)?;
        *previous = display;
    }
    result(snapshot.state)
}

fn write_status(display: &str) -> Result<(), &'static str> {
    let mut output = std::io::stdout().lock();
    writeln!(output, "{display}").map_err(|_| "Cannot write task output")?;
    output.flush().map_err(|_| "Cannot flush task output")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::arguments;
    use std::ffi::OsString;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    // spec: TUIG-RUNTIME-R012.1.3.1
    #[test]
    fn unsupported_flags_and_turn_combinations_are_rejected() {
        for values in [
            vec!["--headless", "--prompt"],
            vec!["--headless", "--prompt", "x", "--override"],
            vec!["--headless", "--prompt", "x", "--bg"],
            vec!["--headless", "--prompt", "x", "--model", "model"],
            vec!["--headless", "--prompt", "x", "--session", "context"],
            vec!["--headless", "--prompt", "x", "--observe-task", "task"],
        ] {
            let (_, remaining) = arguments(&args(&values)).unwrap();
            assert!(super::super::selected_prompt(&remaining, false).is_err());
        }
        let (_, remaining) = arguments(&args(&[
            "--headless",
            "--observe-task",
            "task",
            "--override",
        ]))
        .unwrap();
        assert!(super::super::selected_observation(&remaining, false).is_err());
    }

    // spec: TUIG-RUNTIME-R012.1.3.1
    #[test]
    fn only_completed_is_success_and_approvals_fail_closed() {
        assert_eq!(result(TaskState::Completed), Ok(true));
        assert_eq!(result(TaskState::Submitted), Ok(false));
        assert_eq!(result(TaskState::Working), Ok(false));
        for state in [
            TaskState::InputRequired,
            TaskState::AuthRequired,
            TaskState::Canceled,
            TaskState::Failed,
            TaskState::Rejected,
        ] {
            assert!(result(state).is_err());
        }
    }
}
