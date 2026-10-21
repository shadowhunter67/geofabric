"""Write web/public/attribution.html from the public demo catalog (data/public-demo/catalog.json): the attribution each dataset requires,
verbatim, plus the ODbL notice for the derived pack and the credits of the software and services the page uses.

    python data/scripts/make_attribution_page.py
"""

from __future__ import annotations

import html
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
catalog = json.loads((ROOT / "data" / "public-demo" / "catalog.json").read_text(encoding="utf-8"))
pack = json.loads((ROOT / "data" / "public-demo" / "pack.json").read_text(encoding="utf-8"))
esc = html.escape

rows = []
for d in catalog["datasets"]:
    lic = d["license"]
    name = esc(lic["name"])
    link = f'<a href="{esc(lic["url"])}" rel="noopener noreferrer">{name}</a>' if lic.get("url", "").startswith("https://") else name
    rows.append(
        f"<tr><th scope=row>{esc(d['title']['en'])}<br><small>{esc(d['title']['vi'])}</small></th>"
        f"<td>{esc(d['provider'])}</td><td>{link}</td><td>{esc(lic.get('attribution') or '')}</td></tr>"
    )

page = f"""<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Attribution and licences - GeoFabric</title>
<style>
  :root {{ color-scheme: light dark; --bg:#fff; --fg:#1b2430; --muted:#5a6573; --line:#d6dbe1; --accent:#0f7a6d; }}
  @media (prefers-color-scheme: dark) {{ :root {{ --bg:#10161d; --fg:#e4e9ee; --muted:#9aa6b2; --line:#2a3440; --accent:#40a9a0; }} }}
  body {{ background:var(--bg); color:var(--fg); font:16px/1.55 system-ui, sans-serif; margin:0; padding:24px 16px 64px; }}
  main {{ max-width:60rem; margin:0 auto; }}
  h1 {{ font-size:1.5rem; }} h2 {{ font-size:1.15rem; margin-top:2rem; }}
  a {{ color:var(--accent); }}
  table {{ border-collapse:collapse; width:100%; font-size:.92rem; }}
  th, td {{ text-align:left; vertical-align:top; padding:8px 10px; border-bottom:1px solid var(--line); }}
  small {{ color:var(--muted); font-weight:normal; }}
  .table-wrap {{ overflow-x:auto; }}
</style>
</head>
<body>
<main>
<p><a href="./">&larr; Back to GeoFabric</a></p>
<h1>Attribution and licences</h1>
<p>GeoFabric's own code is available under MIT OR Apache-2.0. The data shown in the demo is not covered by that licence: it keeps the licences below.</p>

<h2>Demo data (derived database, ODbL)</h2>
<p>The demo pack is a small <strong>derived database of Overture Maps data</strong> (release {esc(pack['overture_release'])}), which includes data derived from OpenStreetMap contributors. It was cut to the window
{esc(', '.join(str(x) for x in pack['bbox_crs84']))} (longitude/latitude, OGC:CRS84), flattened to a simple column schema and rewritten with small row groups for browser range requests; no feature was altered.
It is made available under the <a href="https://opendatacommons.org/licenses/odbl/1-0/" rel="noopener noreferrer">Open Database License (ODbL) 1.0</a>: you may share and adapt it, you must attribute it as below, and a database you derive from it must be offered under the ODbL.</p>
<div class="table-wrap"><table>
<thead><tr><th>Dataset</th><th>Provider</th><th>Licence</th><th>Required attribution (verbatim)</th></tr></thead>
<tbody>
{chr(10).join(rows)}
</tbody></table></div>
<p>File checksums and sizes: <a href="demo/pack.json">demo/pack.json</a>. Overture attribution notes: <a href="https://docs.overturemaps.org/attribution/" rel="noopener noreferrer">docs.overturemaps.org/attribution</a>.</p>

<h2>Basemap</h2>
<p>&copy; <a href="https://openfreemap.org" rel="noopener noreferrer">OpenFreeMap</a> &copy; <a href="https://openmaptiles.org" rel="noopener noreferrer">OpenMapTiles</a> Data from <a href="https://www.openstreetmap.org/copyright" rel="noopener noreferrer">OpenStreetMap</a>. The basemap host can see which map tiles you view; GeoFabric sends it no feature or query data.</p>

<h2>Software the page runs</h2>
<ul>
<li><a href="https://duckdb.org" rel="noopener noreferrer">DuckDB-Wasm</a> (MIT). It downloads its <code>parquet</code> extension (code, not data) from <code>extensions.duckdb.org</code> on first use.</li>
<li><a href="https://maplibre.org" rel="noopener noreferrer">MapLibre GL JS</a> (BSD-3-Clause).</li>
<li>geotiff.js (MIT).</li>
</ul>

<h2>Privacy</h2>
<p>This site has no analytics, cookies or tracking. Queries run in your browser. Network contacts: this site (the app and the data files), the basemap host above, and the DuckDB extension host above.</p>
</main>
</body>
</html>
"""
out = ROOT / "web" / "public" / "attribution.html"
out.write_text(page, encoding="utf-8", newline="\n")
print("wrote", out)
