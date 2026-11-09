#!/usr/bin/env python3
"""Real HTTPS/PTY checks on owned fixtures. Never reads the main workspace."""
import argparse
import concurrent.futures
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import ssl
import subprocess
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid

root = Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser()
parser.add_argument('--keep', action='store_true')
parser.add_argument('--bind', default='127.0.0.1')
options = parser.parse_args()
qa = root / '.mpac/mobile-qa'
qa.mkdir(mode=0o700, exist_ok=True)
daemon_dir = qa / 'daemon'
host_dir = qa / 'host'
binary = qa / 'mpac-daemon'
assert not (qa / 'owned.json').exists(), 'A previous owned fixture exists; inspect and stop it before replacing it'
shutil.copyfile(root / 'target/release/mpac-daemon', binary)
binary.chmod(0o700)
checks = []
processes = []

def wait_json(path, process):
    for _ in range(200):
        if process.poll() is not None:
            raise RuntimeError('Owned process exited; inspect its QA log')
        try:
            result = json.loads(path.read_text())
            if result.get('pid') == process.pid: return result
        except (OSError, ValueError): pass
        time.sleep(.05)
    raise RuntimeError('Owned process readiness timed out')

def call(url, token='', body=None, context=None, headers=None):
    request = urllib.request.Request(url, data=None if body is None else json.dumps(body).encode(), headers={
        'Authorization': 'Bearer ' + token, 'Content-Type': 'application/json', **(headers or {})})
    handlers = [urllib.request.ProxyHandler({})]
    if context: handlers.append(urllib.request.HTTPSHandler(context=context))
    try:
        with urllib.request.build_opener(*handlers).open(request, timeout=10) as response:
            return response.status, json.load(response)
    except urllib.error.HTTPError as error:
        return error.code, json.load(error)

