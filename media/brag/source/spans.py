import json, sys
rows = json.load(open(sys.argv[1]))
for y, row in enumerate(rows):
    spans, cur, buf = [], None, ""
    for ch, fg, bg, bold, rev in row:
        key = (fg, bg, bold, rev)
        if key != cur:
            if buf.strip():
                spans.append((cur, buf))
            cur, buf = key, ""
        buf += ch
    if buf.strip():
        spans.append((cur, buf))
    styled = [(k, t.strip()) for k, t in spans if k != ("default", "default", False, False)]
    if styled:
        print(y, styled)
