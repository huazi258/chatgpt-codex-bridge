# Documentation index

This folder is the working contract for the ChatGPT × Codex workflow middleware.

| Area | Document | Use it for |
| --- | --- | --- |
| Product | [Conversation relay V2 decision](decisions/004-conversation-relay-v2.md) | Active product contract, relay protocol, lifecycle, and acceptance criteria. |
| Product (historical) | [MVP PRD](product-specs/mvp-prd.md) | V1 scope, user flow, and acceptance criteria. |
| Architecture (historical) | [System architecture](architecture/system-architecture.md) | V1 components, data flow, ownership and boundaries. |
| Protocol (historical) | [ChatGPT orchestration protocol](protocols/chatgpt-orchestration-protocol.md) | V1 JSON hand-off contract with ChatGPT. |
| Execution (historical) | [MVP backlog](exec-plans/mvp-backlog.md) | V1 ordered implementation tasks and task-level acceptance. |
| Reliability (historical) | [Operating model](reliability/operating-model.md) | V1 states, budgets, pause behavior, recovery and observability. |
| Security (historical) | [Local security model](security/local-security.md) | V1 credentials, permissions and local trust boundaries. |
| Development | [Local development](development/local-development.md) | Prerequisites, run commands and local verification. |
| Decisions | `decisions/` | Accepted design changes that supersede other documentation. |

Keep documents focused on stable contracts and decisions. Put transient command output, screenshots, and temporary debugging evidence outside `docs/` unless it is required for a decision record.
