# CodingSessionAdmission seam

Provider: arbora ARBORA-ACP-R001.4
Consumer: tuig TUIG-RUNTIME-R011

## Wire shape

Arbora admits a coding session and states its permission rules. Tuig reads the admission and displays the granted client capabilities in `CodingSessionView`.

## Refusal codes

Tuig must refuse to start or continue a coding session when the contract is absent, incompatible or unauthorized. It must show the typed refusal from the owner and must not substitute a local default.

## Owner row IDs

- Other repository: `ARBORA-ACP-R001.4`
- This repository: `TUIG-RUNTIME-R011`
