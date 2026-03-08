"""Secure API key storage via system keyring (macOS Keychain)."""

from __future__ import annotations

import os

SERVICE_NAME = "scitadel"
KEY_NAME = "anthropic-api-key"


def get_api_key() -> str | None:
    """Return the Anthropic API key from env var or keyring."""
    # Env var takes precedence
    key = os.environ.get("ANTHROPIC_API_KEY")
    if key:
        return key

    token = os.environ.get("ANTHROPIC_AUTH_TOKEN")
    if token:
        return token

    try:
        import keyring

        return keyring.get_password(SERVICE_NAME, KEY_NAME)
    except Exception:  # noqa: BLE001
        return None


def store_api_key(key: str) -> None:
    """Store the Anthropic API key in the system keyring."""
    import keyring

    keyring.set_password(SERVICE_NAME, KEY_NAME, key)
    # Also set it in the current process so the SDK picks it up
    os.environ["ANTHROPIC_API_KEY"] = key


def delete_api_key() -> None:
    """Remove the stored API key from the system keyring."""
    import keyring

    try:
        keyring.delete_password(SERVICE_NAME, KEY_NAME)
    except keyring.errors.PasswordDeleteError:
        pass
