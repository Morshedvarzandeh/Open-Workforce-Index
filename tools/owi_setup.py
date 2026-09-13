"""Small native first-run screen; the host launches the background MCP process later."""
import json
import os
from pathlib import Path
import queue
import re
import shutil
import subprocess
import sys
import tempfile
import threading
import owi_platform as platform

CLIENTS = {'VS Code Copilot':'vscode', 'Cursor':'cursor',
           'Claude Code':'claude-code', 'Copilot CLI':'copilot-cli'}


def install_copy():
    """Keep MCP paths valid when the user deletes the original download."""
    if not platform.frozen():
        return None
    manifest = json.loads((platform.resources()/'build.json').read_text())
    revision = manifest['revision']
    if not re.fullmatch(r'[0-9a-f]{40}', revision):
        raise ValueError('Invalid download version. Download OWI again from its release page.')
    source = Path(sys.executable).resolve().parent
    parent = (platform.default_home()/'app').resolve()
    destination = parent/revision[:12]
    if source == destination:
        return None
    parent.mkdir(parents=True, exist_ok=True)
    if not destination.exists():
        temporary = Path(tempfile.mkdtemp(prefix='.install-', dir=parent))
        try:
            shutil.copytree(source, temporary/'bundle', symlinks=True)
            os.replace(temporary/'bundle', destination)
        finally:
            shutil.rmtree(temporary, ignore_errors=True)
    installed = destination/Path(sys.executable).name
    if not installed.is_file():
        raise ValueError('The installed copy is incomplete. Remove that version from the OWI app folder and reopen the download.')
    return installed


def connect(client, project, provider, key, home=None):
    """Validate choices before initializing state; never execute a paid test task."""
    home = Path(home) if home is not None else platform.default_home()
    if client not in CLIENTS.values() or provider not in ('claude', 'openrouter'):
        raise ValueError('Choose an AI app and a worker account.')
    project = Path(project).expanduser().resolve() if project else None
    if client in ('claude-code', 'vscode') and project is None:
        raise ValueError('Choose the project folder where you want to use OWI.')
    if project is not None and not project.is_dir():
        raise ValueError('Choose an existing project folder.')
    if provider == 'claude' and not shutil.which('claude'):
        raise ValueError('Claude Code was not found. Use the computer where you already run Claude Code, or choose OpenRouter.')
    import owi_credentials as credentials
    if provider == 'openrouter' and not (key.strip() or os.environ.get('OPENROUTER_API_KEY') or credentials.read(home)):
        raise ValueError('Enter your OpenRouter API key.')
    from owi_bridge import operation_lock, do
    installer = platform.load_tool('owi-connect')
    arguments = ['--client', client, '--home', str(home), '--prepare']
    if project is not None:
        arguments += ['--project', str(project)]
    if provider == 'openrouter':
        arguments += ['--openrouter']
    with operation_lock(home):
        if provider == 'openrouter' and key.strip():
            credentials.save(home, key)
        try:
            installer.main(arguments)
        except SystemExit as error:
            if error.code:
                raise ValueError('Connection setup failed. Check the selected project and existing OWI connection settings.') from None
        if provider == 'claude':
            # The chosen account type must not silently route to another paid provider.
            (home/'bridge.json').write_text(json.dumps({'models':list(do.CLAUDE_MODELS)}, indent=2))
    return 'Connected. Open your AI app, enable OWI tools, and ask it to check OWI status. Then try a small task. Your AI app controls tool approvals; your existing account billing still applies.'


