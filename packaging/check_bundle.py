#!/usr/bin/env python3
"""Test the actual extracted download with no user Python, Rust, or model CLI."""
import json
import os
from pathlib import Path
import queue
import shutil
import subprocess
import tarfile
import tempfile
import threading
import zipfile

ROOT = Path(__file__).resolve().parent.parent


def main():
    archives = list((ROOT/'dist').glob('OWI-*.zip'))+list((ROOT/'dist').glob('OWI-*.tar.gz'))
    assert len(archives) == 1, archives
    with tempfile.TemporaryDirectory(prefix='OWI download with spaces ') as temporary:
        base = Path(temporary)
        if archives[0].suffix == '.zip':
            with zipfile.ZipFile(archives[0]) as archive:
                archive.extractall(base)
        else:
            with tarfile.open(archives[0]) as archive:
                archive.extractall(base, filter='data')
        app = (base/'OWI'/('OWI.exe' if os.name == 'nt' else 'OWI')).resolve()
        project = base/'My project'
        project.mkdir()
        home = base/'private data'
        fake_user = base/'user'
        fake_user.mkdir()
        env = dict(os.environ)
        for name in list(env):
            if name.startswith(('OWI_', 'PYTHON', 'CARGO', 'RUSTUP')) or name.endswith('API_KEY'):
                env.pop(name)
        env.update(HOME=str(fake_user), USERPROFILE=str(fake_user),
            LOCALAPPDATA=str(fake_user/'AppData/Local'), XDG_DATA_HOME=str(fake_user/'data'),
            COPILOT_HOME=str(fake_user/'copilot'),
            PATH=str(Path(env['SystemRoot'])/'System32') if os.name == 'nt' else '')
        def run(*args, **options):
            return subprocess.run([str(app), *args], env=env, cwd=base,
                capture_output=True, text=True, timeout=60, **options)
        assert not shutil.which('python', path=env['PATH'])
        assert not shutil.which('cargo', path=env['PATH'])
        diagnosis = run('diagnose', check=True)
        assert json.loads(diagnosis.stdout)['packaged']
        for client in ('vscode', 'cursor', 'claude-code', 'copilot-cli'):
            result = run('connect', '--client', client, '--project', str(project),
                '--home', str(home), '--prepare')
            assert result.returncode == 0, (client, result.stderr)
        definition = json.loads((project/'.vscode/mcp.json').read_text())['servers']['owi']
        assert definition['command'] == str(app)
        assert definition['args'][0] == 'mcp'
        assert 'env' not in definition
        # A shell builtin stands in for the owner's model. No interpreter is installed.
        runner = 'echo Friday' if os.name == 'nt' else "printf 'Friday\\n'"
        (home/'runners.json').write_text(json.dumps({'haiku-4-5':runner}))
        process = subprocess.Popen([str(app), 'mcp', '--home', str(home)], env=env, cwd=base,
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        lines = queue.Queue()
        def reader():
            for line in process.stdout:
                lines.put(line)
        threading.Thread(target=reader, daemon=True).start()
        def rpc(identifier, method, params):
            process.stdin.write(json.dumps({'jsonrpc':'2.0', 'id':identifier,
                'method':method, 'params':params})+'\n')
            process.stdin.flush()
            result = json.loads(lines.get(timeout=30))
            assert result['id'] == identifier, result
            assert 'error' not in result, result
            return result['result']
        try:
            rpc(1, 'initialize', {'protocolVersion':'2025-11-25'})
            process.stdin.write(json.dumps({'jsonrpc':'2.0', 'method':'notifications/initialized'})+'\n')
            process.stdin.flush()
            status = rpc(2, 'tools/call', {'name':'owi_status', 'arguments':{}})
            assert '"ready": true' in status['content'][0]['text'], status
            result = rpc(3, 'tools/call', {'name':'owi_work', 'arguments':{
                'task':'Write a delivery confirmation', 'checks':['contains:Friday']}})
            assert not result.get('isError'), result
            value = json.loads(result['content'][0]['text'])
            assert value['verdict'] == 'accepted', value
            assert 'Friday' in value['output'], value
            event = json.loads((home/'last-outcome.json').read_text())['event']
            assert event['validation_kind'] == 'deterministic', event
            process.stdin.close()
            assert process.wait(timeout=15) == 0
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()
        # The bundled provider subprocess dispatch must not recurse into setup/Python.
        profile = base/'profile.json'
        profile.write_text('{}')
        rejected = run('openrouter', '--profile', str(profile), input='hello')
        assert rejected.returncode == 1, rejected
        assert 'OPENROUTER_API_KEY' in rejected.stderr, rejected.stderr
        installed = Path(json.loads(run('install', check=True).stdout)['executable'])
        assert installed != app
        app.parent.rename(base/'deleted original download')
        app = installed
        assert json.loads(run('diagnose', check=True).stdout)['packaged']
        assert Path(json.loads(run('install', check=True).stdout)['executable']) == app
        print('Extracted download passed: no Python/Rust on PATH; engine, all four connections, MCP work and deterministic feedback; no paid calls.')


if __name__ == '__main__':
    main()
