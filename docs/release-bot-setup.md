# Release bot token setup

release-please cuts the release tag. Tags pushed with the default `GITHUB_TOKEN`
**do not trigger other workflows** (GitHub's recursion guard), so
`publish-crates.yml` / `binaries.yml` would never fire. A GitHub App token
(preferred) or a PAT — both *non*-`GITHUB_TOKEN` — makes downstream releases run.

`release-please.yml` resolves the token with graceful fallback:

```
GitHub App  →  RELEASE_PLEASE_TOKEN (PAT)  →  GITHUB_TOKEN
```

## Recommended — GitHub App (guided script)

```bash
scripts/setup-release-bot.sh
```

It opens the App-creation page (or reuses an existing App ID), you create the
App + generate a private key and paste the App ID + key, and the script:

- **SOPS-encrypts** the record to `secrets/release-bot.yaml` (age, your recipient),
- sets repo **secret** `RELEASE_BOT_PRIVATE_KEY` + **variable** `RELEASE_BOT_APP_ID`.

It does NOT auto-commit — scitadel is **public**, so committing the (age-encrypted)
record publishes its ciphertext; that stays your explicit `git commit`.

Why an App over a PAT: no expiry to rotate, a bot identity (releases attributed
to a bot, not a person), and it survives people leaving the org. App permissions:
**Contents: R/W**, **Pull requests: R/W** — nothing else.

### Manual fallback (if the script can't open a browser)

1. New App (org-owned): <https://github.com/organizations/vig-os/settings/apps/new>
   — Permissions: Contents R/W, Pull requests R/W; uncheck webhook "Active";
   "Only on this account". Create.
2. Note the **App ID**; **Generate a private key** (downloads a `.pem`).
3. **Install** the App on `vig-os/scitadel`.
4. Store: `gh variable set RELEASE_BOT_APP_ID --repo vig-os/scitadel --body <id>`
   and `gh secret set RELEASE_BOT_PRIVATE_KEY --repo vig-os/scitadel < key.pem`.

## Fallback — Personal Access Token (PAT)

Currently configured. Quicker, but expires and is tied to your account.

- Fine-grained: <https://github.com/settings/personal-access-tokens/new> — owner
  `vig-os`, only `vig-os/scitadel`, **Contents: R/W** + **Pull requests: R/W**.
  (Org may require fine-grained PATs to be enabled/approved.)
- Classic (scope pre-filled): <https://github.com/settings/tokens/new?scopes=repo&description=scitadel-release-please>
- Store it: `scripts/setup-release-token.sh` (or `gh secret set RELEASE_PLEASE_TOKEN --repo vig-os/scitadel`).

Verify: `gh secret list --repo vig-os/scitadel | grep RELEASE_PLEASE_TOKEN`.

When a PAT lapses, release-please still opens PRs (falls back to `GITHUB_TOKEN`)
but the cut tag stops triggering publish/binaries — re-mint and re-store.

## Signed commits

If you enable "Require signed commits" branch protection, you do **not** need to
exempt the `release-please--*` branches: release-please pushes its commits via
the GitHub API, and API-created commits are auto-signed by GitHub's web-flow key
(shown "Verified"). This holds for both the App token and a PAT. Only unsigned
*local CLI* commits would fail such a rule.