def window():
    import tkinter as tk
    from tkinter import filedialog, ttk
    root = tk.Tk()
    root.title('Connect OWI')
    root.minsize(560, 530)
    root.configure(background='#f7f7f5')
    style = ttk.Style(root)
    style.configure('TFrame', background='#f7f7f5')
    style.configure('TLabel', background='#f7f7f5', font=('Arial', 11))
    style.configure('Title.TLabel', font=('Arial', 26, 'bold'))
    style.configure('TButton', padding=(14, 9), font=('Arial', 11))
    frame = ttk.Frame(root, padding=30)
    frame.grid(sticky='nsew')
    root.columnconfigure(0, weight=1)
    root.rowconfigure(0, weight=1)
    frame.columnconfigure(0, weight=1)
    ttk.Label(frame, text='OWI', style='Title.TLabel').grid(sticky='w')
    ttk.Label(frame, text='Connect once. Keep working in your AI app.', wraplength=480).grid(sticky='w', pady=(6, 24))
    ttk.Label(frame, text='Your AI app').grid(sticky='w')
    client = tk.StringVar(value='VS Code Copilot')
    ttk.Combobox(frame, textvariable=client, values=list(CLIENTS), state='readonly').grid(sticky='ew', pady=(5, 15))
    ttk.Label(frame, text='Project folder — required for VS Code and Claude Code').grid(sticky='w')
    folder_frame = ttk.Frame(frame)
    folder_frame.grid(sticky='ew', pady=(5, 15))
    folder_frame.columnconfigure(0, weight=1)
    project = tk.StringVar()
    ttk.Entry(folder_frame, textvariable=project).grid(row=0, column=0, sticky='ew')
    def browse():
        choice = filedialog.askdirectory(parent=root, title='Choose your project')
        if choice:
            project.set(choice)
    ttk.Button(folder_frame, text='Choose…', command=browse).grid(row=0, column=1, padx=(8, 0))
    ttk.Label(frame, text='Account for delegated work').grid(sticky='w')
    provider = tk.StringVar(value='Existing Claude Code login' if shutil.which('claude') else 'OpenRouter')
    ttk.Combobox(frame, textvariable=provider, values=['Existing Claude Code login', 'OpenRouter'], state='readonly').grid(sticky='ew', pady=(5, 8))
    account_note = ttk.Label(frame, wraplength=480)
    account_note.grid(sticky='w', pady=(0, 10))
    key_frame = ttk.Frame(frame)
    key_frame.grid(sticky='ew')
    key_frame.columnconfigure(0, weight=1)
    ttk.Label(key_frame, text='OpenRouter API key').grid(sticky='w')
    key = tk.StringVar()
    ttk.Entry(key_frame, textvariable=key, show='•').grid(sticky='ew', pady=(5, 5))
    ttk.Label(key_frame, text='Saved in your system credential store. Leave blank to reuse it.', wraplength=480).grid(sticky='w', pady=(0, 12))
    def account_changed(*_):
        if provider.get() == 'OpenRouter':
            key_frame.grid()
            account_note.configure(text='Uses your OpenRouter credits. Copilot and Claude subscriptions are separate.')
        else:
            key_frame.grid_remove()
            account_note.configure(text='Uses the Claude Code login and billing already configured on this computer.')
    provider.trace_add('write', account_changed)
    account_changed()
    status = tk.StringVar(value='Everything OWI needs is included. No developer tools to install.')
    ttk.Label(frame, textvariable=status, wraplength=480).grid(sticky='ew', pady=(12, 18))
    messages = queue.Queue()
    active = False
    finished = False
    def submit():
        nonlocal active
        if finished:
            root.destroy()
            return
        if active:
            return
        choices = (CLIENTS[client.get()], project.get().strip(),
                   'openrouter' if provider.get() == 'OpenRouter' else 'claude', key.get())
        key.set('')
        active = True
        button.configure(state='disabled')
        status.set('Connecting… This can take a moment on the first run.')
        def work():
            try:
                messages.put((True, connect(*choices)))
            except ValueError as error:
                messages.put((False, str(error)))
            except Exception:
                messages.put((False, 'Could not complete setup. Check that the project is writable and your account is available.'))
        threading.Thread(target=work, daemon=False).start()
        root.after(100, poll)
    def poll():
        nonlocal active, finished
        try:
            finished, message = messages.get_nowait()
        except queue.Empty:
            root.after(100, poll)
            return
        active = False
        status.set(message)
        button.configure(state='normal', text='Finish' if finished else 'Connect')
    button = ttk.Button(frame, text='Connect', command=submit)
    button.grid(sticky='e')
    root.protocol('WM_DELETE_WINDOW', lambda: None if active else root.destroy())
    return root


def main():
    if platform.frozen() and os.name == 'nt':
        import ctypes
        # Setup is graphical. MCP retains its real stdin/stdout protocol streams.
        sys.stdout = open(os.devnull, 'w')
        sys.stderr = open(os.devnull, 'w')
        ctypes.windll.kernel32.FreeConsole()
    installed = install_copy()
    if installed:
        subprocess.Popen([str(installed), 'setup'], env=platform.external_env(),
            **platform.process_options())
        return 0
    root = window()
    root.mainloop()
    return 0
