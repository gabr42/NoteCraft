"""Removes the <span ...> / </span> tags that old exports left in page text.

The spans only carried colours and fonts, which NoteCraft does not use, and they break rendering.
Both the plain form (<span style='...'>) and the form with guillemets (<<span style='...'>> written
as the characters U+00AB / U+00BB) occur. Used by convert.py and as a stand-alone cleanup of
existing .md files:

    python -I cleanup.py <folder> [--apply] [--backup <folder>]

Without --apply it only reports. With --backup every changed file is first copied there, below the
same relative path.
"""
import os, re, sys, shutil

OPEN = r'\\?[<\u00ab]span\b[^>\u00bb\n]*[>\u00bb]'
CLOSE = r'\\?[<\u00ab]/span[>\u00bb]?\u00bb?'
SPAN_RE = re.compile('%s|%s' % (OPEN, CLOSE))
FENCE_RE = re.compile(r'^\s*(```|~~~)')


def strip_spans(text):
    """Text without span tags. A line that held nothing but tags is dropped; fenced code is left alone."""
    out = []; fenced = False
    for line in text.split('\n'):
        if FENCE_RE.match(line):
            fenced = not fenced
        if fenced or not SPAN_RE.search(line):
            out.append(line); continue
        new = SPAN_RE.sub('', line)
        if new.strip():
            out.append(new)
        # a line of only tags disappears
    return '\n'.join(out)


def main(argv):
    apply = '--apply' in argv
    backup = argv[argv.index('--backup') + 1] if '--backup' in argv else None
    skip = {argv[argv.index('--backup') + 1]} if backup else set()
    args = [a for a in argv[1:] if not a.startswith('--') and a not in skip]
    root = args[0]
    changed = 0; tags = 0
    for dp, dn, fn in os.walk(root):
        for f in fn:
            if not f.lower().endswith('.md'): continue
            p = os.path.join(dp, f)
            with open(p, 'rb') as fh: raw = fh.read()
            text = raw.decode('utf-8', 'replace')
            if '<span' not in text and '\u00abspan' not in text and 'span' not in text: continue
            new = strip_spans(text)
            if new == text: continue
            n = len(SPAN_RE.findall(text))
            changed += 1; tags += n
            print('%s: %d tags' % (os.path.relpath(p, root), n))
            if apply:
                if backup:
                    dst = os.path.join(backup, os.path.relpath(p, root))
                    os.makedirs(os.path.dirname(dst), exist_ok=True)
                    shutil.copy2(p, dst)
                os.chmod(p, 0o666)
                with open(p, 'wb') as fh: fh.write(new.encode('utf-8'))
    print('%s %d files, %d tags' % ('Changed' if apply else 'Would change', changed, tags))


if __name__ == '__main__':
    main(sys.argv)
