"""Desktop API keys live in the operating system's credential store, never MCP JSON."""
import hashlib
import os
from pathlib import Path
import sys


def backend():
    # Select only OS-protected backends. Never fall back to a plaintext file/plugin.
    if sys.platform == 'win32':
        from keyring.backends.Windows import WinVaultKeyring
        return WinVaultKeyring()
    if sys.platform == 'darwin':
        from keyring.backends.macOS import Keyring
        return Keyring()
    from keyring.backends.SecretService import Keyring
    return Keyring()


def account(home):
    return hashlib.sha256(str(Path(home).resolve()).encode()).hexdigest()


def save(home, key):
    if not isinstance(key, str) or not key.strip() or len(key) > 4096 or '\n' in key:
        raise ValueError('Enter a valid OpenRouter API key.')
    try:
        backend().set_password('Open Workforce Index / OpenRouter', account(home), key.strip())
    except Exception:
        raise ValueError('Could not unlock the system credential store. Unlock your desktop keyring and try again; the key was not saved to a file.') from None


def read(home):
    try:
        return backend().get_password('Open Workforce Index / OpenRouter', account(home))
    except Exception:
        return None


def activate(home):
    key = os.environ.get('OPENROUTER_API_KEY', '')
    if not key or key.startswith('${'):
        stored = read(home)
        if stored:
            os.environ['OPENROUTER_API_KEY'] = stored
