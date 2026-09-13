#!/usr/bin/env python3
"""Exercise real Rust import, identity validation, allocation and feedback with fake models."""
import json
import os
from pathlib import Path
import shlex
import sys
import tempfile
import threading
import owi_bridge as bridge
import owi_openrouter as router

binary=Path(os.environ['OWI_BINARY']).resolve()
os.environ['OWI_BINARY']=str(binary)
os.environ['OPENROUTER_API_KEY']='local-test-only-no-network'
with tempfile.TemporaryDirectory() as temporary:
    home=Path(temporary)
    bridge.do.bootstrap(home)
    fixture={'data':[{'id':model,'context_length':200000,
                     'pricing':{'prompt':'0.000001','completion':'0.000005','request':'0'}}
                    for model,_ in router.DEFAULT_MODELS.values()]}
    config=router.configure(home,fixture)
    script=home/'fake-worker.py'
    script.write_text('import sys\nsys.stdin.read()\nprint("Friday")\n')
    runners=json.loads((home/'runners.json').read_text())
    settings=bridge.runtime.settings(home)
    for model in config['models']:
        runners[model]=shlex.join([sys.executable,str(script)])
        settings['formats'][model]='text'
    (home/'runners.json').write_text(json.dumps(runners))
    bridge.runtime.save_settings(home,settings)
    result=bridge.Bridge(home).execute({'task':'Write a delivery confirmation',
        'checks':['contains:Friday']},threading.Event())
    assert result['verdict']=='accepted',result
    assert result['worker'].startswith('worker:or-'),result
    event=json.loads((home/'last-outcome.json').read_text())['event']
    assert event['validation_kind']=='deterministic',event
    assert event['worker_id']==result['worker'],event
    print('Real engine bridge: import, identities, allocation and deterministic feedback passed; no provider calls.')