try:
    with (qa / 'daemon.log').open('wb') as log:
        daemon = subprocess.Popen([str(binary), '--data-dir', str(daemon_dir), '--ui-dir', str(root/'ui/dist'), '--port', '0'], stdout=log, stderr=log, start_new_session=True)
    processes.append(daemon)
    launch = wait_json(daemon_dir / 'launch.json', daemon)
    local_token = urllib.parse.parse_qs(urllib.parse.urlsplit(launch['url']).fragment)['token'][0]
    with (qa / 'host.log').open('wb') as log:
        host = subprocess.Popen([str(root/'target/debug/mpac-mobile-host'), '--data-dir', str(daemon_dir), '--state-dir', str(host_dir), '--bind', options.bind+':0'], stdout=log, stderr=log, start_new_session=True)
    processes.append(host)
    remote = wait_json(host_dir / 'launch.json', host)
    admin = urllib.parse.urlsplit(remote['admin_url'])
    admin_token = urllib.parse.parse_qs(admin.fragment)['token'][0]
    admin_url = urllib.parse.urlunsplit((admin.scheme, admin.netloc, '', '', ''))
    time.sleep(.1)
    status, offer = call(admin_url+'/admin/invite', admin_token, {})
    assert status == 200
    fragment = urllib.parse.parse_qs(urllib.parse.urlsplit(offer['link']).fragment)
    import base64
    certificate = base64.urlsafe_b64decode(fragment['ca'][0]+'===')
    context = ssl.create_default_context(cadata=ssl.DER_cert_to_PEM_cert(certificate))
    endpoint = remote['endpoint'].rstrip('/')
    key = uuid.uuid4().hex + uuid.uuid4().hex
    check = lambda label: checks.append(label)
    assert call(endpoint+'/api/state', key, context=context)[0] == 401
    check('unpaired device cannot read workspace')
    assert call(endpoint+'/pair/request', key, {'invite': fragment['invite'][0], 'name':'QA phone'}, context, {'Origin':'https://elsewhere.test'})[0] == 403
    check('browser Origin rejected on native endpoint')
    with concurrent.futures.ThreadPoolExecutor(2) as pool:
        replies = list(pool.map(lambda _: call(endpoint+'/pair/request', key, {'invite': fragment['invite'][0], 'name':'QA phone'}, context), range(2)))
    assert sorted(r[0] for r in replies) == [200,403]
    pending = next(r[1] for r in replies if r[0] == 200)
    check('concurrent invitation consumers have exactly one winner')
    assert call(endpoint+'/api/state', key, context=context)[0] == 401
    check('pending device remains unauthorized')
    assert call(admin_url+'/admin/approve', admin_token, {'id':pending['id'],'code':'wrong'})[0] == 409
    assert call(admin_url+'/admin/approve', admin_token, {'id':pending['id'],'code':pending['code']})[0] == 200
    check('approval requires exact request and comparison code')
    status, state = call(endpoint+'/api/state', key, context=context)
    assert status == 200 and state['capabilities']['mobile_remote'] and not state['capabilities']['terminal_stream'] and not state['capabilities']['terminal_resize']
    check('approved phone reads actual daemon state through TLS')
    assert local_token not in json.dumps(state)
    check('local daemon credential never returned to phone')
    project = state['projects'][0]
    status, session = call(endpoint+'/api/sessions', key, {'project_id':project['id'], 'root_id':project['roots'][0]['id'], 'agent':'demo', 'title':'Mobile HTTPS QA'}, context)
    assert status == 200, session
    session_id = session['id']
    status, _ = call(endpoint+'/api/sessions/'+session_id+'/resize', key, {'cols':30,'rows':20}, context)
    assert status == 403
    check('phone cannot resize a terminal shared with desktop')
    assert call(endpoint+'/api/sessions/'+session_id+'/input', key, {'data':'status\r'}, context)[0] == 200
    for _ in range(100):
        _, output = call(endpoint+'/api/sessions/'+session_id+'/output?after=0', key, context=context)
        decoded = base64.b64decode(output.get('data_base64','')).decode(errors='replace')
        if 'status' in decoded: break
        time.sleep(.1)
    assert 'status' in decoded
    check('paired device starts no-model demo and sends terminal input')
    assert key not in (host_dir/'devices.json').read_text() and fragment['invite'][0] not in (host_dir/'devices.json').read_text()
    check('host persists device hashes without bearer or invitation')
    try: call(endpoint+'/api/state', key, context=ssl.create_default_context())
    except urllib.error.URLError: check('unknown certificate rejected before workspace request')
    else: raise AssertionError('System roots unexpectedly trusted private fixture certificate')
    assert call(admin_url+'/admin/revoke', admin_token, {'id':pending['id']})[0] == 200
    assert call(endpoint+'/api/state', key, context=context)[0] == 401
    assert call(endpoint+'/api/sessions/'+session_id+'/input', key, {'data':'must-not-arrive\r'}, context)[0] == 401
    check('revocation blocks both new reads and terminal writes')
    assert call(endpoint+'/pair/status', key, {'id':pending['id']}, context)[1]['status'] == 'expired'
    check('revoked pairing cannot become approved again')
    assert call(admin_url+'/admin/state', 'wrong')[0] == 401
    check('local administration requires independent private credential')
    # Leave a clean fixture with the demo stopped, ready for native UI pairing.
    request = urllib.request.Request('http://127.0.0.1:'+str(launch['port'])+'/api/sessions/'+session_id+'/stop',data=b'{}',headers={'Authorization':'Bearer '+local_token,'Content-Type':'application/json'})
    with urllib.request.urlopen(request,timeout=5) as result: assert result.status == 200
    owned={'daemon_pid':daemon.pid,'host_pid':host.pid,'daemon_data':str(daemon_dir),'host_data':str(host_dir),'binary':str(binary),'endpoint':endpoint}
    (qa/'owned.json').write_text(json.dumps(owned,indent=2)+'\n');(qa/'owned.json').chmod(0o600)
    report={'passed':True,'checks':checks,'scope':'Owned real Rust daemon and HTTPS companion; no main workspace, provider login, or model requests','bind':options.bind}
    report_dir = root / '.mpac'
    report_dir.mkdir(exist_ok=True)
    (report_dir / 'mobile-https-smoke.json').write_text(json.dumps(report,indent=2)+'\n')
    print(json.dumps({'passed':True,'checks':len(checks),'owned_daemon':daemon.pid,'owned_host':host.pid,'endpoint':endpoint}))
finally:
    if not options.keep or len(checks)<14:
        for process in reversed(processes):
            if process.poll() is None:
                process.send_signal(signal.SIGINT)
                try: process.wait(timeout=8)
                except subprocess.TimeoutExpired: process.terminate();process.wait(timeout=5)
