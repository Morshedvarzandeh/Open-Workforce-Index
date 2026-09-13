"""Noninteractive work execution shared by MCP clients. No GUI or human prompts."""
from contextlib import contextmanager
import importlib.machinery
import importlib.util
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import owi_platform as platform

TOOLS = platform.resources()/'tools'
loader = importlib.machinery.SourceFileLoader('owi_bridge_do', str(TOOLS / 'owi-do'))
spec = importlib.util.spec_from_loader('owi_bridge_do', loader)
do = importlib.util.module_from_spec(spec)
loader.exec_module(do)
runtime = do.runtime


def validate(arguments):
    if not isinstance(arguments, dict) or set(arguments) - {'task','context','checks','skill','privacy'}:
        raise ValueError('Unknown task argument')
    task = arguments.get('task')
    if not isinstance(task, str) or not task.strip() or len(task) > 20000:
        raise ValueError('task must contain 1–20000 characters')
    context = arguments.get('context', '')
    if not isinstance(context, str) or len(context) > 80000:
        raise ValueError('context must contain at most 80000 characters')
    checks = arguments.get('checks', [])
    if not isinstance(checks, list) or len(checks) > 10 or any(
        not isinstance(s, str) or not 0 < len(s) <= 500 for s in checks
    ):
        raise ValueError('Provide at most ten short checks')
    privacy = arguments.get('privacy', 'private_metadata')
    if privacy not in do.PRIVACY.values():
        raise ValueError('Invalid privacy level')
    if 'skill' in arguments and not isinstance(arguments['skill'], str):
        raise ValueError('skill must be a string')
    skill = arguments.get('skill') or do.classify(task)
    if not isinstance(skill, str) or skill not in do.SKILLS:
        raise ValueError('Unsupported task skill')
    return task, context, checks, skill, privacy


def stop_process(process):
    if process.poll() is not None:
        return
    if os.name == 'nt':
        subprocess.run(['taskkill', '/PID', str(process.pid), '/T', '/F'],
                       capture_output=True, timeout=10)
    else:
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
    process.communicate()


def execute_command(home, model, payload, checks, cancel, timeout=120):
    command = do.resolve_runner(home, model)
    if not command:
        raise ValueError('No eligible configured runner')
    if cancel.is_set():
        raise ValueError('Task cancelled')
    run_id, lessons = runtime.begin_run(home, model, 'worker', checks)
    if lessons:
        payload += '\n\nWorking reminders for the current requirements:\n' + '\n'.join(lessons)
    usage = {'source':'unreported', 'reported_charge_micros':None}
    code = -1
    try:
        with tempfile.TemporaryDirectory(prefix='owi-bridge-run-') as scratch:
            process = subprocess.Popen(command, shell=True, stdin=subprocess.PIPE,
                stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, cwd=scratch,
                start_new_session=os.name != 'nt',
                env={**platform.external_env(),'OWI_NESTED_WORKER':'1'})
            started = time.monotonic()
            first = True
            while True:
                if cancel.is_set() or time.monotonic()-started > timeout:
                    stop_process(process)
                    raise ValueError('Task cancelled' if cancel.is_set() else 'Task timed out')
                try:
                    stdout, stderr = process.communicate(payload if first else None, timeout=.2)
                    break
                except subprocess.TimeoutExpired:
                    first = False
            format_name = runtime.settings(home)['formats'].get(model, 'text')
            # Built-in Claude commands explicitly opt into JSON telemetry.
            import shlex
            argv = shlex.split(command) if os.name != 'nt' or command.startswith('claude ') else []
            if argv and Path(argv[0]).name == 'claude' and (
                '--output-format=json' in argv or any(a == '--output-format' and i+1 < len(argv)
                and argv[i+1] == 'json' for i,a in enumerate(argv))):
                format_name = 'claude-json'
            output, usage, malformed = runtime.decode_output(stdout, format_name)
            code = process.returncode or (1 if malformed else 0)
            # Do not return raw stderr: a custom runner could echo credentials.
            return output, code, run_id
    finally:
        runtime.finish_run(home, run_id, code, usage)


@contextmanager
def operation_lock(home):
    """Serialize MCP operations across different host clients using this home."""
    home = Path(home)
    home.mkdir(parents=True, exist_ok=True)
    with (home/'bridge.lock').open('a+b') as handle:
        try:
            if os.name == 'nt':
                import msvcrt
                if handle.tell() == 0:
                    handle.write(b'0'); handle.flush()
                handle.seek(0)
                msvcrt.locking(handle.fileno(), msvcrt.LK_NBLCK, 1)
            else:
                import fcntl
                fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError:
            raise ValueError('OWI is busy in another client; wait for its active task') from None
        try:
            yield
        finally:
            if os.name == 'nt':
                handle.seek(0)
                msvcrt.locking(handle.fileno(), msvcrt.LK_UNLCK, 1)
            else:
                fcntl.flock(handle, fcntl.LOCK_UN)


