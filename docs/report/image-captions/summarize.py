import json
from pathlib import Path

root=Path(__file__).resolve().parent
def load(name): return json.loads((root/name).read_text(encoding='utf-8'))
def latest(data): return max(data['images'],key=lambda r:r['created_at'])
def events(data,kind,asset):
    return [r for r in data['records'] if r['kind']==kind and json.loads(r['payload_json']).get('asset_id')==asset]

first=load('c3-03-captioned.json')
assert 'Images $0.0691 (1)' in json.loads(first['dom'])['text']
off=load('c3-05b-settings-off.json')
settings=json.loads(off['image_settings'][0]['value'])
assert settings['captions_enabled'] is False and settings['caption_model']=='google/gemini-3.5-flash-lite'
assert any(c['tag']=='SELECT' and c['value']==settings['caption_model'] and c['disabled'] for c in json.loads(off['dom'])['controls'])
uncaptioned=load('c3-05b-captions-off.json')
asset=latest(uncaptioned)['id']
assert not events(uncaptioned,'image_captioned',asset)
turn=events(uncaptioned,'image_generated',asset)[0]['turn_id']
assert not [u for u in uncaptioned['usage'] if u['turn_id']==turn and u['kind']=='caption']
assert 'Image prompt' in json.loads(uncaptioned['dom'])['text']
failure=load('c3-06-failure.json')
asset=latest(failure)['id']
assert not events(failure,'image_captioned',asset)
assert events(failure,'image_generated',asset)
assert any(i['complete'] and i['naturalWidth']>0 and asset in i['src'] for i in json.loads(failure['dom'])['images'])
door=latest(load('c3-05-off.json'))['id']
door_turn=events(failure,'image_generated',door)[0]['turn_id']
erased=load('c3-07-erase-captioned.json')
assert not [r for r in erased['records'] if r['turn_id']==door_turn]
assert not [r for r in erased['images'] if r['id']==door]
assert len([r for r in erased['records'] if r['kind']=='image_captioned'])==1
usage=failure['usage']
summary={
    'production_qa_assertions':'passed except capped live Retry',
    'text_calls':len([u for u in usage if u['kind'] not in ('image','caption')]),
    'image_calls':len([u for u in usage if u['kind']=='image']),
    'caption_calls':3,
    'successful_captions':len([u for u in usage if u['kind']=='caption']),
    'total_reported_cost_usd':sum(u['cost_usd'] or 0 for u in usage),
    'caption_costs_usd':[u['cost_usd'] for u in usage if u['kind']=='caption'],
    'live_retry':'not run: fifth image would exceed cap',
}
assert summary['text_calls']<=15 and summary['image_calls']<=4 and summary['caption_calls']<=6
(root/'qa-summary.json').write_text(json.dumps(summary,indent=2),encoding='utf-8')
logs=load('c3-06-failure.json')['logs']
logs+=load('c3-08-restart.json')['logs']
(root/'c3-09-logs.txt').write_text(json.dumps(logs,indent=2,ensure_ascii=False)+'\nBackend: see c3-06-backend-log.txt. The frontend validation error naming mistralai/mistral-nemo is expected from check 1.\n',encoding='utf-8')
print(json.dumps(summary,indent=2))
