# TUIG-RUNTIME — Data model

This file lists the types the terminal client holds. Each type lives in the native crate under `native/tuig/src`.

## TaskSubmission

- Defined in `native/tuig/src/lib.rs:44`.
- Fields: `prompt` (1 to 65536 bytes), `message_id` (32 hex characters, generated per submission) and `context_id` (optional, validated by `validate_context_id`).
- A one-shot request (`TUIG-RUNTIME-R002.8.1`) adds an operation ID, an object of parameters, a client idempotency key and an optional `preapproval_ref`. It sits beside this type and reuses its submission path.

## TaskCorrelation

- Defined in `native/tuig/src/correlation.rs:4`.
- Binds the server `task_id` and `context_id` from the first event. The optional server `run_id` is never inferred. Later events must match the initial binding.

## HostLaunchReceipt

- Defined in `native/tuig/src/host_logs.rs:13`.
- Fields: version, opaque log reference, host instance, endpoint, workspace, tenant, principal, session reference, policy revision, grant revision, expiry, interface mode, `web_logs` preference, readiness and source generations.
- The terminal validates every binding before a read. A receipt grants no authority by itself.

## LogBatch and LogRecord

- Defined in `native/tuig/src/host_logs.rs:196` and `:202`.
- A batch holds at most 128 records and a `gap` flag. A record holds host instance, sequence, request, timestamp, severity and message, rendered as a literal line.

## Capability and ApprovalRequest

- Defined in `native/tuig/src/coding_approval.rs:10` and `:74`.
- `Capability` is the closed set of host file and terminal methods. `ApprovalRequest` holds one pending request with its preview and choice.

## Plan binding (planned)

- `TUIG-RUNTIME-R001.3` holds the short-lived server binding in memory only: operation, parameters, digest, subject, registry version and expiry. A confirmation uses it once.
