#!/usr/bin/env python3
"""Renderer spike benchmark (docs/TESTING.md "1000 pieces in the browser").

    cd web && npm run spike          # serves http://127.0.0.1:8099/dev/spike.html
    python3 web/dev/spike-bench.py OUT_DIR [/usr/bin/google-chrome]

Needs Python Playwright. Hardware GL through ANGLE/GL; Google Chrome (not
Chromium) for the H.264 loops. Writes OUT_DIR/results.json and a screenshot
per case, prints one line per case.
"""
import json, sys, time, os
from playwright.sync_api import sync_playwright
OUT = sys.argv[1]; EXE = sys.argv[2] if len(sys.argv) > 2 else None
BASE = os.environ.get('SPIKE_URL', 'http://127.0.0.1:8099/dev/spike.html')
os.makedirs(OUT, exist_ok=True)
ARGS = ['--use-gl=angle', '--use-angle=gl', '--ignore-gpu-blocklist', '--enable-gpu', '--autoplay-policy=no-user-gesture-required']
def stats(page):
    return page.evaluate("""() => { const s = window.__spike; const o = {}; for (const k in s) if (typeof s[k] !== 'function') o[k] = s[k]; return o; }""")
cases = [(400,'jigsaw',''),(1000,'jigsaw',''),(1000,'hexagon',''),(1000,'triangle',''),(1000,'jigsaw','&turned=1'),(1000,'hexagon','&turned=1')]
res = {}
with sync_playwright() as p:
    kw = dict(headless=True, args=ARGS)
    if EXE: kw['executable_path'] = EXE
    b = p.chromium.launch(**kw)
    for pieces, cut, extra in cases:
        ctx = b.new_context(viewport={'width':1280,'height':800}, device_scale_factor=2)
        page = ctx.new_page(); logs=[]
        page.on('console', lambda m: logs.append(f'{m.type}: {m.text}') if m.type in ('error','warning') else None)
        page.on('pageerror', lambda e: logs.append(f'pageerror: {e}'))
        page.goto(f'{BASE}?pieces={pieces}&cut={cut}{extra}')
        page.wait_for_function("window.__spike && window.__spike.frames > 30", timeout=60000)
        time.sleep(4)
        s = stats(page)
        # drag piece 0 and measure the frames during the drag
        sx, sy = page.evaluate('window.__spike.pieceScreen(0)')
        page.mouse.move(sx, sy); page.mouse.down(); page.mouse.move(sx-200, sy+50, steps=30); page.mouse.up()
        time.sleep(2.2)
        sd = stats(page)
        page.evaluate('window.__spike.setBench(false)'); time.sleep(2.5)
        od = stats(page)
        k = f'{pieces}-{cut}{"-turned" if extra else ""}'
        page.screenshot(path=f'{OUT}/{k}.png')
        res[k] = {'s': s, 'after_drag': sd, 'on_demand_rps': od['rendersPerSec'], 'video_fps': od['videoFps'], 'console': logs}
        ctx.close()
    b.close()
json.dump(res, open(f'{OUT}/results.json','w'), indent=1)
for k, r in res.items():
    s = r['s']; d = r['after_drag']
    print(k, f"avg {s['avgMs']:.2f} p95 {s['p95Ms']:.2f} >20 {s['pctOver20']:.0f}% js {s['jsMs']:.2f} vfps {s['videoFps']} stride {s['stride']} verts {s['vertices']} build {s['meshBuildMs']:.1f} | after drag avg {d['avgMs']:.2f} p95 {d['p95Ms']:.2f} js {d['jsMs']:.2f} | idle rps {r['on_demand_rps']} vfps {r['video_fps']} | dpr {s['dpr']} err {s['errors']} console {r['console']} gl {s['glRenderer']}")
