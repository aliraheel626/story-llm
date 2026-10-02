import json
import os
import sqlite3
import subprocess
import sys
import time
from pathlib import Path

root = Path(__file__).resolve().parent
db = Path(os.environ['APPDATA']) / 'com.story-llm.app' / 'story-llm.sqlite3'
pipe = r'\\.\pipe\tauri-pilot-com.story-llm.app'

def pilot(*args):
    result = subprocess.run(['tauri-pilot', '--socket', pipe, '--json', *args], capture_output=True, text=True, encoding='utf-8')
    if result.returncode:
        raise RuntimeError(result.stdout + result.stderr)
    return json.loads(result.stdout)

def query(sql, args=()):
    with sqlite3.connect(db.as_uri() + '?mode=ro', uri=True) as conn:
        conn.row_factory = sqlite3.Row
        return [dict(row) for row in conn.execute(sql, args)]

def save(label, data):
    (root / label).write_text(json.dumps(data, indent=2, ensure_ascii=False), encoding='utf-8')

def clicktext(text):
    elements = pilot('snapshot')['elements']
    matches = [e for e in elements if e.get('role') == 'button' and e.get('name') == text]
    assert len(matches) == 1, (text, matches)
    return pilot('click', '@' + matches[0]['ref'])

def clickcontains(text):
    matches=[e for e in pilot('snapshot')['elements'] if e.get('role')=='button' and text in e.get('name','')]
    assert matches
    return pilot('click','@'+matches[-1]['ref'])

def checkbox(label):
    return pilot('eval', "(()=>{const label=Array.from(document.querySelectorAll('label')).find(e=>e.textContent.trim().startsWith(" + json.dumps(label) + "));if(!label)throw Error('label missing');let e=label.querySelector('input[type=checkbox]');const checked=e.checked;const path=[];while(e&&e!==document.body){path.unshift(e.tagName.toLowerCase()+':nth-child('+(Array.from(e.parentNode.children).indexOf(e)+1)+')');e=e.parentElement;}return {selector:'body>'+path.join('>'),checked};})()")

def evidence():
    original = json.loads((root / 'baseline.json').read_text(encoding='utf-8'))
    ids = [row['id'] for row in query('SELECT id FROM stories') if row['id'] not in original['story_ids']]
    tracked = root / 'qa-story-ids.json'
    if tracked.exists():
        ids = list(dict.fromkeys(ids + json.loads(tracked.read_text())))
    marks = ','.join('?' for _ in ids) or 'NULL'
    return {
        'captured_at': time.time(),
        'snapshot': pilot('snapshot'),
        'dom': pilot('eval', "JSON.stringify({text:document.body.innerText,controls:Array.from(document.querySelectorAll('input,select,button')).map(e=>({tag:e.tagName,text:e.textContent,type:e.type,value:e.value,checked:e.checked,disabled:e.disabled,label:e.getAttribute('aria-label')})),images:Array.from(document.querySelectorAll('main img')).map(e=>({alt:e.alt,src:e.getAttribute('src'),complete:e.complete,naturalWidth:e.naturalWidth}))})"),
        'image_settings': query("SELECT value FROM settings WHERE key='image_model_default'"),
        'stories': query(f'SELECT * FROM stories WHERE id IN ({marks})', ids),
        'records': query(f'SELECT * FROM transcript_entries WHERE story_id IN ({marks}) ORDER BY story_id,seq', ids),
        'images': query(f'SELECT a.* FROM image_assets a JOIN transcript_entries e ON e.id=a.entry_id WHERE e.story_id IN ({marks})', ids),
        'usage': query(f'SELECT * FROM usage_records WHERE story_id IN ({marks}) ORDER BY created_at', ids),
        'logs': pilot('logs'),
    }

command = sys.argv[1]
if command == 'baseline':
    save('baseline.json', {'story_ids':[r['id'] for r in query('SELECT id FROM stories')], 'usage':query('SELECT kind,COUNT(*) AS calls FROM usage_records GROUP BY kind'), 'settings':query("SELECT value FROM settings WHERE key='image_model_default'")})
elif command == 'track':
    rows = query('SELECT story_id FROM transcript_entries WHERE content=? ORDER BY created_at DESC LIMIT 1', [sys.argv[2]])
    assert rows
    path=root/'qa-story-ids.json'
    ids=json.loads(path.read_text()) if path.exists() else []
    save('qa-story-ids.json',list(dict.fromkeys(ids+[rows[0]['story_id']])))
    print(rows[0]['story_id'])
elif command == 'capture':
    data = evidence()
    save(sys.argv[2], data)
    print(json.dumps({'file':sys.argv[2],'images':len(data['images']),'usage':[(r['kind'],r['cost_usd']) for r in data['usage']]}))
elif command == 'clicktext':
    print(json.dumps(clicktext(sys.argv[2])))
elif command == 'clickcontains':
    print(json.dumps(clickcontains(sys.argv[2])))
