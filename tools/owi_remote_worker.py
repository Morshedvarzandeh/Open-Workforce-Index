#!/usr/bin/env python3
"""Isolated hosted worker. Receives only one user's provider credentials."""
import argparse
import json
from pathlib import Path
import sys
import threading
import signal
from owi_bridge import Bridge, do, operation_lock
from owi_openrouter import configure


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--home', type=Path, required=True)
    parser.add_argument('--ceiling', type=int, default=100000)
    args = parser.parse_args()
    cancel = threading.Event()
    signal.signal(signal.SIGTERM, lambda *_: cancel.set())
    for stream in (sys.stdin, sys.stdout, sys.stderr):
        stream.reconfigure(encoding='utf-8')
    try:
        task = json.load(sys.stdin)
        with operation_lock(args.home):
            do.bootstrap(args.home)
            if not (args.home/'bridge.json').exists():
                configure(args.home)
        if cancel.is_set():
            raise ValueError('Task cancelled before execution.')
        result = Bridge(args.home, args.ceiling).execute(task, cancel)
        print(json.dumps(result))
    except (Exception, SystemExit) as error:
        message = str(error) if isinstance(error, ValueError) else 'Hosted worker setup or execution failed.'
        print(json.dumps({'error':message}))
        return 1
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
