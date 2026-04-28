# Security Policy

## Supported versions

Nexide is pre-1.0 and explicitly experimental. Only the `main` branch is
supported. There are no LTS branches and there will not be security backports
to older tags.

## Reporting a vulnerability

**Do not open a public GitHub issue for security problems.**

Instead, report privately via one of:

- GitHub's [private vulnerability reporting](https://github.com/sonquer/nexide/security/advisories/new)
  on this repository (preferred).
- Email the maintainers at `sonquer@o2.pl` with a clear description, a
  proof of concept if possible, and the affected commit SHA.

Please include:

- The commit SHA or release you tested against.
- A minimal reproduction (a Next.js route, request payload, or unit test).
- The impact you believe the bug has (information disclosure, sandbox escape,
  remote code execution, denial of service, etc.).
- Any mitigations you are aware of.

## What to expect

- Acknowledgement within 5 working days.
- A triage decision (accepted, needs more info, not a vulnerability) within
  10 working days of acknowledgement.
- Coordinated disclosure once a fix is available. We will credit you in the
  advisory unless you ask us not to.

## Scope

In scope:

- Sandbox escapes from JavaScript out of the V8 isolate into the host process.
- Path traversal or unauthorised file access through the `fs` sandbox.
- Memory safety issues in `unsafe` Rust code.
- Crashes or panics reachable from a crafted HTTP request.
- Denial of service vectors that are not bounded by reasonable rate limiting.

Out of scope:

- Vulnerabilities in upstream dependencies (V8, Tokio, Hyper, Axum). Please
  report those to the respective projects; we will pull in fixes once they
  are released.
- Issues that require an attacker to already control the Next.js application
  bundle, environment variables or filesystem.
- Performance regressions or resource exhaustion that any HTTP server is
  inherently susceptible to (e.g. slowloris without a reverse proxy).

## Hardening status

Nexide has not been audited and has not been deployed to production
workloads we are aware of. Treat any findings accordingly: the absence of
public CVEs reflects youth, not assurance.
