#!/usr/bin/env python3
"""Build a native, self-contained download on each supported OS (never cross-freeze)."""
import argparse
import hashlib
import importlib.metadata
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import tarfile

ROOT = Path(__file__).resolve().parent.parent


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--engine', type=Path, required=True)
    args = parser.parse_args()
    engine = args.engine.resolve()
    if not engine.is_file():
        parser.error('Build the Rust release engine first')
    os.chdir(ROOT)
    build = ROOT/'build/desktop'
    build.mkdir(parents=True, exist_ok=True)
    revision = subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip()
    manifest = build/'build.json'
    manifest.write_text(json.dumps({'revision':revision, 'platform':sys.platform,
        'architecture':platform.machine(), 'python':platform.python_version()}, indent=2))
    # Ship licenses next to the executable, including dependency-specific terms.
    notices = build/'licenses'
    notices.mkdir(exist_ok=True)
    shutil.copy2(ROOT/'LICENSE', notices/'OWI-AGPL-3.0.txt')
    for distribution in importlib.metadata.distributions():
        for path in distribution.files or []:
            if any(word in path.name.lower() for word in ('license', 'copying', 'notice')) and '.dist-info' in str(path):
                source = Path(distribution.locate_file(path))
                if source.is_file():
                    folder = notices/distribution.metadata['Name']
                    folder.mkdir(exist_ok=True)
                    shutil.copy2(source, folder/path.name)
    python_license = Path(sys.base_prefix)/'LICENSE.txt'
    if python_license.exists():
        shutil.copy2(python_license, notices/'Python-LICENSE.txt')
    # Cargo packages remain available via their lockfile and source coordinates.
    metadata = json.loads(subprocess.check_output(['cargo', 'metadata', '--locked', '--format-version', '1'], text=True))
    inventory = []
    for package in metadata['packages']:
        inventory.append({key:package.get(key) for key in ('name','version','license','source','repository')})
        package_root = Path(package['manifest_path']).parent
        folder = notices/('rust-'+package['name']+'-'+package['version'])
        for path in package_root.iterdir():
            if path.is_file() and any(path.name.upper().startswith(s) for s in ('LICENSE', 'COPYING', 'NOTICE')):
                folder.mkdir(exist_ok=True)
                shutil.copy2(path, folder/path.name)
    (notices/'rust-dependencies.json').write_text(json.dumps(inventory, indent=2))
    import PyInstaller.__main__
    arguments = ['--noconfirm', '--clean', '--onedir', '--name', 'OWI',
        '--paths', str(ROOT/'tools'), '--distpath', str(ROOT/'dist'),
        '--workpath', str(build/'work'), '--specpath', str(build),
        '--add-binary', str(engine)+':engine',
        '--add-data', str(manifest)+':.', '--add-data', str(notices)+':licenses',
        '--collect-all', 'keyring', '--collect-all', 'certifi']
    for script in ('owi-do', 'owi-mcp', 'owi-connect'):
        arguments += ['--add-data', str(ROOT/'tools'/script)+':tools']
    for name in ('litellm-prices-sample.json', 'price-import-options.json', 'manager-scenario-seed.json'):
        arguments += ['--add-data', str(ROOT/'examples'/name)+':examples']
    arguments += ['--hidden-import', 'owi_runtime', '--hidden-import', 'owi_platform',
                  '--hidden-import', 'owi_bridge', '--hidden-import', 'owi_credentials',
                  '--hidden-import', 'owi_openrouter', '--hidden-import', 'owi_setup',
                  str(ROOT/'tools/owi_app.py')]
    PyInstaller.__main__.run(arguments)
    bundle = ROOT/'dist/OWI'
    shutil.copy2(ROOT/'LICENSE', bundle/'LICENSE.txt')
    shutil.copy2(ROOT/'docs/DESKTOP.md', bundle/'READ-ME.txt')
    (bundle/'SOURCE.txt').write_text('Corresponding source, build scripts and dependency lockfile:\n'
        'https://github.com/Morshedvarzandeh/Open-Workforce-Index/tree/'+revision+'\n')
    if sys.platform != 'win32':
        launcher = bundle/('Open OWI.command' if sys.platform == 'darwin' else 'Open OWI.sh')
        launcher.write_text('#!/bin/sh\ncd -- "$(dirname -- "$0")" || exit 1\nexec ./OWI setup\n')
        launcher.chmod(0o755)
    architecture = {'AMD64':'x64', 'x86_64':'x64', 'aarch64':'arm64'}.get(platform.machine(), platform.machine())
    system = {'win32':'windows', 'darwin':'macos', 'linux':'linux'}[sys.platform]
    name = 'OWI-'+system+'-'+architecture
    if os.name == 'nt':
        archive = Path(shutil.make_archive(str(ROOT/'dist'/name), 'zip', ROOT/'dist', 'OWI'))
    else:
        archive = ROOT/'dist'/(name+'.tar.gz')
        with tarfile.open(archive, 'w:gz') as output:
            output.add(bundle, arcname='OWI')
    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    archive.with_name(archive.name+'.sha256').write_text(digest+'  '+archive.name+'\n')
    print('Built '+str(archive))


if __name__ == '__main__':
    main()
