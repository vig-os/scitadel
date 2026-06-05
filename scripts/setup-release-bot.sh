#!/usr/bin/env bash
# Guided setup of the release-please GitHub App credentials.
#
# You do the browser parts (create/choose the App, generate a private key, paste
# the App ID + key). The script does the rest:
#   - SOPS-encrypts the record at secrets/release-bot.yaml (age; your recipient)
#   - sets GitHub Actions: secret RELEASE_BOT_PRIVATE_KEY + variable
#     RELEASE_BOT_APP_ID  (exactly what release-please.yml consumes)
#
# It does NOT git-commit — scitadel is a PUBLIC repo, so committing the (age-
# encrypted) record publishes its ciphertext. That stays your explicit choice;
# the script just stages it and prints the commit command.
set -euo pipefail

REPO="${REPO:-vig-os/scitadel}"
ORG="${ORG:-vig-os}"
AGE_RECIPIENT="${AGE_RECIPIENT:-age17ueacw8hpda37r0j3ed8sh7z0k7rcrjuztc22krk7eth030t5u8q0jfm87}"
SECRET_FILE="secrets/release-bot.yaml"

for b in gh sops; do command -v "$b" >/dev/null 2>&1 || { echo "error: '$b' is required"; exit 1; }; done
gh auth status >/dev/null 2>&1 || { echo "error: run 'gh auth login' first"; exit 1; }
opener=$(command -v open 2>/dev/null || command -v xdg-open 2>/dev/null || true)
open_url() { [ -n "$opener" ] && "$opener" "$1" >/dev/null 2>&1 || echo "  open: $1"; }

cd "$(git rev-parse --show-toplevel)"

# 1. Ensure .sops.yaml exists (mirrors your dotfiles convention).
if [ ! -f .sops.yaml ]; then
  cat > .sops.yaml <<EOF
keys:
  - &larsgerchow $AGE_RECIPIENT
creation_rules:
  - path_regex: secrets/.*\.yaml\$
    key_groups:
      - age:
          - *larsgerchow
EOF
  echo "created .sops.yaml (age recipient ${AGE_RECIPIENT:0:16}…)"
fi

# 2. App ID — reuse an existing App or create a dedicated one.
read -rp "Existing App ID to reuse (blank → create a new dedicated app): " APP_ID
if [ -z "$APP_ID" ]; then
  cat <<EOF
Opening the App creation page. Enter:
  • Name:        vig-os-release-please
  • Homepage:    https://github.com/$REPO
  • Webhook:     UNCHECK "Active"
  • Permissions: Repository → Contents = Read & write, Pull requests = Read & write (nothing else)
  • Install:     Only on this account → Create GitHub App
EOF
  open_url "https://github.com/organizations/$ORG/settings/apps/new"
  read -rp "After creating, paste the App ID (top of the app's settings page): " APP_ID
fi
[ -n "$APP_ID" ] || { echo "error: no App ID provided"; exit 1; }

# 3. Private key — accept a .pem path or pasted PEM.
echo "On the app page: Private keys → Generate a private key (downloads a .pem)."
read -rp "Path to the .pem (blank → paste the contents next): " PEM_PATH
if [ -n "$PEM_PATH" ]; then
  PEM_PATH="${PEM_PATH/#\~/$HOME}"
  PEM="$(cat "$PEM_PATH")"
else
  echo "Paste the PEM contents, then press Ctrl-D:"
  PEM="$(cat)"
fi
case "$PEM" in *"PRIVATE KEY"*) : ;; *) echo "error: that doesn't look like a PEM private key"; exit 1 ;; esac

# 4. SOPS-encrypt the record (app id + key) in place.
mkdir -p secrets
{
  printf 'app_id: "%s"\n' "$APP_ID"
  printf 'private_key: |\n'
  printf '%s\n' "$PEM" | sed 's/^/  /'
} > "$SECRET_FILE"
sops -e -i "$SECRET_FILE"
echo "encrypted → $SECRET_FILE"

# 5. GitHub Actions wiring. Secret BEFORE variable: release-please.yml gates the
#    App-token step on RELEASE_BOT_APP_ID, so the key must exist first.
printf '%s' "$PEM" | gh secret set RELEASE_BOT_PRIVATE_KEY --repo "$REPO"
gh variable set RELEASE_BOT_APP_ID --repo "$REPO" --body "$APP_ID"
unset PEM
echo "GitHub: set secret RELEASE_BOT_PRIVATE_KEY + variable RELEASE_BOT_APP_ID"

# 6. Install + next steps.
echo
echo "If not already installed: app settings → Install App → $ORG → select $REPO."
open_url "https://github.com/organizations/$ORG/settings/apps"
cat <<EOF

Next:
  • Review, then commit the encrypted record (PUBLIC repo — ciphertext goes public):
      git add .sops.yaml $SECRET_FILE && git commit -m "chore: SOPS-encrypted release-bot credentials"
  • Once verified, drop the redundant PAT:
      gh secret delete RELEASE_PLEASE_TOKEN --repo $REPO
EOF
