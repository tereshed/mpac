#!/usr/bin/env python3
"""Own daemon/PTY fake CLIs only; no model calls or real environment files."""
from pathlib import Path
import argparse,base64,json,os,signal,subprocess,sys,tempfile,time,urllib.request,urllib.error,urllib.parse,shutil

FAKE=r'''
import json,os,sys
from pathlib import Path
provider=Path(sys.argv[0]).name
if '--version' in sys.argv:
 print('2.1.318 (Claude Code)' if provider=='claude' else 'codex-cli 0.166.0' if provider=='codex' else 'hermes 0.3.0');raise SystemExit
mode=os.environ.get('CHECK_MODE','baseline')
checks={}
if mode=='overlay':
 checks['custom_shared']=os.environ.get('MODEL_LABEL')=='GLM Español 文'
 checks['literal_shell']=os.environ.get('LITERAL','').startswith('$(touch ')
 checks['path_overlay']=os.environ.get('PATH')=='/not-used-for-resolution'
 if provider=='claude':checks['explicit_auth']=os.environ.get('ANTHROPIC_AUTH_TOKEN')=='ENV_FILE_SECRET_901' and os.environ.get('ANTHROPIC_BASE_URL')=='https://glm.example.invalid'
 if provider=='codex':checks['explicit_auth']=os.environ.get('OPENAI_API_KEY')=='ENV_FILE_SECRET_902' and os.environ.get('OPENAI_BASE_URL')=='https://openai.example.invalid'
 if provider=='hermes':checks['explicit_auth']=os.environ.get('OPENROUTER_API_KEY')=='ENV_FILE_SECRET_903'
else:
 if provider=='claude':checks['no_ambient_api']=all(k not in os.environ for k in ['ANTHROPIC_AUTH_TOKEN','ANTHROPIC_API_KEY','ANTHROPIC_BASE_URL'])
 if provider=='codex':checks['no_ambient_api']=all(k not in os.environ for k in ['OPENAI_API_KEY','CODEX_API_KEY','OPENAI_BASE_URL'])
checks['internal_events']=not os.environ.get('MPAC_EVENT_CHANNEL') or os.environ.get('MPAC_EVENT_PROVIDER')==provider
result={'provider':provider,'mode':mode,'checks':checks,'skip_permissions':'--dangerously-skip-permissions' in sys.argv}
Path('env-observation-'+provider+'-'+str(os.getpid())+'.json').write_text(json.dumps(result))
print('ENV snapshot checked; no values printed')
raise SystemExit(0 if all(checks.values()) else 2)
'''

