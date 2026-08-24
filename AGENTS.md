# Agent guide

## Project purpose

This repository contains the local desktop middleware that orchestrates a controlled workflow between ChatGPT and Codex App Server. ChatGPT plans and reviews, Codex executes work in a repository, and the middleware transports context, enforces budgets, and pauses for human acceptance.

## Documentation map

- [Documentation index](docs/README.md)
- [MVP product requirements](docs/product-specs/mvp-prd.md)
- [System architecture](docs/architecture/system-architecture.md)
- [ChatGPT orchestration protocol](docs/protocols/chatgpt-orchestration-protocol.md)
- [Implementation backlog](docs/exec-plans/mvp-backlog.md)
- [Reliability and operations](docs/reliability/operating-model.md)
- [Local security model](docs/security/local-security.md)

Read the product requirements, architecture, protocol, and relevant execution plan before changing behavior. Update the corresponding document in the same change when an implementation decision changes the documented contract.

## Source of truth

- Product behavior: `docs/product-specs/`
- Interfaces and component boundaries: `docs/architecture/` and `docs/protocols/`
- Ordered implementation work and acceptance criteria: `docs/exec-plans/`
- Failure handling and recovery: `docs/reliability/`
- Trust boundaries and credential handling: `docs/security/`

When documents conflict, prefer the newest accepted decision record in `docs/decisions/`; otherwise stop and request clarification.

## Engineering constraints

- The MVP is single-module, single-repository, single-ChatGPT-session, and serial.
- Use Codex App Server as the execution interface. Do not automate the Codex desktop window.
- Treat the ChatGPT browser adapter as untrusted input: validate the machine-readable protocol before running Codex.
- Do not save browser cookies, ChatGPT passwords, Git credentials, or raw App Server secrets.
- Use loopback or stdio only for local component communication; do not expose App Server remotely in the MVP.
- Codex owns code changes, test execution, commits, and pushes. The middleware verifies outcomes; it does not silently repair failures.
- Any protocol failure, unclear task, failed verification, or external error must pause the workflow and present a user-actionable summary.

## Change workflow

1. Inspect the relevant implementation and linked documents.
2. Identify the applicable acceptance criteria in `docs/exec-plans/`.
3. Implement the smallest coherent slice.
4. Run the relevant tests, build, or integration check.
5. Update docs if contracts, state transitions, permissions, or operating behavior changed.
6. For every code change, commit and push the verified change to GitHub unless the user explicitly instructs otherwise.
7. Report changed files, verification, and remaining risks.

## Current delivery stage

Task 6 completed its implementation and automated verification on 2026-08-16. Task 7 pilot validation may now begin.
