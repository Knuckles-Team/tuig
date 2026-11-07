# WorkstreamPeer seam

Provider: arbora ARBORA-HARNESS-PLUGINS-R004.2
Consumer: tuig TUIG-RUNTIME-R013

## Wire shape

Tuig calls `workstream.attach` with `{harness: "tuig", harness_session_id, workspace_root}` and renders the returned `context`. At exit it calls `workstream.checkpoint` with `{harness_session_id, summary, run_ids, trigger: "exit"}` and then `workstream.detach`. Arbora returns `already_attached` with an empty context on a repeat attach.

## Refusal codes

- `AUTH_REQUIRED`: no valid credential; tuig shows the sign-in hint and keeps the session.
- `WORKSTREAM_NOT_FOUND`: the read names a workstream that the caller does not own.
