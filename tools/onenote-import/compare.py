r"""Compares the output of run_import.py with the output of NoteCraft's built-in importer.

    python -I compare.py <python_out> <rust_out>

Both arguments are the notebook folders (...\py\Notebook and ...\rs\Notebook). Every .md file of
either tree is normalized and compared; differing files are listed with the first 20 lines of a
unified diff. The Rust side's resource links must point to existing files.

Normalization removes only the deliberate differences of the port:
  Python side: span tags (strip_spans), the Slovenian untitled title, the ../resources/ link path.
  Rust side:   <u>, <sup>, <sub> tags.
"""
import difflib, os, re, sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from cleanup import strip_spans

RES_PATH = re.compile(r'(\.\./)+resources/')
RS_TAGS = re.compile(r'</?(u|sup|sub)>')
RES_LINK = re.compile(r'\(resources/([^)\s]+)\)')


def md_files(root):
    out = {}
    for dp, dn, fn in os.walk(root):
        for f in fn:
            if f.lower().endswith('.md'):
                p = os.path.join(dp, f)
                out[os.path.relpath(p, root)] = p
    return out


def read(p):
    with open(p, 'rb') as fh:
        return fh.read().decode('utf-8')


def norm_py(text):
    text = strip_spans(text)
    text = text.replace('title: Neimenovana stran', 'title: Untitled page')
    return RES_PATH.sub('resources/', text)


def norm_rs(text):
    return RS_TAGS.sub('', text)


def main(argv):
    if len(argv) != 3:
        print(__doc__)
        return 2
    py, rs = md_files(argv[1]), md_files(argv[2])
    differing = 0
    for rel in sorted(set(py) | set(rs)):
        if rel not in rs:
            print('ONLY IN PYTHON: %s' % rel); differing += 1; continue
        if rel not in py:
            print('ONLY IN RUST: %s' % rel); differing += 1; continue
        a, b = norm_py(read(py[rel])), norm_rs(read(rs[rel]))
        if a != b:
            differing += 1
            print('DIFFERS: %s' % rel)
            diff = list(difflib.unified_diff(a.split('\n'), b.split('\n'), 'python', 'rust', lineterm='', n=1))
            for line in diff[:20]:
                print('  ' + line)
    broken = 0
    for rel, p in rs.items():
        folder = os.path.dirname(p)
        for name in RES_LINK.findall(read(p)):
            if not os.path.isfile(os.path.join(folder, 'resources', name)):
                broken += 1
                print('BROKEN LINK: %s -> resources/%s' % (rel, name))
    print('%d python pages, %d rust pages, %d differing files, %d broken resource links'
          % (len(py), len(rs), differing, broken))
    return 1 if differing or broken else 0


if __name__ == '__main__':
    sys.exit(main(sys.argv))
