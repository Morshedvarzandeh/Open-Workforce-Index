#!/usr/bin/env python3
"""OWI desktop entry point. The same executable serves MCP without opening a window."""
import sys
import owi_platform as platform


def main():
    # MCP and provider text use UTF-8 on Windows as well as Unix.
    for stream in (sys.stdin, sys.stdout, sys.stderr):
        if stream is not None and hasattr(stream, 'reconfigure'):
            stream.reconfigure(encoding='utf-8')
    platform.initialize()
    action = sys.argv[1] if len(sys.argv) > 1 else 'setup'
    if action == 'setup':
        from owi_setup import main as setup
        return setup()
    if action == 'install':
        import json
        from owi_setup import install_copy
        print(json.dumps({'executable':str(install_copy() or sys.executable)}))
        return 0
    if action == 'openrouter':
        import owi_openrouter
        sys.argv = [sys.argv[0], *sys.argv[2:]]
        return owi_openrouter.main()
    if action in ('connect', 'mcp'):
        sys.argv = [sys.argv[0], *sys.argv[2:]]
        return platform.load_tool('owi-'+action).main()
    if action == 'diagnose':
        import json
        import subprocess
        import certifi
        import ssl
        from owi_credentials import backend
        credential_backend = type(backend()).__name__
        certificates = ssl.create_default_context(cafile=certifi.where()).cert_store_stats()['x509_ca']
        if certificates == 0:
            raise ValueError('The downloaded certificate bundle is empty.')
        result = subprocess.run([__import__('os').environ['OWI_BINARY'], 'ontology', 'validate'],
            capture_output=True, text=True, env=platform.external_env(), check=True)
        print(json.dumps({'packaged':platform.frozen(), 'engine':json.loads(result.stdout),
            'credential_backend':credential_backend, 'trusted_certificates':certificates}))
        return 0
    if action == 'gui-smoke':
        from owi_setup import window
        root = window()
        root.update()
        root.destroy()
        print('Packaged setup window opened successfully.')
        return 0
    print('Usage: OWI [setup | connect | mcp | diagnose]', file=sys.stderr)
    return 2


if __name__ == '__main__':
    raise SystemExit(main())
