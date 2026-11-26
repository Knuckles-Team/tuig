# Architecture

`tuig` is the sole Rust/Ratatui frontend. Tokio owns asynchronous requests;
Ratatui renders presentation state. Interactive and headless commands share
`ArboraClient`, task correlation, authentication and response validation.

```text
tuig -> Arbora A2A task submission and observation
     -> Arbora served operations for run inspection and logs
Arbora -> agent runtime, authorization and durable state
```

The current implementation uses A2A and served HTTP operations, not a remote
Agent Client Protocol endpoint. A future ACP connection requires a producer
contract and qualification; it is not enabled by renaming the CLI.

The terminal does not own agent decisions, model execution or tool effects.
Arbora is the one door: it handles admission, authentication and served operations.
Meristem runs the agent loop and Medulla holds the facts; Tuig reaches neither directly.
Unknown submission outcomes are not automatically replayed. Headless execution
never approves or cancels tasks. Interactive remote cancellation requires an
explicit confirmation. Existing credentials and authorization remain enforced.

The Python/Textual runtime has been removed. Native interactive composition,
streaming, approval attachment and durable resume remain open requirements,
not features supplied by a second frontend.

## Module organization

The library crate `tuig_native` declares one public module per responsibility in
`native/tuig/src/lib.rs`. The binary in `native/tuig/src/main.rs` owns the command
line and the terminal views.

| Module | Responsibility |
|---|---|
| `acp` | Experimental local headless turn over an owned backend child |
| `app` | Host terminal entry points re-exported for the binary |
| `coding_approval` | Approval requests, previews and fail-closed decisions |
| `correlation` | Task, context and run identity binding |
| `execution_graph` | Validated read of the run execution graph |
| `host_logs` | Host launch receipts and log reads |
| `operation_command` | Served operation invocation from the command line |
| `plan_confirm` | Plan confirmation and its explicit decision |
| `public_run_events` | Public run event reads |
| `recovery` | Resume after an unknown submission outcome |
| `run_inspector` | Read-only run inspector state |
| `run_inspector_client` | Served run reads for the inspector |
| `task_history` | Paged task history reads |
| `task_output` | Task output extraction and bounds |
| `task_stream` | Task observation transport |
| `terminal_event` | Terminal event model |
| `turn_usage` | Turn usage accounting |
