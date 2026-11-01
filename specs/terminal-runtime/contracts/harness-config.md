# HarnessConfig seam

Provider: medulla MEDULLA-ORCH-VOCAB-R009
Consumer: tuig TUIG-RUNTIME-R011

## Wire shape

Medulla supplies the tool selection through `HarnessConfig`. Tuig consumes only the selection that Arbora relays and states no tool list of its own.

## Refusal codes

Tuig must refuse to start or continue a coding session when the contract is absent, incompatible or unauthorized. It must show the typed refusal from the owner and must not substitute a local default.

## Owner row IDs

- Other repository: `MEDULLA-ORCH-VOCAB-R009`
- This repository: `TUIG-RUNTIME-R011`
