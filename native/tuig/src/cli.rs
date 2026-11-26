use std::ffi::OsString;

// contract: TUIG-RUNTIME-R002.1.3
pub fn arguments(args: &[OsString]) -> Result<(bool, Vec<OsString>), &'static str> {
    let mut headless = false;
    let mut remaining = Vec::new();
    let mut index = 0;
    while index < args.len() {
        if args[index] == "--headless" {
            if headless {
                return Err("--headless may be supplied only once");
            }
            headless = true;
        } else {
            remaining.push(args[index].clone());
            // A prompt or identifier is data, even when it looks like a flag.
            if args[index] == "--prompt"
                || args[index] == "--observe-task"
                || args[index] == "--continue"
            {
                index += 1;
                if let Some(value) = args.get(index) {
                    remaining.push(value.clone());
                }
            }
        }
        index += 1;
    }
    if headless && remaining.is_empty() {
        return Err("--headless requires --prompt TEXT or --observe-task TASK_ID");
    }
    Ok((headless, remaining))
}

// contract: TUIG-RUNTIME-R002.8.1
pub fn one_shot(args: &[OsString]) -> Result<bool, &'static str> {
    match args {
        [flag] if flag == "--one-shot" => Ok(true),
        [flag, ..] if flag == "--one-shot" => {
            Err("--one-shot reads its request from standard input and takes no other argument")
        }
        _ => Ok(false),
    }
}

pub fn continuation(
    args: &[OsString],
    task_selected: bool,
) -> Result<(Option<String>, Vec<OsString>), &'static str> {
    let mut context = None;
    let mut remaining = Vec::new();
    let mut index = 0;
    while index < args.len() {
        if args[index] == "--continue" {
            context = Some(continue_value(args, index, context, task_selected)?);
            index += 1; // the context value is consumed by continue_value
        } else {
            remaining.push(args[index].clone());
            if value_flag(&args[index]) {
                index += 1;
                if let Some(value) = args.get(index) {
                    remaining.push(value.clone());
                }
            }
        }
        index += 1;
    }
    if context.is_some() && !prompt_only(&remaining) {
        return Err("--continue requires exactly --prompt TEXT and cannot observe a task");
    }
    Ok((context, remaining))
}

fn value_flag(arg: &OsString) -> bool {
    arg == "--prompt" || arg == "--observe-task"
}

fn prompt_only(remaining: &[OsString]) -> bool {
    matches!(remaining, [flag, _] if flag == "--prompt")
}

fn continue_value(
    args: &[OsString],
    index: usize,
    context: Option<String>,
    task_selected: bool,
) -> Result<String, &'static str> {
    if context.is_some() || task_selected {
        return Err("--continue cannot be repeated or combined with AGENT_TASK_ID");
    }
    let value = args
        .get(index + 1)
        .and_then(|value| value.to_str())
        .ok_or("--continue requires a context identifier")?;
    tuig_native::validate_context_id(value)?;
    Ok(value.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    // spec: TUIG-RUNTIME-R012.1.3.1
    #[test]
    fn headless_is_explicit_and_preserves_argument_data() {
        for values in [
            vec!["--headless", "--prompt", "hello"],
            vec!["--prompt", "hello", "--headless"],
        ] {
            assert_eq!(
                arguments(&args(&values)),
                Ok((true, args(&["--prompt", "hello"])))
            );
        }
        assert_eq!(
            arguments(&args(&["--prompt", "--headless"])),
            Ok((false, args(&["--prompt", "--headless"])))
        );
        assert!(arguments(&args(&["--headless"])).is_err());
        assert!(arguments(&args(&["--headless", "--headless", "--prompt", "x"])).is_err());
    }
}

#[cfg(test)]
mod one_shot_tests {
    use super::*;

    // spec: TUIG-RUNTIME-R002.8.1
    #[test]
    fn one_shot_is_the_only_argument_and_never_inherits_override() {
        let args = |values: &[&str]| values.iter().map(OsString::from).collect::<Vec<_>>();
        assert_eq!(one_shot(&args(&["--one-shot"])), Ok(true));
        assert_eq!(one_shot(&args(&["--prompt", "--one-shot"])), Ok(false));
        assert_eq!(one_shot(&args(&[])), Ok(false));
        for values in [
            vec!["--one-shot", "--override"],
            vec!["--one-shot", "--prompt", "x"],
            vec!["--one-shot", "--headless"],
        ] {
            assert!(one_shot(&args(&values)).is_err());
        }
    }
}

#[cfg(test)]
mod continuation_tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    // spec: TUIG-RUNTIME-R012.3.2.1
    #[test]
    fn continuation_requires_one_canonical_context_and_new_prompt() {
        let context = format!("a2a-context-{}", "a".repeat(64));
        for input in [
            vec!["--continue", &context, "--prompt", "new text"],
            vec!["--prompt", "new text", "--continue", &context],
        ] {
            assert_eq!(
                continuation(&args(&input), false),
                Ok((Some(context.clone()), args(&["--prompt", "new text"])))
            );
            assert!(continuation(&args(&input), true).is_err());
        }
        for input in [
            vec!["--continue"],
            vec!["--continue", &context],
            vec!["--continue", "label", "--prompt", "text"],
            vec![
                "--continue",
                &context,
                "--continue",
                &context,
                "--prompt",
                "text",
            ],
            vec!["--continue", &context, "--observe-task", "task"],
            vec![
                "--continue",
                &context,
                "--prompt",
                "text",
                "--prompt",
                "again",
            ],
        ] {
            assert!(continuation(&args(&input), false).is_err());
        }
        assert_eq!(
            continuation(&args(&["--prompt", "--continue"]), false),
            Ok((None, args(&["--prompt", "--continue"])))
        );
        assert!(
            super::super::local_runtime::options(&args(&[
                "--local-runtime",
                "--local-model",
                "test:model",
                "--continue",
                &context,
                "--prompt",
                "text"
            ]))
            .is_err()
        );
        assert_eq!(
            arguments(&args(&["--continue", "--headless"])),
            Ok((false, args(&["--continue", "--headless"])))
        );
    }
}
