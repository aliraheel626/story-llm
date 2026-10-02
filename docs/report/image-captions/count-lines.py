import json
import re
import subprocess
import sys
from pathlib import Path

args = sys.argv[1:]
diff_args = args or []
numstat = subprocess.check_output(['git', 'diff', '--numstat', *diff_args], text=True)
patch = subprocess.check_output(['git', 'diff', '--unified=0', *diff_args], text=True)
def test_lines(text, path):
    lines = text.splitlines()
    if path.endswith('prompts.rs'):
        start = next((i + 1 for i, line in enumerate(lines) if line == '#[cfg(test)]' and i + 2 < len(lines) and 'caption_names_are_optional' in lines[i+2]), len(lines)+1)
        end = next((i + 1 for i, line in enumerate(lines) if i + 1 > start and line == '}'), start-1)
        return set(range(start, end+1))
    start = next((i+1 for i, line in enumerate(lines) if line == '#[cfg(test)]'), len(lines)+1)
    return set(range(start, len(lines)+1)) if path.endswith('.rs') else set()

totals = {'production_added':0,'production_deleted':0,'test_added':0,'test_deleted':0}
path = None
for line in patch.splitlines():
    if line.startswith('diff --git '):
        path = line.split(' b/',1)[1]
        base = args[0] if args else 'HEAD'
        old = subprocess.check_output(['git','show',f'{base}:{path}'],text=True)
        new = subprocess.check_output(['git','show',f'{args[1]}:{path}'],text=True) if len(args)>1 else Path(path).read_text(encoding='utf-8')
        oldtests,newtests=test_lines(old,path),test_lines(new,path)
    elif line.startswith('@@'):
        m=re.match(r'@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@',line)
        oldn,newn=map(int,m.groups())
    elif line.startswith('+') and not line.startswith('+++'):
        totals['test_added' if newn in newtests else 'production_added']+=1
        newn+=1
    elif line.startswith('-') and not line.startswith('---'):
        totals['test_deleted' if oldn in oldtests else 'production_deleted']+=1
        oldn+=1
    elif line.startswith(' '):
        oldn+=1;newn+=1
print(numstat)
print(json.dumps(totals,indent=2))
if len(args)==2:
    (Path(__file__).resolve().parent / f'numstat-{args[1]}.txt').write_text(numstat+'\n'+json.dumps(totals,indent=2),encoding='utf-8')
