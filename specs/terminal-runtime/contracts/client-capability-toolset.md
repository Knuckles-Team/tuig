# ClientCapabilityToolset seam

Provider: meristem MERISTEM-CONTROL-R010
Consumer: tuig TUIG-RUNTIME-R011

## Wire shape

Meristem exposes the tools that map to the host file and terminal client capabilities. Tuig performs the host action only for a capability in the closed set held by `CodingSessionView`.

## Refusal codes

Tuig must refuse to start or continue a coding session when the contract is absent, incompatible or unauthorized. It must show the typed refusal from the owner and must not substitute a local default.

## Owner row IDs

- Other repository: `MERISTEM-CONTROL-R010`
- This repository: `TUIG-RUNTIME-R011`
