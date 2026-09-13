"""OpenRouter text runner and versioned catalog import. Uses an existing API key."""
import argparse
from decimal import Decimal, ROUND_CEILING
import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import subprocess
import sys
import time
import urllib.error
import urllib.request
import owi_platform as platform

API = 'https://openrouter.ai/api/v1'
# Explicit model/provider selections; neither OpenRouter auto nor model fallback is used.
DEFAULT_MODELS = {'gpt-5-mini':('openai/gpt-5-mini','openai'),
                  'haiku-4-5':('anthropic/claude-haiku-4.5','anthropic')}


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        raise ValueError('Unexpected provider redirect')


def request_json(endpoint, body=None, key=None):
    headers = {'Content-Type':'application/json','X-OpenRouter-Title':'Open Workforce Index'}
    if key: headers['Authorization'] = 'Bearer '+key
    request = urllib.request.Request(API+endpoint,
        data=json.dumps(body).encode() if body is not None else None, headers=headers)
    try:
        handlers = [NoRedirect]
        if platform.frozen():
            import certifi
            import ssl
            handlers.append(urllib.request.HTTPSHandler(context=ssl.create_default_context(
                cafile=os.environ.get('SSL_CERT_FILE') or certifi.where())))
        with urllib.request.build_opener(*handlers).open(request, timeout=15 if body is None else 110) as response:
            content = response.read(5000001)
            if len(content)>5000000: raise ValueError('Provider response too large')
            return json.loads(content)
    except urllib.error.HTTPError as error:
        # Do not expose a provider error body, prompt, or key in application logs.
        raise ValueError(f'OpenRouter HTTP {error.code}; check account, model and provider access') from None
    except (urllib.error.URLError, TimeoutError):
        raise ValueError('OpenRouter connection failed') from None


def micros(value, factor=1000000):
    if value is None: return None
    try: number = Decimal(str(value))
    except Exception: return None
    if not number.is_finite() or number<0: return None
    return int((number*factor).to_integral_value(rounding=ROUND_CEILING))


def complete(profile, prompt):
    key = os.environ.get('OPENROUTER_API_KEY')
    if not key or key.startswith('${'): raise ValueError('OPENROUTER_API_KEY is missing or unresolved in the MCP server environment')
    if time.time() >= profile['expires_at']:
        raise ValueError('OpenRouter price declaration expired; refresh its connection setup')
    request = {'model':profile['model'], 'messages':[{'role':'user','content':prompt}],
        'stream':False, 'max_tokens':2048,
        'provider':{'only':[profile['provider']], 'order':[profile['provider']],
                    'allow_fallbacks':False, 'require_parameters':True,
                    'max_price':{'prompt':profile['input_rate']/1000000,
                                 'completion':profile['output_rate']/1000000}}}
    result = request_json('/chat/completions', request, key)
    if not isinstance(result, dict) or result.get('error'):
        raise ValueError('OpenRouter returned an error')
    choices = result.get('choices')
    if not isinstance(choices, list) or not choices:
        raise ValueError('OpenRouter did not return a completion')
    first = choices[0]
    message = first.get('message', {})
    if first.get('error') or first.get('finish_reason') != 'stop' or message.get('tool_calls'):
        raise ValueError('OpenRouter returned an incomplete or unsupported completion')
    output = message.get('content')
    if not isinstance(output,str) or not output.strip():
        raise ValueError('OpenRouter returned no text')
    usage = result.get('usage') or {}
    details = usage.get('prompt_tokens_details') or {}
    envelope = {'owi_usage_version':1, 'output':output, 'usage':{
        'input_tokens':usage.get('prompt_tokens'), 'output_tokens':usage.get('completion_tokens'),
        'cache_read_input_tokens':details.get('cached_tokens'),
        'cache_creation_input_tokens':details.get('cache_write_tokens'),
        # Credit usage is a provider report, not proof of a reconciled invoice.
        'reported_charge_micros':micros(usage.get('cost')), 'api_equivalent_micros':None}}
    return envelope


