"""Desktop path, installation and credential boundaries; no network or paid calls."""
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import Mock, patch
import owi_credentials as credentials
import owi_platform as platform
import owi_setup as setup


class DesktopTests(unittest.TestCase):
    def test_frozen_commands_never_invoke_a_python_script(self):
        with patch.object(sys, 'frozen', True, create=True):
            for action in ('mcp', 'openrouter'):
                self.assertEqual(platform.command(action), [sys.executable, action])

    def test_external_clis_do_not_inherit_bundled_library_paths(self):
        with patch.dict(os.environ, {'LD_LIBRARY_PATH':'bundled', 'LD_LIBRARY_PATH_ORIG':'owner'}, clear=True), \
             patch.object(sys, 'frozen', True, create=True):
            self.assertEqual(platform.external_env()['LD_LIBRARY_PATH'], 'owner')
            self.assertEqual(os.environ['LD_LIBRARY_PATH'], 'bundled')
        with patch.dict(os.environ, {'LD_LIBRARY_PATH':'bundled'}, clear=True), \
             patch.object(sys, 'frozen', True, create=True):
            self.assertNotIn('LD_LIBRARY_PATH', platform.external_env())

    def test_credentials_are_scoped_and_never_fall_back_to_a_file(self):
        with tempfile.TemporaryDirectory() as temporary:
            home = Path(temporary)
            store = Mock()
            with patch.object(credentials, 'backend', return_value=store):
                credentials.save(home, 'test-key')
            self.assertIn('test-key', store.set_password.call_args.args)
            self.assertNotEqual(credentials.account(home), credentials.account(home/'other'))
            with patch.object(credentials, 'backend', side_effect=RuntimeError('secret in failure')):
                with self.assertRaisesRegex(ValueError, 'credential store') as error:
                    credentials.save(home, 'test-key')
                self.assertNotIn('secret in failure', str(error.exception))
                self.assertIsNone(credentials.read(home))
            self.assertEqual(list(home.iterdir()), [])

    def test_explicit_environment_key_takes_precedence(self):
        with patch.dict(os.environ, {'OPENROUTER_API_KEY':'owner-key'}), \
             patch.object(credentials, 'read', side_effect=AssertionError('no keyring read')):
            credentials.activate(Path('.'))

    def test_unresolved_environment_reference_uses_saved_key(self):
        with patch.dict(os.environ, {'OPENROUTER_API_KEY':'${MISSING}'}), \
             patch.object(credentials, 'read', return_value='saved-key'):
            credentials.activate(Path('.'))
            self.assertEqual(os.environ['OPENROUTER_API_KEY'], 'saved-key')

    def test_invalid_project_does_not_store_key_or_initialize(self):
        with patch.object(credentials, 'save', side_effect=AssertionError('no write')):
            with self.assertRaisesRegex(ValueError, 'project folder'):
                setup.connect('vscode', '', 'openrouter', 'test-key')

    def test_missing_claude_is_clear_without_changing_an_account(self):
        with patch('shutil.which', return_value=None):
            with self.assertRaisesRegex(ValueError, 'Claude Code was not found'):
                setup.connect('cursor', '', 'claude', '')

    def test_install_preserves_bundle_and_uses_stable_path(self):
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            download = base/'download'
            resources = download/'_internal'
            resources.mkdir(parents=True)
            (resources/'build.json').write_text(json.dumps({'revision':'a'*40}))
            app = download/'OWI'
            app.write_text('test executable')
            with patch.object(platform, 'frozen', return_value=True), \
                 patch.object(platform, 'resources', return_value=resources), \
                 patch.object(platform, 'default_home', return_value=base/'user data'), \
                 patch.object(sys, 'executable', str(app)):
                installed = setup.install_copy()
                self.assertEqual(installed.read_text(), 'test executable')
                self.assertTrue((installed.parent/'_internal/build.json').exists())
                self.assertEqual(setup.install_copy(), installed)
                with patch.object(sys, 'executable', str(installed)):
                    self.assertIsNone(setup.install_copy())


if __name__ == '__main__':
    unittest.main(verbosity=2)