def main():
 parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--binary',required=True,type=Path);parser.add_argument('--report',type=Path);args=parser.parse_args();binary=args.binary.resolve(strict=True)
 result={'passed':False,'scope':'isolated Rust daemon + real PTY + fake Claude/Codex/Hermes; no models','checks':[]}
 def check(label,condition=True):
  if not condition:raise AssertionError(label)
  result['checks'].append(label)
 with tempfile.TemporaryDirectory(prefix='mpac-env-smoke-') as temporary:
  workspace=Path(temporary).resolve();workspace.chmod(0o700);bins=workspace/'bin';bins.mkdir();root=workspace/'project';root.mkdir();data=workspace/'state';logfile=workspace/'daemon.log'
  for provider in ['claude','codex','hermes']:
   p=bins/provider;p.write_text('#!'+sys.executable+'\n'+FAKE);p.chmod(0o700)
  own_bin=workspace/'daemon-bin';own_bin.mkdir();own_binary=own_bin/'mpac-daemon';shutil.copy2(binary,own_binary)
  ambient_fixture = 'AMBIENT_SECRET_' + '900'
  env={**os.environ,'PATH':'bin'+os.pathsep+os.environ.get('PATH',''),'ANTHROPIC_AUTH_TOKEN':ambient_fixture,'ANTHROPIC_API_KEY':ambient_fixture,'ANTHROPIC_BASE_URL':'https://ambient.invalid','OPENAI_API_KEY':ambient_fixture,'CODEX_API_KEY':ambient_fixture,'OPENAI_BASE_URL':'https://ambient.invalid'}
  with logfile.open('wb') as log:
   proc=subprocess.Popen([str(own_binary),'--port','0','--data-dir',str(data),'--ui-dir',str(workspace)],stdin=subprocess.DEVNULL,stdout=log,stderr=log,env=env,start_new_session=True,cwd=workspace)
   try:
    deadline=time.monotonic()+8
    while not (data/'launch.json').exists():
     if proc.poll() is not None or time.monotonic()>deadline:raise AssertionError('own daemon did not start')
     time.sleep(.025)
    launch=json.loads((data/'launch.json').read_text());check('owned daemon PID recorded',launch['pid']==proc.pid);token=(data/'auth-token').read_text();base='http://127.0.0.1:'+str(launch['port'])
    http=urllib.request.build_opener(urllib.request.ProxyHandler({}))
    def api(route,body=None,expected=200):
     request=urllib.request.Request(base+'/api'+route,data=None if body is None else json.dumps(body).encode(),headers={'Authorization':'Bearer '+token,'Content-Type':'application/json'})
     try:
      response=http.open(request,timeout=8);code=response.status;content=response.read()
     except urllib.error.HTTPError as error:code=error.code;content=error.read()
     if code not in (expected if isinstance(expected,tuple) else (expected,)):raise AssertionError('unexpected own API status '+str(code))
     return json.loads(content)
    check('ENV capability advertised',api('/state')['capabilities']['session_env_files'] is True)
    project=api('/projects',{'name':'Owned ENV smoke','paths':[str(root)]});root_id=project['roots'][0]['id'];marker=workspace/'must-not-be-created'
    file=root/'any variables.profile';file.write_text('export CHECK_MODE=overlay\nANTHROPIC_AUTH_TOKEN=ENV_FILE_SECRET_901\nANTHROPIC_BASE_URL=https://glm.example.invalid\nOPENAI_API_KEY=ENV_FILE_SECRET_902\nOPENAI_BASE_URL=https://openai.example.invalid\nOPENROUTER_API_KEY=ENV_FILE_SECRET_903\nMODEL_LABEL="GLM Español 文"\nPATH=/not-used-for-resolution\nLITERAL=\'$(touch '+str(marker)+')\'\n')
    listing=api('/environment-files?path='+urllib.parse.quote(str(root)));check('picker finds arbitrary filename and returns no contents',any(x['name']==file.name for x in listing['entries']) and 'ENV_FILE_SECRET' not in json.dumps(listing))
    outside=workspace/'outside settings 文.env';outside.write_text(file.read_text())
    def start(provider,selected=False,full=False,env_path=None):
     observations=set(root.glob('env-observation-*.json'));body={'project_id':project['id'],'root_id':root_id,'agent':provider,'title':'Owned ENV '+provider}
     selected_file=file if env_path is None else env_path
     if selected:body['env_file']=selected_file.name if selected_file.parent==root else str(selected_file)
     if full:body['access']={'mode':'full_access','additional_root_ids':[],'acknowledge_full_access':True}
     session=api('/sessions',body);deadline=time.monotonic()+8
     while True:
      new=set(root.glob('env-observation-*.json'))-observations
      if new:break
      if time.monotonic()>deadline:raise AssertionError('owned fake CLI did not report')
      time.sleep(.025)
     observation=json.loads(next(iter(new)).read_text());check(provider+(' selected ENV' if selected else ' default profile')+(' full access' if full else '')+' checks='+','.join(k for k,v in observation['checks'].items() if not v),all(observation['checks'].values()))
     check(provider+' access flag remains explicit',observation['skip_permissions']==full)
     if selected:check('metadata contains only selected path and count',session['environment']=={'file':str(selected_file),'variable_count':9} and 'ENV_FILE_SECRET' not in json.dumps(session))
     else:check('default profile has no custom ENV metadata',not session.get('environment'))
     return session
    start('claude');start('codex')
    for provider in ['claude','codex','hermes']:start(provider,True)
    start('claude',True,True)
    start('hermes',True,env_path=outside)
    start('claude')
    check('shell substitutions were not executed',not marker.exists())
    before=len(api('/state')['sessions']);bad=root/'bad.env';bad.write_text('TOKEN="ENV_FILE_SECRET_BAD\n');error=api('/sessions',{'project_id':project['id'],'root_id':root_id,'agent':'claude','env_file':bad.name},400);check('invalid ENV is rejected without secret or new session','ENV_FILE_SECRET' not in json.dumps(error) and len(api('/state')['sessions'])==before)
    broken=root/'broken.env';broken.write_text('PATH=/ENV_FILE_SECRET_FAIL_999\n');(bins/'hermes').write_text('#!/nonexistent/mpac-fixture-interpreter\n');(bins/'hermes').chmod(0o700)
    error=api('/sessions',{'project_id':project['id'],'root_id':root_id,'agent':'hermes','env_file':broken.name},(200,400));check('failed exec response excludes custom PATH value','ENV_FILE_SECRET' not in json.dumps(error))
    if error.get('id'):
     failed_id=error['id'];deadline=time.monotonic()+8
     while True:
      current=next(s for s in api('/state')['sessions'] if s['id']==failed_id)
      if current['status'] not in ('running','needs_attention','starting'):break
      if time.monotonic()>deadline:raise AssertionError('owned failed exec did not complete')
      time.sleep(.025)
     output=api('/sessions/'+failed_id+'/output?after=0');check('async exec failure excludes PATH from error/output','ENV_FILE_SECRET' not in json.dumps(current) and b'ENV_FILE_SECRET' not in base64.b64decode(output['data_base64']))
    shell=root/'shell.env';shell.write_text('SHELL=/ENV_FILE_SECRET_SHELL_999\n');error=api('/sessions',{'project_id':project['id'],'root_id':root_id,'agent':'claude','env_file':shell.name},400);check('invalid custom SHELL is rejected without value/log warning','ENV_FILE_SECRET' not in json.dumps(error))
    check('API state excludes environment values','ENV_FILE_SECRET' not in json.dumps(api('/state')))
    # Force SQLite pages to disk before inspecting only app-owned database files.
    for file_db in data.glob('state.sqlite3*'):check('SQLite excludes ENV values',b'ENV_FILE_SECRET' not in file_db.read_bytes() and b'AMBIENT_SECRET' not in file_db.read_bytes())
    check('daemon log excludes supplied environment values',b'ENV_FILE_SECRET' not in logfile.read_bytes() and b'AMBIENT_SECRET' not in logfile.read_bytes())
    result['passed']=True
   finally:
    if proc.poll() is None:proc.send_signal(signal.SIGTERM)
    try:proc.wait(timeout=12)
    except subprocess.TimeoutExpired:proc.kill();proc.wait();raise AssertionError('own daemon did not stop gracefully')
 if args.report:args.report.write_text(json.dumps(result,indent=2)+'\n')
 print(json.dumps(result))
if __name__=='__main__':main()
