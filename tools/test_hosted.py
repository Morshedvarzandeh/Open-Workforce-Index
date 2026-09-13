"""Hosted authentication, isolation, quotas, MCP sessions and Telegram retry checks."""
import io
import json
import os
from pathlib import Path
import tempfile
import threading
import time
import unittest
from unittest.mock import patch, Mock
from owi_hosted import Service, RequestError

TOKEN_A = 'test-account-a-'+'a'*32
TOKEN_B = 'test-account-b-'+'b'*32


class HostedTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.calls, self.messages = [], []
        def execute(account, task, cancel):
            self.calls.append((account['id'], dict(task)))
            return {'output':'Friday — سلام', 'verdict':'undecided', 'usage':{}}
        self.service = Service([
            {'id':'alice', 'token':TOKEN_A, 'openrouter_api_key':'fake-provider-key-for-alice', 'daily_tasks':2, 'telegram_user_id':1001},
            {'id':'bob', 'token':TOKEN_B, 'openrouter_api_key':'fake-provider-key-for-bob'}],
            Path(self.temp.name), public_url='https://owi.example',
            telegram_token='test-bot-token', telegram_secret='s'*32,
            executor=execute, sender=lambda chat, text:self.messages.append((chat, text)))

    def tearDown(self):
        self.service.pool.shutdown(wait=True)
        self.temp.cleanup()

    def request(self, path, method='GET', body=None, token=TOKEN_A, **headers):
        payload = json.dumps(body).encode() if body is not None else b''
        env = {'PATH_INFO':path, 'REQUEST_METHOD':method, 'CONTENT_TYPE':'application/json',
            'CONTENT_LENGTH':str(len(payload)), 'wsgi.input':io.BytesIO(payload),
            'HTTP_AUTHORIZATION':'Bearer '+token, **headers}
        response = {}
        def start(status, values):
            response.update(status=int(status.split()[0]), headers=dict(values))
        raw = b''.join(self.service(env, start))
        response['body'] = json.loads(raw) if raw and response['headers']['Content-Type']=='application/json' else raw
        return response

    def initialize(self, token=TOKEN_A):
        result = self.request('/mcp', 'POST', {'jsonrpc':'2.0','id':1,'method':'initialize',
            'params':{'protocolVersion':'2025-11-25'}}, token=token)
        self.assertEqual(result['status'], 200)
        return result['headers']['MCP-Session-Id']

    def test_authentication_origin_and_request_limits_precede_execution(self):
        self.assertEqual(self.request('/api/work', 'POST', {'task':'write'}, token='invalid')['status'],401)
        self.assertEqual(self.request('/api/status', HTTP_ORIGIN='https://attacker.example')['status'],403)
        self.assertEqual(self.request('/api/work','POST',{'task':'write'}, CONTENT_LENGTH='600001')['status'],413)
        self.assertEqual(self.request('/api/work','POST',{'task':'write'})['status'],400)
        self.assertEqual(self.request('/api/status','POST',{'task':'write'})['status'],405)
        self.assertEqual(self.calls, [])

    def test_account_quotas_and_duplicate_requests_do_not_cross_users(self):
        task = {'task':'write a confirmation'}
        for identifier in ('one', 'two'):
            self.assertEqual(self.request('/api/work','POST',task,HTTP_IDEMPOTENCY_KEY=identifier)['status'],200)
        self.assertEqual(self.request('/api/work','POST',task,HTTP_IDEMPOTENCY_KEY='one')['status'],409)
        self.assertEqual(self.request('/api/work','POST',task,HTTP_IDEMPOTENCY_KEY='three')['status'],429)
        self.assertEqual(self.request('/api/work','POST',task,token=TOKEN_B,HTTP_IDEMPOTENCY_KEY='one')['status'],200)
        self.assertEqual([owner for owner,_ in self.calls], ['alice','alice','bob'])
        self.assertEqual(self.request('/api/status')['body']['remaining_tasks_today'],0)
        self.assertEqual(self.request('/api/status',token=TOKEN_B)['body']['remaining_tasks_today'],19)

    def test_confidential_input_is_rejected_before_quota_or_execution(self):
        result=self.request('/api/work','POST',{'task':'secret','privacy':'secret'},HTTP_IDEMPOTENCY_KEY='one')
        self.assertEqual(result['status'],400)
        self.assertEqual(self.calls,[])
        self.assertEqual(self.request('/api/status')['body']['remaining_tasks_today'],2)

    def test_mcp_sessions_are_bound_to_account_and_protocol(self):
        session = self.initialize()
        call = {'jsonrpc':'2.0','id':2,'method':'tools/call','params':{'name':'owi_work','arguments':{'task':'write'}}}
        self.assertEqual(self.request('/mcp','POST',call)['status'],400)
        self.assertEqual(self.request('/mcp','POST',call,token=TOKEN_B,HTTP_MCP_SESSION_ID=session)['status'],404)
        self.assertEqual(self.request('/mcp','POST',call,HTTP_MCP_SESSION_ID=session,HTTP_MCP_PROTOCOL_VERSION='2025-06-18')['status'],400)
        result = self.request('/mcp','POST',call,HTTP_MCP_SESSION_ID=session)
        self.assertFalse(result['body']['result']['isError'])
        duplicate = self.request('/mcp','POST',call,HTTP_MCP_SESSION_ID=session)
        self.assertTrue(duplicate['body']['result']['isError'])
        other_session = self.initialize()
        self.assertFalse(self.request('/mcp','POST',call,HTTP_MCP_SESSION_ID=other_session)['body']['result']['isError'])
        self.assertEqual(len(self.calls),2)

    def test_mcp_discovery_and_notifications_do_not_spend_or_queue(self):
        session=self.initialize()
        listing=self.request('/mcp','POST',{'jsonrpc':'2.0','id':2,'method':'tools/list'},HTTP_MCP_SESSION_ID=session)
        self.assertEqual(len(listing['body']['result']['tools']),2)
        notification=self.request('/mcp','POST',{'jsonrpc':'2.0','method':'notifications/initialized'},HTTP_MCP_SESSION_ID=session)
        self.assertEqual(notification['status'],202)
        self.assertEqual(self.request('/mcp')['status'],405)
        self.assertEqual(self.calls,[])

    def test_active_task_and_cancellation_are_scoped_to_account(self):
        session=self.initialize()
        started=threading.Event()
        def execute(account, task, cancel):
            started.set()
            if not cancel.wait(3):
                raise AssertionError('cancellation did not arrive')
            return {'output':'cancelled'}
        self.service.executor=execute
        call={'jsonrpc':'2.0','id':7,'method':'tools/call','params':{'name':'owi_work','arguments':{'task':'write'}}}
        thread=threading.Thread(target=lambda:self.request('/mcp','POST',call,HTTP_MCP_SESSION_ID=session))
        thread.start()
        self.assertTrue(started.wait(1))
        self.assertEqual(self.request('/api/work','POST',{'task':'write'},HTTP_IDEMPOTENCY_KEY='other')['status'],429)
        cancel={'jsonrpc':'2.0','method':'notifications/cancelled','params':{'requestId':7}}
        self.assertEqual(self.request('/mcp','POST',cancel,token=TOKEN_B,HTTP_MCP_SESSION_ID=session)['status'],404)
        self.assertEqual(self.request('/mcp','POST',cancel,HTTP_MCP_SESSION_ID=session)['status'],202)
        thread.join(2)
        self.assertFalse(thread.is_alive())

    def update(self, update_id=1, user=1001, chat_type='private'):
        return {'update_id':update_id, 'message':{'from':{'id':user,'is_bot':False},
            'chat':{'id':user,'type':chat_type}, 'text':'write a confirmation'}}

    def test_telegram_requires_secret_private_chat_and_allowlisted_user(self):
        self.assertEqual(self.request('/telegram/webhook','POST',self.update())['status'],401)
        for update in (self.update(user=999),self.update(chat_type='group')):
            self.assertEqual(self.request('/telegram/webhook','POST',update,HTTP_X_TELEGRAM_BOT_API_SECRET_TOKEN='s'*32)['status'],200)
        self.assertEqual(self.calls,[])
        self.assertEqual(self.messages,[])

    def test_telegram_redelivery_never_executes_a_paid_task_twice(self):
        entered, release = threading.Event(), threading.Event()
        original = self.service.executor
        def execute(*args):
            entered.set()
            release.wait(2)
            return original(*args)
        self.service.executor = execute
        for _ in range(2):
            self.request('/telegram/webhook','POST',self.update(),HTTP_X_TELEGRAM_BOT_API_SECRET_TOKEN='s'*32)
            self.assertTrue(entered.wait(1))
        release.set()
        self.service.pool.shutdown(wait=True)
        self.assertEqual(len(self.calls),1)
        self.assertEqual(self.messages,[(1001,'Friday — سلام')])

    def test_child_environment_contains_only_the_current_provider_key(self):
        process=Mock()
        process.communicate.return_value=('{}','')
        process.returncode=0
        process.poll.return_value=0
        account=self.service.account('Bearer '+TOKEN_A)
        with patch.dict(os.environ,{'OWI_ACCOUNTS_JSON':'all keys','OWI_TELEGRAM_BOT_TOKEN':'bot secret','UNRELATED_SECRET':'secret'}), \
             patch('subprocess.Popen',return_value=process) as launch:
            self.service.execute_child(account,{'task':'write'},threading.Event())
        env=launch.call_args.kwargs['env']
        self.assertEqual(env['OPENROUTER_API_KEY'],'fake-provider-key-for-alice')
        self.assertNotIn('OWI_ACCOUNTS_JSON',env)
        self.assertNotIn('OWI_TELEGRAM_BOT_TOKEN',env)
        self.assertNotIn('UNRELATED_SECRET',env)
        self.assertIn('alice',env['HOME'])

    def test_status_and_assets_never_disclose_connection_or_provider_keys(self):
        for path in ('/','/app.js','/healthz','/api/status'):
            result=self.request(path)
            self.assertEqual(result['status'],200)
            text=str(result['body'])
            self.assertNotIn(TOKEN_A,text)
            self.assertNotIn('fake-provider-key-for-alice',text)


if __name__ == '__main__':
    unittest.main(verbosity=2)
