#!/usr/bin/env python3
"""Build an offline browser example; no Rust, provider calls, or credentials."""
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

def build():
    skills = ['skill:text-editing', 'skill:structured-extraction', 'skill:planning-decomposition']
    prices = json.loads((ROOT / 'examples/litellm-prices-sample.json').read_text())
    workers = []
    for model in ['haiku-4-5', 'opus-4-5']:
        rate = prices['claude-' + model]
        workers.append(dict(id='worker:'+model+'/text', skills=skills,
            tools=['json-schema-validator'], clearance='private_metadata',
            inRate=round(rate['input_cost_per_token'] * 10**12),
            outRate=round(rate['output_cost_per_token'] * 10**12)))
    data = dict(local=False, workers=workers,
        posteriors={s:{w['id']:dict(a=8,b=2) for w in workers} for s in skills},
        skillTools={s:[] for s in skills}, fallbackMicros=20000, floor=.24)
    template = (ROOT / 'tools/owi_ask_template.html').read_text()
    banner = '''<aside class="next-step" style="margin:0 0 24px" role="note">
<strong>Interactive example · no account needed</strong>
Sample prices from the repository fixture and assumed abilities, not a live benchmark.
No models run here. Use fictional tasks; feedback stays in this browser.
<a href="https://github.com/Morshedvarzandeh/Open-Workforce-Index/blob/main/docs/GETTING_STARTED.md">Connect your own models</a>.
</aside>'''
    page = template.replace('__DATA__', json.dumps(data).replace('</','<\\/')).replace('__BUILT__','illustrative fixture')
    page = page.replace('  <details class="guide"', banner+'\n  <details class="guide"', 1)
    (ROOT / 'demo/index.html').write_text(page)
    (ROOT / 'docs/demo.html').write_text(page)

if __name__ == '__main__':
    build()
