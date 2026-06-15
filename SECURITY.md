# Security Policy

## Supported versions

`orion-core` is pre-1.0. Security fixes land on the latest `0.x` release; please
upgrade to the most recent version before reporting.

| Version | Supported |
|---------|-----------|
| 0.2.x   | ✅        |
| < 0.2   | ❌        |

## Reporting a vulnerability

Please **do not** open a public issue for security problems.

Report privately via one of:

- GitHub's [private vulnerability reporting](https://docs.github.com/en/code-security/security-advisories/guidance-on-reporting-and-writing-information-about-vulnerabilities/privately-reporting-a-security-vulnerability)
  ("Report a vulnerability" under the repository's **Security** tab), or
- email **anirudhastark@gmail.com** with a description and reproduction steps.

You can expect an acknowledgement within a few days. Once a fix is ready we'll
coordinate a release and credit you in the advisory unless you prefer to remain
anonymous.

## Scope

`orion-core` is a library that orchestrates conversation state and prompt
formatting; it does not run a model or perform network I/O itself. Note that
**tool execution and `LlmBackend` implementations are provided by the host
application** — vulnerabilities in those belong to the host, not this crate.