class Bridge:
    def __init__(self, home, ceiling=100000, timeout=120):
        self.home = Path(home)
        self.ceiling = ceiling
        self.timeout = timeout

    def status(self):
        config_path = self.home / 'bridge.json'
        config = json.loads(config_path.read_text()) if config_path.exists() else {}
        return {'ready': (self.home/'index.sqlite').exists() and bool(os.environ.get('OWI_BINARY') and Path(os.environ['OWI_BINARY']).is_file()),
                'mode':'automatic tasks, one attempt, local checks',
                'estimated_cost_ceiling_micros':self.ceiling,
                'models':config.get('models'),
                'note':'Host chat decides when to invoke OWI. Provider time and charges are additional.'}

    def execute(self, arguments, cancel):
        validate(arguments)
        with operation_lock(self.home):
            return self._execute(arguments, cancel)

    def _execute(self, arguments, cancel):
        task, context, checks, skill, privacy = validate(arguments)
        start = time.monotonic()
        operation = runtime.start_operation()
        if not (self.home/'index.sqlite').exists():
            raise ValueError('OWI needs one-time setup: run tools/owi-connect --prepare')
        if not os.environ.get('OWI_BINARY'):
            raise ValueError('Compiled engine missing; run tools/owi-connect --prepare')
        config_path = self.home/'bridge.json'
        config = json.loads(config_path.read_text()) if config_path.exists() else {}
        if time.time() >= config.get('catalog_expires_at', float('inf')):
            from owi_openrouter import configure
            config = configure(self.home)
            if cancel.is_set():
                raise ValueError('Task cancelled')
        # No LLM planner. The host supplies requirements; JSON has a cheap default gate.
        if not checks and skill == 'skill:structured-extraction':
            checks = ['json']
        full_task = task + ('\n\nSupplied context:\n'+context if context else '')
        if cancel.is_set():
            raise ValueError('Task cancelled')
        options = {k:config[k] for k in ('snapshot_id','at_epoch_ms') if k in config}
        # Confidential text must not enter the engine's persisted task summary.
        _, _, eligible = do.choose(self.home, '[confidential task]' if privacy in
            ('secret','confidential_content') else task, skill,
            1500+len(full_task)//4, 2048, 0, privacy, **options)
        configured_models = config.get('models')
        if configured_models is not None:
            eligible = [c for c in eligible if c['worker_id'].split(':')[1].split('/')[0] in configured_models]
        eligible = do.execution_candidates(self.home, eligible)
        candidate = next((c for c in eligible if do.resolve_runner(self.home,
            c['worker_id'].split(':')[1].split('/')[0]) and
            c['cost']['expected_accepted_cost_micros'] <= self.ceiling), None)
        if candidate is None:
            raise ValueError('No configured worker meets the task, billing and estimated-cost limits')
        routing_ms = round((time.monotonic()-start)*1000, 2)
        worker = candidate['worker_id']
        model = worker.split(':')[1].split('/')[0]
        if model.startswith('or-') and (not os.environ.get('OPENROUTER_API_KEY') or os.environ['OPENROUTER_API_KEY'].startswith('${')):
            raise ValueError('OPENROUTER_API_KEY is missing or unresolved in the MCP client environment')
        payload = do.build_payload(task, context, checks, do.prevention_notes(self.home, skill))
        output, code, run_id = execute_command(self.home, model, payload, checks, cancel, self.timeout)
        if cancel.is_set():
            raise ValueError('Task cancelled')
        report = {'items':[], 'verdict':'undecided', 'judge':None}
        if code == 0 and output.strip():
            for item in checks:
                # Unbounded regular expressions and subjective judging are outside fast mode.
                if item.strip().lower().split(':',1)[0] in ('json','contains','min-words','python'):
                    kind, passed, note = do.check_item(item, output)
                else:
                    kind, passed, note = 'judged', None, 'requires host review'
                report['items'].append({'item':item,'kind':kind,'pass':passed,'note':note})
            if any(i['pass'] is False for i in report['items']):
                report['verdict'] = 'rejected'
            elif report['items'] and all(i['pass'] is True for i in report['items']):
                report['verdict'] = 'accepted'
            if report['verdict'] != 'undecided':
                # Record the checklist evidence; no invented human approval.
                do.record_outcome(self.home, worker, skill, report['verdict']=='accepted',
                    'worker' if report['verdict']=='rejected' else None,
                    task=task, checklist=report['items'], inspection='full', privacy=privacy,
                    validation_kind='deterministic')
            report['agent_update'] = runtime.observe_checks(self.home, model, run_id, report, privacy)
        return {'output':output[-100000:], 'worker':worker,
                'verdict':report['verdict'] if code == 0 and output.strip() else 'execution_error',
                'checks':report, 'usage':runtime.operation_usage(self.home, operation),
                'routing_ms':routing_ms, 'elapsed_ms':round((time.monotonic()-start)*1000,2),
                'note':'Checks cover declared requirements only. Host reviews meaning and applies any edits.'}