def catalog_seed(catalog, now):
    from owi_bridge import do
    by_id = {m['id']:m for m in catalog['data']}
    seed_template = json.loads((do.REPO/'examples/manager-scenario-seed.json').read_text())
    timestamp = time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime(now))
    source_digest = hashlib.sha256(json.dumps(catalog,sort_keys=True).encode()).hexdigest()
    version = hashlib.sha256((source_digest+str(now)).encode()).hexdigest()[:12]
    seed = dict(snapshot_id='snapshot:openrouter-'+version, created_at=timestamp,
        ontology_version='ontology:v1', source_revision='openrouter-models:'+source_digest,
        model_releases=[], provider_offerings=[], worker_profiles=[], evidence=[])
    profiles = {}
    for base,(model,provider) in DEFAULT_MODELS.items():
        if model not in by_id: continue
        row=by_id[model]; pricing=row['pricing']
        input_rate=micros(pricing.get('prompt'),10**12); output_rate=micros(pricing.get('completion'),10**12)
        request_rate=micros(pricing.get('request','0'))
        context=row.get('context_length')
        if input_rate is None or output_rate is None or request_rate != 0 or type(context) is not int or context<=0:
            continue
        short='or-'+base+'-'+version
        release='model:'+short; offering='offering:openrouter/'+short
        seed['model_releases'].append(dict(id=release,developer='unknown',model_family=model,
            released_at='unknown',context_window_tokens=context,source_url=API+'/models',
            artifact_sha256=source_digest,recorded_at=timestamp))
        seed['provider_offerings'].append(dict(id=offering,model_release_id=release,provider='openrouter',
            effective_from_epoch_ms=int(now*1000),effective_until_epoch_ms=int((now+86400)*1000),
            currency='USD',input_micros_per_million_tokens=input_rate,
            output_micros_per_million_tokens=output_rate,fixed_request_micros=0,
            quota_milliunits_per_request=0,context_window_tokens=context,source_url=API+'/models',recorded_at=timestamp))
        profile={'model':model,'provider':provider,'expires_at':now+86400,
                 'input_rate':input_rate,'output_rate':output_rate}
        policy_digest=hashlib.sha256(json.dumps(profile,sort_keys=True).encode()).hexdigest()
        for original in seed_template['worker_profiles']:
            if not original['id'].startswith('worker:'+base+'/') or original['id'].split('/')[-1] not in ('text','extract','plan'):
                continue
            worker={**original,'id':original['id'].replace('worker:'+base+'/', 'worker:'+short+'/'),
                'offering_id':offering, 'harness_id':'owi-openrouter-text', 'harness_version':'1.0.0',
                'execution_policy_sha256':policy_digest,'recorded_at':timestamp}
            parts=[release,offering,'openrouter',worker['harness_id'],worker['harness_version'],
                worker['reasoning_configuration'],worker['system_prompt_sha256'],worker['skill_pack_version'],
                worker['toolset_version'],worker['execution_policy_sha256']]
            worker['configuration_sha256']=hashlib.sha256(''.join(f'{len(p.encode())}:{p}' for p in parts).encode()).hexdigest()
            seed['worker_profiles'].append(worker)
            # Preserve the project's explicitly assumed demo prior, never copy private measured outcomes.
            for original_evidence in seed_template['evidence']:
                if original_evidence.get('worker_id') == original['id']:
                    evidence={**original_evidence,'id':original_evidence['id']+':'+short,
                        'worker_id':worker['id'],'model_release_id':release,'observed_at':timestamp,
                        'benchmark_id':'benchmark:assumed-for-demonstration',
                        'artifact_sha256':hashlib.sha256((do.REPO/'examples/manager-scenario-seed.json').read_bytes()).hexdigest(),
                        'source_url':'https://github.com/Morshedvarzandeh/Open-Workforce-Index/blob/main/examples/manager-scenario-seed.json'}
                    seed['evidence'].append(evidence)
        profiles[short]=profile
    if not profiles: raise ValueError('No supported OpenRouter model with usable text pricing was found')
    return seed, profiles


def configure(home, catalog=None):
    from owi_bridge import do, runtime
    home=Path(home); now=time.time()
    catalog = request_json('/models') if catalog is None else catalog
    seed,profiles=catalog_seed(catalog, now)
    version=seed['snapshot_id'].split(':')[-1]
    source=home/(version+'-catalog.json'); source.write_text(json.dumps(catalog,sort_keys=True))
    path=home/(version+'-seed.json'); path.write_text(json.dumps(seed,indent=2))
    do.owi('seed','--index',str(home/'index.sqlite'),'--input',str(path))
    runners_path=home/'runners.json'; runners=json.loads(runners_path.read_text()) if runners_path.exists() else {}
    settings=runtime.settings(home)
    config_path=home/'bridge.json'
    previous=json.loads(config_path.read_text()).get('models',[]) if config_path.exists() else []
    for old in previous:
        if old.startswith('or-'):
            settings['billing'].pop(old,None)
            settings['formats'].pop(old,None)
            runners.pop(old,None)
    for model,profile in profiles.items():
        profile_path=home/(model+'.json');profile_path.write_text(json.dumps(profile))
        args=[*platform.command('openrouter'),'--profile',str(profile_path.resolve())]
        runners[model]=subprocess.list2cmdline(args) if os.name=='nt' else shlex.join(args)
        settings['billing'][model]={'mode':'api','plan':'OpenRouter API'}
        settings['formats'][model]='owi-json'
    runners_path.write_text(json.dumps(runners,indent=2)+'\n')
    runtime.save_settings(home,settings)
    config={'snapshot_id':seed['snapshot_id'],'at_epoch_ms':int(now*1000),
            'catalog_expires_at':now+86400,'models':list(profiles)}
    (home/'bridge.json').write_text(json.dumps(config,indent=2))
    return config


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--profile',type=Path,required=True)
    args=parser.parse_args()
    try:
        prompt=sys.stdin.read(100001)
        if len(prompt)>100000: raise ValueError('Task exceeds runner input limit')
        print(json.dumps(complete(json.loads(args.profile.read_text()),prompt)))
    except (ValueError,OSError,KeyError,TypeError) as error:
        print(str(error) if isinstance(error,ValueError) else 'Invalid OpenRouter runner setup',file=sys.stderr)
        return 1
    return 0

if __name__=='__main__': raise SystemExit(main())
