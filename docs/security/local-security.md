# Local security model

## Assets

- The user’s authenticated ChatGPT browser session.
- The selected local repository and Git remote credentials.
- Codex account access, execution permissions, and App Server session.
- Module prompts, summaries, audit logs, and the local SQLite database.

## Trust boundaries

- ChatGPT response text crosses from the browser extension into the middleware and is untrusted until schema validation.
- The middleware starts Codex in a repository selected by the user; repository instructions can influence Codex and are therefore treated as untrusted project input.
- Git remotes are external systems; push verification is required before ChatGPT is told a change is available.

## Controls

- Use a dedicated Chrome profile and a site-limited extension permission for `chatgpt.com` only.
- Pair the extension and desktop app with a short-lived local secret; reject unknown local clients.
- Keep App Server on stdio. If a loopback transport is added later, require a capability token and never bind it to a non-loopback interface.
- Store no browser cookies, account passwords, Git credentials, or raw App Server tokens in SQLite or logs.
- Require an explicit repository root and branch at module start; stop if either changes unexpectedly.
- Redact credential-shaped strings from diagnostics and limit log retention through a user-visible setting.

## Non-goals

The MVP does not attempt to sandbox Codex beyond its configured Codex environment, protect against a compromised local operating-system account, or support multi-user access.
