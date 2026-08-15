# Documentation index

This folder is the working contract for the ChatGPT × Codex workflow middleware.

| Area | Document | Use it for |
| --- | --- | --- |
| Product | [MVP PRD](product-specs/mvp-prd.md) | Scope, user flow, MVP acceptance criteria. |
| Architecture | [System architecture](architecture/system-architecture.md) | Components, data flow, ownership and boundaries. |
| Protocol | [ChatGPT orchestration protocol](protocols/chatgpt-orchestration-protocol.md) | The validated hand-off contract with ChatGPT. |
| Execution | [MVP backlog](exec-plans/mvp-backlog.md) | Ordered implementation tasks and task-level acceptance. |
| Reliability | [Operating model](reliability/operating-model.md) | States, budgets, pause behavior, recovery and observability. |
| Security | [Local security model](security/local-security.md) | Credentials, permissions and local trust boundaries. |
| Development | [Local development](development/local-development.md) | Prerequisites, run commands and local verification. |
| Decisions | `decisions/` | Accepted design changes that supersede other documentation. |

Keep documents focused on stable contracts and decisions. Put transient command output, screenshots, and temporary debugging evidence outside `docs/` unless it is required for a decision record.
