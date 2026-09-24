<!-- Managed by vigOS devkit — regenerated on upgrade; local edits are lost. -->
<!-- Customize in justfile.project. Bugs / missing tools: https://github.com/vig-os/devkit/issues -->

# Security Policy

## Supported Versions

| Version  | Supported |
|----------|-----------|
| latest   | Yes       |
| < latest | No        |

Only the latest released version receives security updates.
Older versions are not maintained.

## Reporting a Vulnerability

**Do not open a public GitHub issue for security vulnerabilities.**

To report a vulnerability, use
[GitHub Private Vulnerability Reporting](https://github.com/vig-os/scitadel/security/advisories/new).

When reporting, please include:

- Description of the vulnerability
- Steps to reproduce (or proof of concept)
- Affected component (source code, CI workflow, dependency, etc.)
- Potential impact assessment

## Response Timeline

| Stage              | Target           |
|--------------------|------------------|
| Acknowledgement    | 3 business days  |
| Initial assessment | 7 business days  |
| Fix or mitigation  | 30 calendar days |

We will keep you informed of progress throughout.

## Scope

The following areas are in scope for security reports:

- **Source code:** Application and library code under `src/`
- **Supply chain:** GitHub Actions workflows and pinned dependencies
- **Build tooling:** Project scripts and CI/CD configuration
- **Secrets handling:** Accidental exposure of tokens, keys, or credentials
- **Workflow permissions:** Overly broad permissions in CI/CD pipelines

## Security Practices

This repository follows these security practices:

- All GitHub Actions are pinned to commit SHAs (not mutable tags)
- Pre-commit hook repos are pinned to commit SHAs
- Renovate monitors dependencies for updates and known vulnerabilities
- Dependency review blocks pull requests that introduce vulnerable dependencies
- Workflow permissions follow the principle of least privilege
- Workflow inputs are bound to environment variables (not interpolated inline)
- No `pull_request_target` triggers are used (prevents untrusted code execution)
- OpenSSF Scorecard runs weekly to track security posture
- CodeQL static analysis scans the project and GitHub Actions workflows

### OpenSSF Scorecard accepted findings

The following Scorecard checks are not applicable to this project and are
accepted as won't-fix:

- **FuzzingID** (medium): no fuzzing targets in the project or CI scripts
- **CIIBestPracticesID** (low): not a CII Best Practices badge candidate; posture is tracked via Scorecard and CodeQL instead
