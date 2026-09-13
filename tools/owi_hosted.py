"""Hosted OWI: authenticated HTTP MCP/API and an optional private Telegram bot.

Run behind HTTPS with a persistent volume. End users install no OWI software.
Private-beta accounts are provisioned by the operator; no public anonymous billing.
"""
from concurrent.futures import ThreadPoolExecutor
from contextlib import contextmanager
import hashlib
import hmac
import json
import os
from pathlib import Path
import re
import secrets
import sqlite3
import subprocess
import sys
import threading
import time
import urllib.request

import owi_platform as platform
from owi_bridge import validate

PROTOCOLS = ('2025-11-25', '2025-06-18', '2025-03-26')
TOOL_DEFINITIONS = platform.load_tool('owi-mcp').TOOLS


def mcp_key(session, identifier):
    return 'mcp:'+session+':'+hashlib.sha256(json.dumps(identifier).encode()).hexdigest()


class RequestError(Exception):
    def __init__(self, status, message):
        self.status, self.message = status, message


class Service:
    def __init__(self, accounts, home, public_url='', telegram_token='', telegram_secret='',
                 telegram_name='', origins=(), executor=None, sender=None):
        self.home = Path(home)
        self.home.mkdir(parents=True, exist_ok=True, mode=0o700)
        self.public_url = public_url.rstrip('/')
        self.origins = set(origins) | ({self.public_url} if self.public_url else set())
        self.accounts, self.telegram_accounts = {}, {}
        self.telegram_token, self.telegram_secret, self.telegram_name = telegram_token, telegram_secret, telegram_name
        self.executor = executor or self.execute_child
        self.sender = sender or self.send_telegram
        self.pool = ThreadPoolExecutor(max_workers=2)
        self.slots = threading.BoundedSemaphore(2)
        self.active_lock = threading.Lock()
        self.active = {}
        ids = set()
        for account in accounts:
            account = dict(account)
            identifier = account.get('id', '')
            token = account.get('token', '')
            key = account.get('openrouter_api_key', '')
            if not re.fullmatch(r'[a-z0-9_-]{1,64}', identifier) or identifier in ids:
                raise ValueError('Each hosted account needs a unique simple id.')
            if not isinstance(token, str) or len(token) < 32 or not isinstance(key, str) or len(key) < 16:
                raise ValueError('Configure a strong connection token and an OpenRouter key for each account.')
            daily = account.get('daily_tasks', 20)
            ceiling = account.get('estimated_cost_ceiling_micros', 100000)
            if type(daily) is not int or not 1 <= daily <= 1000 or type(ceiling) is not int or not 0 <= ceiling <= 10000000:
                raise ValueError('Invalid hosted account execution limits.')
            account.update(daily_tasks=daily, estimated_cost_ceiling_micros=ceiling)
            digest = hashlib.sha256(token.encode()).hexdigest()
            if digest in self.accounts:
                raise ValueError('Connection tokens must be unique.')
            account.pop('token')
            self.accounts[digest] = account
            ids.add(identifier)
            telegram_id = account.get('telegram_user_id')
            if telegram_id is not None:
                if type(telegram_id) is not int or telegram_id <= 0 or telegram_id in self.telegram_accounts:
                    raise ValueError('Telegram accounts must have distinct numeric user IDs.')
                self.telegram_accounts[telegram_id] = account
        if bool(telegram_token) != bool(telegram_secret) or (telegram_secret and len(telegram_secret) < 32):
            raise ValueError('Telegram needs a bot token and a strong webhook secret.')
        with self.database() as db:
            db.executescript('''
              CREATE TABLE IF NOT EXISTS requests (
                account TEXT NOT NULL, day TEXT NOT NULL, request_id TEXT NOT NULL,
                state TEXT NOT NULL, PRIMARY KEY(account, request_id));
              CREATE INDEX IF NOT EXISTS daily_requests ON requests(account, day);
              CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY, account TEXT NOT NULL, protocol TEXT NOT NULL, expires REAL NOT NULL);
            ''')

    @contextmanager
    def database(self):
        db = sqlite3.connect(self.home/'service.sqlite', timeout=10)
        try:
            yield db
            db.commit()
        except BaseException:
            db.rollback()
            raise
        finally:
            db.close()

    def account(self, authorization):
        if not authorization.startswith('Bearer '):
            raise RequestError(401, 'Connect with your OWI connection key.')
        digest = hashlib.sha256(authorization[7:].encode()).hexdigest()
        account = self.accounts.get(digest)
        if account is None:
            raise RequestError(401, 'The OWI connection key is invalid.')
        return account

    def reserve(self, account, request_id):
        if not re.fullmatch(r'[A-Za-z0-9:._-]{1,160}', request_id):
            raise RequestError(400, 'Invalid request identifier.')
        day = time.strftime('%Y-%m-%d', time.gmtime())
        with self.database() as db:
            db.execute('BEGIN IMMEDIATE')
            previous = db.execute('SELECT state FROM requests WHERE account=? AND request_id=?',
                (account['id'], request_id)).fetchone()
            if previous:
                raise RequestError(409, 'This request was already received. It will not run or charge again.')
            count = db.execute('SELECT COUNT(*) FROM requests WHERE account=? AND day=?',
                (account['id'], day)).fetchone()[0]
            if count >= account['daily_tasks']:
                raise RequestError(429, 'Your daily OWI task limit has been reached.')
            db.execute('INSERT INTO requests VALUES (?,?,?,?)', (account['id'], day, request_id, 'received'))

    def status(self, account):
        with self.database() as db:
            used = db.execute('SELECT COUNT(*) FROM requests WHERE account=? AND day=?',
                (account['id'], time.strftime('%Y-%m-%d', time.gmtime()))).fetchone()[0]
        return {'ready':True, 'mode':'hosted', 'remaining_tasks_today':max(0, account['daily_tasks']-used),
            'estimated_cost_ceiling_micros':account['estimated_cost_ceiling_micros'],
            'telegram_url':('https://t.me/'+self.telegram_name) if re.fullmatch(r'[A-Za-z0-9_]{5,32}',self.telegram_name) and account.get('telegram_user_id') else None,
            'note':'OpenRouter usage is billed to the configured account. The quote limit is not an invoice cap.'}

    def execute_child(self, account, arguments, cancel):
        home = self.home/'users'/account['id']
        home.mkdir(parents=True, exist_ok=True, mode=0o700)
        # Never inherit other users' keys, the service account list, or the bot token.
        env = {'PATH':os.environ.get('PATH', '/usr/local/bin:/usr/bin:/bin'),
               'HOME':str(home), 'PYTHONIOENCODING':'utf-8',
               'OWI_BINARY':os.environ.get('OWI_BINARY', ''),
               'OPENROUTER_API_KEY':account['openrouter_api_key']}
        for name in ('SystemRoot', 'WINDIR', 'COMSPEC', 'TEMP', 'TMP', 'SSL_CERT_FILE', 'LD_LIBRARY_PATH', 'LIBPATH'):
            if name in os.environ:
                env[name] = os.environ[name]
        process = subprocess.Popen([sys.executable, str(platform.resources()/'tools/owi_remote_worker.py'),
            '--home', str(home), '--ceiling', str(account['estimated_cost_ceiling_micros'])],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            text=True, encoding='utf-8', env=env, start_new_session=os.name != 'nt')
        payload, started, first = json.dumps(arguments), time.monotonic(), True
        try:
            while True:
                if cancel.is_set() or time.monotonic()-started > 150:
                    raise RequestError(408, 'Task cancelled or timed out. Consumed provider usage may still be billed.')
                try:
                    output, _ = process.communicate(payload if first else None, timeout=.2)
                    break
                except subprocess.TimeoutExpired:
                    first = False
            try:
                result = json.loads(output)
            except ValueError:
                raise RequestError(502, 'The hosted worker could not complete this task.') from None
            if process.returncode or 'error' in result:
                raise RequestError(502, result.get('error', 'The hosted worker failed.'))
            return result
        finally:
            if process.poll() is None:
                process.terminate()
                try:
                    process.communicate(timeout=20)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.communicate()

    def begin(self, account, arguments, request_id):
        try:
            validate(arguments)
        except ValueError as error:
            raise RequestError(400, str(error)) from None
        if arguments.get('privacy') in ('confidential_content', 'secret'):
            raise RequestError(400, 'This hosted connection uses cloud workers and cannot accept confidential or secret tasks.')
        if not self.slots.acquire(blocking=False):
            raise RequestError(429, 'OWI is busy. Try again after the current work finishes.')
        cancel = threading.Event()
        with self.active_lock:
            if (account['id'], request_id) in self.active:
                self.slots.release()
                raise RequestError(409, 'This request is already running. It will not run again.')
            if any(owner == account['id'] for owner, _ in self.active):
                self.slots.release()
                raise RequestError(429, 'This account already has an active task.')
            self.active[(account['id'], request_id)] = cancel
        try:
            self.reserve(account, request_id)
        except BaseException:
            self.finish(account, request_id, None)
            raise
        return cancel

    def finish(self, account, request_id, state):
        with self.active_lock:
            self.active.pop((account['id'], request_id), None)
        self.slots.release()
        if state:
            with self.database() as db:
                db.execute('UPDATE requests SET state=? WHERE account=? AND request_id=?',
                    (state, account['id'], request_id))

    def work(self, account, arguments, request_id):
        cancel = self.begin(account, arguments, request_id)
        state = 'failed'
        try:
            result = self.executor(account, arguments, cancel)
            state = 'completed'
            return result
        finally:
            self.finish(account, request_id, state)

    def mcp(self, account, body, session):
        if not isinstance(body, dict) or body.get('jsonrpc') != '2.0' or not isinstance(body.get('method'), str):
            raise RequestError(400, 'Invalid JSON-RPC request.')
        method, identifier, params = body['method'], body.get('id'), body.get('params', {})
        if not isinstance(params, dict) or ('id' in body and type(identifier) not in (int, str)):
            raise RequestError(400, 'Invalid JSON-RPC parameters.')
        if 'id' not in body:
            if method == 'notifications/cancelled':
                with self.active_lock:
                    event = self.active.get((account['id'], mcp_key(session, params.get('requestId'))))
                    if event:
                        event.set()
            return 202, None
        result = {}
        if method == 'initialize':
            version = params.get('protocolVersion')
            result = {'protocolVersion':version if version in PROTOCOLS else PROTOCOLS[0],
                'capabilities':{'tools':{}}, 'serverInfo':{'name':'owi-hosted', 'version':'0.1.0'}}
        elif method == 'tools/list':
            result = {'tools':TOOL_DEFINITIONS}
        elif method == 'tools/call':
            try:
                if params.get('name') == 'owi_status' and params.get('arguments', {}) == {}:
                    value = self.status(account)
                elif params.get('name') == 'owi_work':
                    value = self.work(account, params.get('arguments'), mcp_key(session, identifier))
                else:
                    raise RequestError(400, 'Unknown tool or invalid arguments.')
                result = {'content':[{'type':'text','text':json.dumps(value)}],
                          'isError':value.get('verdict') == 'execution_error'}
            except RequestError as error:
                result = {'content':[{'type':'text','text':error.message}], 'isError':True}
        elif method != 'ping':
            return 200, {'jsonrpc':'2.0', 'id':identifier, 'error':{'code':-32601, 'message':'Unknown method.'}}
        return 200, {'jsonrpc':'2.0', 'id':identifier, 'result':result}

    def session(self, account, identifier, version=None):
        with self.database() as db:
            row = db.execute('SELECT protocol, expires FROM sessions WHERE id=? AND account=?',
                (identifier, account['id'])).fetchone()
        if not row or row[1] <= time.time():
            raise RequestError(404, 'MCP session expired. Reconnect your client.')
        if version and version != row[0]:
            raise RequestError(400, 'MCP protocol version does not match this session.')

    def telegram(self, body):
        message = body.get('message', {}) if isinstance(body, dict) else {}
        sender = message.get('from', {})
        chat = message.get('chat', {})
        account = self.telegram_accounts.get(sender.get('id'))
        if not account or chat.get('type') != 'private' or chat.get('id') != sender.get('id') or sender.get('is_bot'):
            return
        text = message.get('text', '')
        update = body.get('update_id')
        if type(update) is not int or not isinstance(text, str) or not text.strip():
            return
        request_id = 'telegram:'+str(update)
        if text.startswith('/start'):
            self.sender(chat['id'], 'OWI is connected. Send a writing, extraction or planning task here. Work uses your configured OpenRouter account; there is no download.')
            return
        arguments = {'task':text, 'privacy':'private_metadata'}
        try:
            cancel = self.begin(account, arguments, request_id)
        except RequestError as error:
            if error.status != 409:
                self.sender(chat['id'], error.message)
            return
        def run():
            state = 'failed'
            try:
                result = self.executor(account, arguments, cancel)
                output = result.get('output') or 'The worker returned no answer.'
                self.sender(chat['id'], output)
                state = 'delivered'
            except Exception:
                try:
                    self.sender(chat['id'], 'OWI could not finish this request. It will not retry the model automatically; check your connection status before trying again.')
                except Exception:
                    pass
            finally:
                self.finish(account, request_id, state)
        self.pool.submit(run)

    def send_telegram(self, chat_id, text):
        # Private chats only; no parse mode, raw error bodies, or token-bearing logs.
        chunks = [text[i:i+1800] for i in range(0, min(len(text), 14400), 1800)]
        if len(text) > 14400:
            chunks[-1] += '\n[Answer truncated for Telegram.]'
        for chunk in chunks:
            request = urllib.request.Request('https://api.telegram.org/bot'+self.telegram_token+'/sendMessage',
                data=json.dumps({'chat_id':chat_id, 'text':chunk}).encode(),
                headers={'Content-Type':'application/json'})
            try:
                from owi_openrouter import NoRedirect
                with urllib.request.build_opener(NoRedirect).open(request, timeout=10) as response:
                    if not json.loads(response.read(100000)).get('ok'):
                        raise ValueError()
            except Exception:
                raise RequestError(502, 'Telegram delivery failed.') from None

    def __call__(self, env, start_response):
        origin = env.get('HTTP_ORIGIN')
        headers = [('Cache-Control','no-store'), ('X-Content-Type-Options','nosniff'),
                   ('Referrer-Policy','no-referrer'), ('Content-Security-Policy',"default-src 'self'; style-src 'self' 'unsafe-inline'; script-src 'self'; frame-ancestors 'none'")]
        status, value, content_type = 200, None, 'application/json'
        try:
            if origin and origin not in self.origins:
                raise RequestError(403, 'This website is not allowed to use this connection.')
            if origin:
                headers += [('Access-Control-Allow-Origin', origin), ('Vary','Origin'),
                            ('Access-Control-Allow-Headers','Authorization, Content-Type, MCP-Protocol-Version, MCP-Session-Id, Idempotency-Key'),
                            ('Access-Control-Expose-Headers','MCP-Session-Id'),
                            ('Access-Control-Allow-Methods','GET, POST, OPTIONS')]
            path, method = env.get('PATH_INFO', '/'), env.get('REQUEST_METHOD', 'GET')
            if method == 'OPTIONS':
                status = 204
            elif path == '/healthz' and method == 'GET':
                value = {'ok':True, 'configured':bool(self.accounts)}
            elif path == '/' and method == 'GET':
                value = (platform.resources()/'tools/owi_hosted.html').read_text(encoding='utf-8')
                content_type = 'text/html; charset=utf-8'
            elif path == '/app.js' and method == 'GET':
                value = (platform.resources()/'tools/owi_hosted.js').read_text(encoding='utf-8')
                content_type = 'text/javascript; charset=utf-8'
            elif path in ('/mcp', '/api/status', '/api/work', '/telegram/webhook'):
                if path == '/mcp' and method in ('GET', 'DELETE'):
                    self.account(env.get('HTTP_AUTHORIZATION', ''))
                    raise RequestError(405, 'Use POST; this server returns JSON responses without an SSE stream.')
                if path == '/api/status' and method == 'GET':
                    value = self.status(self.account(env.get('HTTP_AUTHORIZATION', '')))
                elif method == 'POST' and path != '/api/status':
                    if env.get('CONTENT_TYPE', '').split(';')[0] != 'application/json' or env.get('HTTP_TRANSFER_ENCODING'):
                        raise RequestError(415, 'Send a JSON request with a Content-Length.')
                    try:
                        length = int(env.get('CONTENT_LENGTH', '0'))
                    except ValueError:
                        raise RequestError(400, 'Invalid request length.') from None
                    if not 0 < length <= 600000:
                        raise RequestError(413, 'Request exceeds the size limit.')
                    if path == '/telegram/webhook':
                        if not self.telegram_secret or not hmac.compare_digest(env.get('HTTP_X_TELEGRAM_BOT_API_SECRET_TOKEN', ''), self.telegram_secret):
                            raise RequestError(401, 'Invalid webhook connection.')
                        account = None
                    else:
                        account = self.account(env.get('HTTP_AUTHORIZATION', ''))
                    try:
                        body = json.loads(env['wsgi.input'].read(length))
                    except (ValueError, UnicodeDecodeError):
                        raise RequestError(400, 'Invalid JSON.') from None
                    if path == '/telegram/webhook':
                        self.telegram(body)
                        value = {'ok':True}
                    elif path == '/mcp':
                        if env.get('HTTP_MCP_PROTOCOL_VERSION', PROTOCOLS[-1]) not in PROTOCOLS:
                            raise RequestError(400, 'Unsupported MCP protocol version.')
                        if isinstance(body, dict) and body.get('method') == 'initialize' and 'id' in body:
                            session = secrets.token_hex(16)
                            status, value = self.mcp(account, body, session)
                            with self.database() as db:
                                db.execute('DELETE FROM sessions WHERE expires<=?', (time.time(),))
                                count = db.execute('SELECT COUNT(*) FROM sessions WHERE account=?', (account['id'],)).fetchone()[0]
                                if count >= 30:
                                    raise RequestError(429, 'Too many active connections. Reuse an existing connection or wait for it to expire.')
                                db.execute('INSERT INTO sessions VALUES (?,?,?,?)',
                                    (session, account['id'], value['result']['protocolVersion'], time.time()+3600))
                            headers.append(('MCP-Session-Id', session))
                        else:
                            session = env.get('HTTP_MCP_SESSION_ID', '')
                            if not session:
                                raise RequestError(400, 'Initialize an MCP session first.')
                            self.session(account, session, env.get('HTTP_MCP_PROTOCOL_VERSION'))
                            status, value = self.mcp(account, body, session)
                    else:
                        request_id = env.get('HTTP_IDEMPOTENCY_KEY', '')
                        if not request_id:
                            raise RequestError(400, 'An Idempotency-Key is required for a work request.')
                        value = self.work(account, body, 'api:'+request_id)
                else:
                    raise RequestError(405, 'Unsupported request method.')
            else:
                raise RequestError(404, 'Not found.')
        except RequestError as error:
            status, value = error.status, {'error':error.message}
        except Exception:
            status, value = 500, {'error':'The hosted connection could not complete this request.'}
        if status == 401:
            headers.append(('WWW-Authenticate', 'Bearer realm="OWI"'))
        payload = b'' if value is None else (value.encode('utf-8') if isinstance(value, str) else json.dumps(value).encode())
        headers += [('Content-Type', content_type), ('Content-Length',str(len(payload)))]
        from http import HTTPStatus
        start_response(str(status)+' '+HTTPStatus(status).phrase, headers)
        return [payload]


def create_app():
    return Service(json.loads(os.environ.get('OWI_ACCOUNTS_JSON', '[]')),
        os.environ.get('OWI_HOSTED_HOME', '/data/owi'),
        public_url=os.environ.get('OWI_PUBLIC_URL', os.environ.get('RENDER_EXTERNAL_URL', '')),
        telegram_token=os.environ.get('OWI_TELEGRAM_BOT_TOKEN', ''),
        telegram_secret=os.environ.get('OWI_TELEGRAM_WEBHOOK_SECRET', ''),
        telegram_name=os.environ.get('OWI_TELEGRAM_BOT_NAME', ''),
        origins=json.loads(os.environ.get('OWI_ALLOWED_ORIGINS_JSON', '[]')))
