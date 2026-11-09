#!/usr/bin/env python3
"""Latency of the real GUI-owned WS bridge, restricted to recorded QA daemons."""
from pathlib import Path
import base64,json,subprocess,time,re,sys,urllib.request
import websocket
folder=Path(__file__).resolve().parents[1]/'.mpac/streaming-preview'
mode=sys.argv[1]
assert mode in ('websocket','compatibility')
owned=json.loads((folder/('owned.json' if mode=='websocket' else 'legacy-owned.json')).read_text())
assert Path(owned['data']).resolve() in ((folder/'state').resolve(),(folder/'legacy-state').resolve())
info=json.loads((folder/('gui-'+mode+'.json')).read_text())
sid=json.loads((folder/'stream-smoke.json').read_text())['ui_session'] if mode=='websocket' else owned['session_id']
assert info['session_id']==sid
pids=subprocess.check_output(['pgrep','-x','mpac-desktop'],text=True).split()
assert len(pids)==1
out=subprocess.check_output(['/usr/sbin/lsof','-nP','-a','-p',pids[0],'-iTCP','-sTCP:LISTEN','-F0n'])
ports=re.findall(rb'n127\.0\.0\.1:(\d+)',out);assert len(ports)==1
url='ws://127.0.0.1:'+ports[0].decode()+'/terminal'
s=websocket.create_connection(url,origin='mpac://localhost',timeout=5)
s.send(json.dumps({'type':'auth','token':info['token'],'session_id':sid,'after':0}))
ready=json.loads(s.recv());assert ready['type']=='ready' and ready['transport']==mode
text=bytearray();cursor=0

def read_until(predicate):
 global cursor
 deadline=time.monotonic()+5
 while not predicate():
  assert time.monotonic()<deadline
  f=json.loads(s.recv())
  if f['type']=='output':
   if f['reset']:text.clear()
   text.extend(base64.b64decode(f['data_base64']));cursor=f['seq'];s.send(json.dumps({'type':'ack','seq':cursor}))
  elif f['type']=='ack':pass
  else:raise AssertionError('Unexpected native terminal response')
read_until(lambda:b'STREAM QA' in text)
samples=[]
for i in range(20):
 marker=('~NATIVE-'+mode+'-'+str(i)+'~').encode();start=time.monotonic()
 s.send(json.dumps({'type':'input','id':i+1,'data':marker.decode()}));read_until(lambda:marker in text);samples.append((time.monotonic()-start)*1000)
s.close()
values=sorted(samples);report={'scope':'Real GUI WS bridge to owned fake-Claude raw PTY; no model calls','transport':mode,'daemon_pid':owned['pid'],'samples':len(values),'median_ms':round(values[len(values)//2],2),'p95_ms':round(values[int(len(values)*.95)],2),'max_ms':round(values[-1],2)}
(folder/('native-'+mode+'-smoke.json')).write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report))
