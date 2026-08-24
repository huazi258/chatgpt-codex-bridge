# Documentation

This folder contains both user-facing documentation and the design/engineering records used to maintain ChatGPT Codex Bridge.

## New user

Start here if you cloned the repository and want to use it.

| Document | Use it for |
| --- | --- |
| [Getting Started](getting-started.md) | Go from a fresh clone to the first ChatGPT → Codex → ChatGPT relay cycle. |
| [User Guide](user-guide.md) | Learn modules, manual vs automation messages, Codex threads, budgets, acceptance, delivery states, and recovery. |
| [Troubleshooting](troubleshooting.md) | Diagnose startup, Chrome pairing, Codex, protocol, `UNKNOWN`, and thread-recovery problems. |

The repository root [README](../README.md) is the short product overview and Quick Start.

## Contributor

Use these when changing or validating the implementation.

| Area | Document | Use it for |
| --- | --- | --- |
| Development | [Local development](development/local-development.md) | Toolchain, run commands, build and verification. |
| Agent instructions | [`AGENTS.md`](../AGENTS.md) | Repository change workflow and engineering constraints. |
| Architecture | [System architecture](architecture/system-architecture.md) | Historical V1 components, data flow, ownership, and boundaries. |
| Tests / implementation | source and test directories | Verify current behavior against the newest accepted product decisions. |

## Maintainer and design history

The active product contract is defined by the newest accepted decision records. Older documents remain useful historical evidence but may have been superseded.

| Area | Document | Use it for |
| --- | --- | --- |
| Product | [Conversation relay V2 decision](decisions/004-conversation-relay-v2.md) | Active relay product contract, lifecycle, protocol, recovery rules, and acceptance criteria. |
| Product (historical) | [MVP PRD](product-specs/mvp-prd.md) | V1 scope, user flow, and acceptance criteria. |
| Architecture (historical) | [System architecture](architecture/system-architecture.md) | V1 components and boundaries. |
| Protocol (historical) | [ChatGPT orchestration protocol](protocols/chatgpt-orchestration-protocol.md) | Earlier V1 machine-readable hand-off contract. |
| Execution (historical) | [MVP backlog](exec-plans/mvp-backlog.md) | Ordered V1 implementation tasks and acceptance. |
| Reliability (historical) | [Operating model](reliability/operating-model.md) | Earlier states, budgets, pause behavior, recovery, and observability. |
| Security (historical) | [Local security model](security/local-security.md) | Credentials, permissions, and local trust boundaries. |
| Decisions | [`decisions/`](decisions/) | Accepted design changes that supersede conflicting older documentation. |
| Detailed design records | [`superpowers/specs/`](superpowers/specs/) | Focused design and implementation planning for later V2 slices. |

## Documentation rule

Keep user guides focused on what a person must know to operate the current product.

Keep architecture, decision, protocol, reliability, and security documents focused on stable contracts and engineering rationale.

When two design documents conflict, prefer the newest accepted decision record.