elif command == 'assertcaption':
    data=evidence()
    image=max(data['images'],key=lambda r:r['created_at'])
    captions=[r for r in data['records'] if r['kind']=='image_captioned' and json.loads(r['payload_json']).get('asset_id')==image['id']]
    generated=[r for r in data['records'] if r['kind']=='image_generated' and json.loads(r['payload_json']).get('asset_id')==image['id']]
    assert len(captions)==len(generated)==1
    assert captions[0]['visibility']=='hidden' and captions[0]['turn_id']==generated[0]['turn_id']
    caption=json.loads(captions[0]['payload_json'])['caption']
    assert caption and caption in json.loads(data['dom'])['text']
    usages=[r for r in data['usage'] if r['kind']=='caption' and r['turn_id']==captions[0]['turn_id']]
    assert len(usages)==1 and usages[0]['image_asset_id'] is None
    data['assertions']={'caption_matches_disclosure':True,'caption_usage_cost':usages[0]['cost_usd'],'same_turn':True}
    save(sys.argv[2],data)
    print(json.dumps(data['assertions']))
elif command == 'preview':
    data=evidence()
    captioned=[r for r in data['records'] if r['kind']=='image_captioned']
    latest=max(captioned,key=lambda r:r['created_at'])
    preview=pilot('ipc','preview_story_context','--args',json.dumps({'storyId':latest['story_id']}))
    data['read_only_full_preview']=preview
    text='\n'.join(m['text'] for m in preview['messages'])
    caption=json.loads(latest['payload_json'])['caption']
    if sys.argv[3]=='on':
        assert '[Authoritative story event: image_captioned]' in text and caption in text
    else:
        assert '[Authoritative story event: image_captioned]' not in text
    assert '[Authoritative story event: image_generated]' not in text
    save(sys.argv[2],data)
    print('caption preview '+sys.argv[3]+' verified, image prompts absent')
elif command == 'togglelabel':
    result = pilot('click', checkbox(sys.argv[2])['selector'])
    print(json.dumps(result))
elif command == 'defaults':
    assert checkbox('Image captions')['checked']
    assert not checkbox('Image prompts')['checked']
    assert not checkbox('The images themselves')['checked']
    save('c3-02-toggle.json', evidence())
    print('fresh defaults: captions on; prompts and images off')
elif command == 'tools-budget':
    clicktext('Narrator Tools')
    for label in ['Look up entities','Create entities','Update entities','Adjust attributes','Roll uncertain outcomes']:
        field = checkbox(label)
        if field['checked']:
            pilot('click', field['selector'])
    if len(sys.argv)>2 and sys.argv[2]=='no-images':
        field=checkbox('Illustrate scenes')
        if field['checked']: pilot('click',field['selector'])
    save('tools-budget-' + str(int(time.time())) + '.json', evidence())
    clicktext('Narrator Tools')
    print('unrelated narrator tools disabled through real clicks')
elif command == 'settings-check':
    pilot('select', '#caption-model', '__custom__')
    pilot('fill', '[aria-label="Custom caption model"]', 'mistralai/mistral-nemo')
    clicktext('Save')
    frames = []
    for _ in range(150):
        dom = pilot('eval', "document.body.innerText")
        frames.append(dom)
        if "mistralai/mistral-nemo can't read images" in dom:
            break
        time.sleep(0.4)
    save('c3-01-reject-poll.json', frames)
    assert "mistralai/mistral-nemo can't read images" in frames[-1]
    save('c3-01-rejected.json', evidence())
    assert 'mistralai/mistral-nemo' not in query("SELECT value FROM settings WHERE key='image_model_default'")[0]['value']
    pilot('select', '#caption-model', 'google/gemini-3.5-flash-lite')
    clicktext('Save')
    frames = []
    for _ in range(150):
        dom = pilot('eval', "document.body.innerText")
        frames.append(dom)
        if '\nSaved\n' in dom:
            break
        time.sleep(0.4)
    save('c3-01-save-poll.json', frames)
    assert '"caption_model":"google/gemini-3.5-flash-lite"' in query("SELECT value FROM settings WHERE key='image_model_default'")[0]['value']
    save('c3-01-restored.json', evidence())
    print('text-only model rejected; default saved')
elif command == 'poll':
    frames = []
    deadline = time.monotonic() + 180
    expression = sys.argv[3]
    if expression == 'ready':
        expression = "Array.from(document.querySelectorAll('main button')).some(b=>b.textContent.trim().startsWith('Continue')&&!b.disabled)"
    while time.monotonic() < deadline:
        value = pilot('eval', expression)
        frames.append({'time':time.time(),'value':value})
        if value is True or value == 'true':
            save(sys.argv[2], frames)
            print('condition met; frames=' + str(len(frames)))
            break
        time.sleep(0.4)
    else:
        save(sys.argv[2], frames)
        raise RuntimeError('poll timed out')
else:
    value = pilot(*sys.argv[1:])
    with (root / 'interactions.jsonl').open('a', encoding='utf-8') as f:
        f.write(json.dumps({'time':time.time(),'command':sys.argv[1:],'result':value},ensure_ascii=False) + '\n')
    print(json.dumps(value, ensure_ascii=False))
