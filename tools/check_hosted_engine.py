#!/usr/bin/env python3
"""Real hosted HTTP -> isolated child -> Rust routing/feedback. Fake providers only."""
import json
import os
from pathlib import Path
import tempfile
import threading
import urllib.request
from wsgiref.simple_server import make_server
import owi_bridge as bridge
import owi_openrouter as router
from owi_hosted import Service


def main():
    os.environ['OWI_BINARY'] = str(Path(os.environ['OWI_BINARY']).resolve())
    with tempfile.TemporaryDirectory() as temporary:
        root = Path(temporary)
        accounts = []
        for name in ('alice','bob'):
            accounts.append({'id':name, 'token':name+'-'+'x'*40,
                'openrouter_api_key':'fake-provider-key-'+name})
            home = root/'users'/name
            bridge.do.bootstrap(home)
            fixture = {'data':[{'id':model,'context_length':200000,'pricing':{
                'prompt':'0.000001','completion':'0.000005','request':'0'}}
                for model,_ in router.DEFAULT_MODELS.values()]}
            config = router.configure(home, fixture)
            settings = bridge.runtime.settings(home)
            runners = {}
            for model in config['models']:
                runners[model] = 'echo '+name+' Friday'
                settings['formats'][model] = 'text'
            (home/'runners.json').write_text(json.dumps(runners))
            bridge.runtime.save_settings(home, settings)
        service = Service(accounts, root)
        server = make_server('127.0.0.1', 0, service)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            for account in accounts:
                request = urllib.request.Request('http://127.0.0.1:'+str(server.server_port)+'/api/work',
                    data=json.dumps({'task':'Write a delivery confirmation',
                        'context':'سلام — café', 'checks':['contains:Friday']}).encode(),
                    headers={'Content-Type':'application/json','Authorization':'Bearer '+account['token'],
                        'Idempotency-Key':'same-request-id-different-account'})
                with urllib.request.urlopen(request, timeout=30) as response:
                    result = json.load(response)
                assert result['verdict'] == 'accepted', result
                assert account['id']+' Friday' in result['output'], result
                event = json.loads((root/'users'/account['id']/'last-outcome.json').read_text())['event']
                assert event['validation_kind'] == 'deterministic', event
            print('Hosted HTTP, per-user child execution, Rust routing and private feedback passed; no provider calls.')
        finally:
            server.shutdown()
            server.server_close()
            service.pool.shutdown(wait=True)


if __name__ == '__main__':
    main()
