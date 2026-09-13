"""Paths and subprocess boundaries shared by source and self-contained builds."""
import importlib.machinery
import importlib.util
import os
from pathlib import Path
import sys


def frozen():
    return bool(getattr(sys, 'frozen', False))


def resources():
    return Path(sys._MEIPASS) if frozen() else Path(__file__).resolve().parent.parent


def default_home():
    if not frozen():
        return resources()/'.owi-quick'
    if sys.platform == 'win32':
        return Path(os.environ.get('LOCALAPPDATA', Path.home()/'AppData/Local'))/'OWI'
    if sys.platform == 'darwin':
        return Path.home()/'Library/Application Support/OWI'
    return Path(os.environ.get('XDG_DATA_HOME', Path.home()/'.local/share'))/'owi'


def command(tool):
    if frozen():
        return [sys.executable, tool]
    script = 'owi_openrouter.py' if tool == 'openrouter' else 'owi-'+tool
    return [sys.executable, str(resources()/'tools'/script)]


def load_tool(name):
    loader = importlib.machinery.SourceFileLoader('owi_'+name.replace('-', '_'),
        str(resources()/'tools'/name))
    spec = importlib.util.spec_from_loader(loader.name, loader)
    module = importlib.util.module_from_spec(spec)
    loader.exec_module(module)
    return module


def external_env():
    """Do not inject bundled Python shared libraries into an owner's model CLI."""
    env = dict(os.environ)
    if frozen():
        for name in ('LD_LIBRARY_PATH', 'LIBPATH'):
            original = env.pop(name+'_ORIG', None)
            if original is None:
                env.pop(name, None)
            else:
                env[name] = original
        env['PYINSTALLER_RESET_ENVIRONMENT'] = '1'
    return env


def process_options():
    if frozen() and os.name == 'nt':
        import subprocess
        return {'creationflags':subprocess.CREATE_NO_WINDOW}
    return {}


def initialize():
    if frozen():
        engine = resources()/'engine'/('owi.exe' if os.name == 'nt' else 'owi')
        if not engine.is_file():
            raise ValueError('The OWI download is incomplete. Extract the entire archive again.')
        os.environ['OWI_BINARY'] = str(engine)
        if os.name == 'nt':
            import ctypes
            ctypes.windll.kernel32.SetDllDirectoryW(None)
    # Desktop launchers often omit the standard locations of existing AI CLIs.
    additions = [Path.home()/'.local/bin', Path.home()/'.cargo/bin']
    if sys.platform == 'darwin':
        additions += [Path('/opt/homebrew/bin'), Path('/usr/local/bin')]
    current = os.environ.get('PATH', '').split(os.pathsep)
    os.environ['PATH'] = os.pathsep.join(current+[str(p) for p in additions if str(p) not in current])
