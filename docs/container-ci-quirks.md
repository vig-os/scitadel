# Container CI Notes

Behavioral notes for the workspace CI workflow (`.github/workflows/ci.yml`)
when jobs run inside `ghcr.io/vig-os/devcontainer:*` via GitHub Actions
`container:`.

## Tool bootstrap model

The workflow runs with tools already provided by the devcontainer image, then
uses downstream `just` recipes to keep CI aligned with project commands:

```yaml
- run: just sync
```

## git safe.directory

`actions/checkout` runs on the host and bind-mounts the workspace into the
container. The resulting directory is owned by a different UID than the
container's root user, which triggers git's `safe.directory` rejection.
The container workflow adds:

```yaml
- run: git config --global --add safe.directory "$GITHUB_WORKSPACE"
```

## Root user

The container runs as `root` by default. No `sudo` is required and file
permission issues are unlikely, but any git operations need the
`safe.directory` fix above.

## No Docker-in-Docker

The container job does not have access to a Docker or Podman daemon.
Jobs that require building or running containers (e.g. integration tests
using `devcontainer up`) are not supported in this workflow.

## Security scope

`bandit` can still run via the `pre-commit` lint hook (`uv run bandit`), but
there is no separate CI security-report job with JSON artifact uploads.

## Dependency review scope

The CI workflow does not include a dedicated `actions/dependency-review-action`
job; it focuses on validating code quality and tests inside the image.

## No coverage artifact upload

The test job runs `just test` (plain `pytest`) and does not upload
coverage artifacts.

## Pre-commit cache miss

The image ships a pre-commit hook cache at `/opt/pre-commit-cache`, built
from the template workspace's `.pre-commit-config.yaml` (which uses version
tags as revs).  This repository pins hooks by commit hash, so the cached
environments do not match and pre-commit downloads fresh environments at
runtime.
