"""MCP, noninteractive execution, cancellation and OpenRouter adapter checks. No paid calls."""
import importlib.machinery
import importlib.util
import io
import json
import os
from pathlib import Path
import shlex
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from unittest.mock import patch

import owi_bridge as bridge
import owi_openrouter as router


def load(name):
    loader=importlib.machinery.SourceFileLoader(name,str(Path(__file__).parent/name))
    spec=importlib.util.spec_from_loader(name,loader)
    mod=importlib.util.module_from_spec(spec);loader.exec_module(mod);return mod

mcp=load('owi-mcp');connect=load('owi-connect')


def catalog():
    return {'data':[{'id':model,'context_length':200000,'pricing':{
        'prompt':'0.000001','completion':'0.000005','request':'0'}}
        for model,_ in router.DEFAULT_MODELS.values()]}


class IntegrationTests(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory();self.home=Path(self.temp.name)
        self.env=patch.dict(os.environ,{'OWI_BINARY':sys.executable},clear=True);self.env.start()
        (self.home/'index.sqlite').touch()
        self.worker='worker:haiku-4-5/text'
        self.candidate={'worker_id':self.worker,'billing':{'mode':'api'},
                        'cost':{'expected_accepted_cost_micros':1000}}

    def tearDown(self):self.env.stop();self.temp.cleanup()

    def runner(self, text='hello', code=None):
        script=self.home/'worker script.py'
        script.write_text(code or ('import sys\nsys.stdin.read()\nprint('+repr(text)+')\n'))
        (self.home/'runners.json').write_text(json.dumps({'haiku-4-5':shlex.join([sys.executable,str(script)])}))

    def execute(self,args):
        with patch.object(bridge.do,'choose',return_value=({}, {}, [self.candidate])), \
             patch.object(bridge.do,'record_outcome') as record:
            value=bridge.Bridge(self.home).execute(args,threading.Event())
            return value,record

    def test_single_task_does_not_prompt_or_call_planner(self):
        self.runner()
        with patch('builtins.input',side_effect=AssertionError('no prompt')), \
             patch.object(bridge.do,'auto_checklist',side_effect=AssertionError('no planner')):
            value,record=self.execute({'task':'write a greeting'})
        self.assertEqual(value['output'],'hello\n');self.assertEqual(value['verdict'],'undecided')
        self.assertEqual(len(value['usage']['runs']),1);record.assert_not_called()

    def test_checked_task_records_real_validation_kind_and_learns(self):
        self.runner('invalid json')
        for _ in range(2):
            value,record=self.execute({'task':'extract JSON','checks':['json']})
            self.assertEqual(value['verdict'],'rejected')
            self.assertEqual(record.call_args.kwargs['validation_kind'],'deterministic')
        self.assertEqual(value['checks']['agent_update']['action'],'probation_started')

    def test_unknown_checks_do_not_become_passes(self):
        self.runner('Friday')
        value,record=self.execute({'task':'write a reply','checks':['contains:Friday','polite','regex:(a+)+$']})
        self.assertEqual(value['verdict'],'undecided');record.assert_not_called()

    def test_cost_limit_blocks_before_execution(self):
        self.runner();self.candidate['cost']['expected_accepted_cost_micros']=100001
        with patch.object(bridge,'execute_command',side_effect=AssertionError('must not run')):
            with self.assertRaisesRegex(ValueError,'No configured worker'):self.execute({'task':'write'})

    def test_confidential_summary_redacted_and_no_helper(self):
        self.runner()
        with patch.object(bridge.do,'choose',return_value=({}, {}, [])) as choose:
            with self.assertRaises(ValueError):bridge.Bridge(self.home).execute({'task':'secret text','privacy':'secret'},threading.Event())
        self.assertEqual(choose.call_args.args[1],'[confidential task]')
        self.assertEqual(choose.call_args.args[-1],'secret')

    def test_expired_openrouter_profile_blocks_before_allocation(self):
        (self.home/'bridge.json').write_text(json.dumps({'catalog_expires_at':0}))
        with patch.object(bridge.do,'choose',side_effect=AssertionError('must not allocate')), \
             patch.object(router,'configure',side_effect=ValueError('expired catalog could not refresh')):
            with self.assertRaisesRegex(ValueError,'expired'):bridge.Bridge(self.home).execute({'task':'write'},threading.Event())

    def test_cancellation_stops_running_command_and_records_unknown_charge(self):
        self.runner(code='import time,sys\nsys.stdin.read()\ntime.sleep(30)\n')
        cancel=threading.Event();timer=threading.Timer(.3,cancel.set);timer.start()
        start=time.monotonic()
        with self.assertRaisesRegex(ValueError,'cancelled'):
            bridge.execute_command(self.home,'haiku-4-5','task',[],cancel)
        timer.join();self.assertLess(time.monotonic()-start,3)
        usage=bridge.runtime.recent_usage(self.home)[0]
        self.assertEqual(usage['runs'][0]['exit_code'],-1)
        self.assertIsNone(usage['runs'][0]['usage']['reported_charge_micros'])

    def test_worker_cannot_recursively_spawn_owi_work(self):
        self.runner(code='import os,sys\nsys.stdin.read()\nprint(os.environ.get("OWI_NESTED_WORKER"))\n')
        value,_=self.execute({'task':'write'});self.assertEqual(value['output'].strip(),'1')

    def test_runtime_stderr_is_not_returned_to_host(self):
        self.runner(code='import sys\nsys.stdin.read()\nprint("FAKE-SECRET",file=sys.stderr)\nsys.exit(1)\n')
        value,_=self.execute({'task':'write'})
        self.assertEqual(value['verdict'],'execution_error');self.assertNotIn('FAKE-SECRET',json.dumps(value))

    def test_separate_clients_cannot_race_shared_scratch_records(self):
        with bridge.operation_lock(self.home):
            with self.assertRaisesRegex(ValueError,'another client'):
                bridge.Bridge(self.home).execute({'task':'write'},threading.Event())
        with bridge.operation_lock(self.home):
            pass

    def test_strict_arguments(self):
        for value in ({'task':'x','command':'evil'}, {'task':'x','skill':[]}, {'task':'x','checks':'yes'}, {'task':''}):
            with self.assertRaises(ValueError):bridge.validate(value)

    def test_mcp_discovery_needs_no_engine_or_workspace(self):
        with tempfile.TemporaryDirectory() as home:
            messages=[{'jsonrpc':'2.0','id':1,'method':'initialize','params':{'protocolVersion':'2025-11-25'}},
                      {'jsonrpc':'2.0','method':'notifications/initialized'},
                      {'jsonrpc':'2.0','id':2,'method':'tools/list'},
                      {'jsonrpc':'2.0','id':3,'method':'ping'}]
            result=subprocess.run([sys.executable,str(Path(__file__).parent/'owi-mcp'),'--home',home],
                input='\n'.join(map(json.dumps,messages))+'\n',text=True,capture_output=True,timeout=5)
            self.assertEqual(result.returncode,0,result.stderr)
            replies=[json.loads(line) for line in result.stdout.splitlines()]
            self.assertEqual([r['id'] for r in replies],[1,2,3])
            self.assertEqual(replies[1]['result']['tools'][0]['name'],'owi_work')
            self.assertEqual(list(Path(home).iterdir()),[])

    def test_mcp_errors_and_nonblocking_cancel(self):
        class Fake:
            def execute(self,args,event):
                event.wait(3)
                if event.is_set():raise ValueError('Task cancelled')
                return {'output':'done'}
        out=io.StringIO();server=mcp.Server(Fake(),out)
        server.handle({'jsonrpc':'2.0','id':0,'method':'tools/list'})
        server.handle({'jsonrpc':'2.0','id':1,'method':'initialize','params':{'protocolVersion':'2025-06-18'}})
        server.handle({'jsonrpc':'2.0','method':'notifications/initialized'})
        server.handle({'jsonrpc':'2.0','id':2,'method':'tools/call','params':{'name':'owi_work','arguments':{'task':'write'}}})
        server.handle({'jsonrpc':'2.0','id':3,'method':'ping'})
        server.handle({'jsonrpc':'2.0','method':'notifications/cancelled','params':{'requestId':2}})
        server.close()
        replies={r['id']:r for r in map(json.loads,out.getvalue().splitlines())}
        self.assertEqual(replies[0]['error']['code'],-32002)
        self.assertEqual(replies[3]['result'],{})
        self.assertTrue(replies[2]['result']['isError'])

    def test_config_merge_preserves_other_servers_and_rejects_conflicts(self):
        path=self.home/'mcp.json';path.write_text(json.dumps({'servers':{'other':{'command':'other'}},'inputs':[]}))
        definition=connect.definition(self.home,100000)
        connect.merge_config(path,'servers',definition)
        self.assertEqual(json.loads(path.read_text())['servers']['other'],{'command':'other'})
        connect.merge_config(path,'servers',definition)
        original=path.read_bytes()
        with self.assertRaises(ValueError):connect.merge_config(path,'servers',{'command':'different'})
        self.assertEqual(path.read_bytes(),original)
        self.assertNotIn('OPENROUTER_API_KEY',path.read_text())

    def test_openrouter_preserves_task_and_usage_without_fallback(self):
        profile={'model':'openai/gpt-5-mini','provider':'openai','expires_at':time.time()+300,
                 'input_rate':1000000,'output_rate':5000000}
        response={'choices':[{'finish_reason':'stop','message':{'content':'done'}}],
                  'usage':{'prompt_tokens':50,'completion_tokens':10,'cost':.0000121,
                           'prompt_tokens_details':{'cached_tokens':20}}}
        with patch.dict(os.environ,{'OPENROUTER_API_KEY':'fake-key'}), \
             patch.object(router,'request_json',return_value=response) as request:
            result=router.complete(profile,'literal $(never-run)')
        body=request.call_args.args[1]
        self.assertEqual(body['messages'][0]['content'],'literal $(never-run)')
        self.assertFalse(body['provider']['allow_fallbacks'])
        self.assertEqual(body['provider']['only'],['openai']);self.assertNotIn('models',body)
        self.assertEqual(result['usage']['reported_charge_micros'],13)
        self.assertEqual(result['usage']['cache_read_input_tokens'],20)
        self.assertIsNone(result['usage']['api_equivalent_micros'])
        self.assertNotIn('fake-key',json.dumps(result))

    def test_missing_key_or_expired_profile_makes_no_network_call(self):
        profile={'expires_at':0}
        with patch.object(router,'request_json',side_effect=AssertionError('network')):
            with self.assertRaisesRegex(ValueError,'missing'):router.complete(profile,'x')
            with patch.dict(os.environ,{'OPENROUTER_API_KEY':'fake'}):
                with self.assertRaisesRegex(ValueError,'expired'):router.complete(profile,'x')

    def test_openrouter_catalog_has_distinct_identities_and_assumed_evidence(self):
        seed,profiles=router.catalog_seed(catalog(),time.time())
        self.assertEqual(len(profiles),2)
        self.assertTrue(all(w['id'].startswith('worker:or-') for w in seed['worker_profiles']))
        self.assertTrue(all(e['benchmark_id']=='benchmark:assumed-for-demonstration' for e in seed['evidence']))
        self.assertTrue(all(o['provider']=='openrouter' for o in seed['provider_offerings']))
        with patch.object(bridge.do,'owi'):
            config=router.configure(self.home,catalog())
        self.assertEqual(set(config['models']),set(bridge.runtime.settings(self.home)['billing']))
        self.assertNotIn('fake-key',''.join(p.read_text() for p in self.home.glob('*.json')))

if __name__=='__main__':unittest.main(verbosity=2)
