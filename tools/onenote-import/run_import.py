import sys, os, re, hashlib
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from convert import *

SRC, DST = sys.argv[1], sys.argv[2]          # SRC = ...\MyNotebook, DST = ...\MyNotebook.note
base = os.path.join(DST, os.path.basename(os.path.normpath(SRC)))
resdir = os.path.join(DST, 'resources')
os.makedirs(resdir, exist_ok=True)
nres = [0]

def add(data, ext):
    n = hashlib.md5(data).hexdigest()
    if not ext.startswith('.'): ext = '.' + ext
    name = n + ext
    p = os.path.join(resdir, name)
    if not os.path.exists(p):
        open(p, 'wb').write(data); nres[0] += 1
    return name

def safe(s):
    s = re.sub(r'[\\/:*?"<>|\r\n\t\x0b]', '', s).strip().rstrip('.')
    return s or 'Untitled page'

total = 0
for root, dirs, files in os.walk(SRC):
    dirs[:] = [d for d in dirs if d != 'OneNote_RecycleBin']
    for f in sorted(files):
        if not f.lower().endswith('.one'): continue
        rel = os.path.relpath(root, SRC)
        sec = safe(f[:-4].replace('^J', ' ').replace('\n', ' ').replace('\x0a', ' '))
        outdir = base if rel == '.' else os.path.join(base, rel)
        outdir = os.path.join(outdir, sec)
        os.makedirs(outdir, exist_ok=True)
        depth = len(os.path.relpath(outdir, DST).split(os.sep))
        try:
            one = OneFile(open(os.path.join(root, f), 'rb').read())
            conv = Converter(one, add)
            pages = conv.section_pages()
        except Exception as e:
            print('FAIL', os.path.join(root, f), repr(e)); continue
        used = {}
        for p in pages:
            name = safe(p.title or 'Untitled page')
            k = name.lower()
            if k in used:
                used[k] += 1; name = '%s-%d' % (name, used[k] - 1)
            else:
                used[k] = 1
            md = page_markdown(p, depth)
            open(os.path.join(outdir, name + '.md'), 'wb').write(md.encode('utf-8'))
            total += 1
        print('%-40s pages=%d' % (os.path.relpath(os.path.join(root, f), SRC), len(pages)))
print('pages', total, 'new resources', nres[0])
