#!/usr/bin/env python3
"""Markdown tables for REPORT-GEOFABRIC-PLANNER-HARDENING.md from results/heldout-summary.json (analyze_heldout.py)."""
import json
import os

R = os.path.join(os.path.dirname(__file__), "results")
d = json.load(open(os.path.join(R, "heldout-summary.json")))


def ef(e, a):
    x = (e + 1) / (a + 1)
    return max(x, 1 / x)


out = []
est = d["estimator_v2"]
unc = d.get("estimator_uncalibrated")
out.append("## Estimator accuracy (result rows, held-out, n=%d)\n" % est["n"])
out.append("| estimator | median × | P90 × | max × | over-estimates | under-estimates | actual inside [low, high] |\n|---|---|---|---|---|---|---|")
if unc:
    out.append(f"| v1-equivalent (no calibration) | {unc['median']} | {unc['p90']} | {unc['max']} | {unc['over_rate']:.0%} | {unc['under_rate']:.0%} | {unc['interval_coverage']:.0%} |")
out.append(f"| v2 (calibrated on the calibration set) | {est['median']} | {est['p90']} | {est['max']} | {est['over_rate']:.0%} | {est['under_rate']:.0%} | {est['interval_coverage']:.0%} |")
out.append("\nBy plan shape (v2): " + "; ".join(f"{k}: median {v['median']}×, P90 {v['p90']}×, max {v['max']}× (n={v['n']})" for k, v in d["estimator_v2_by_kind"].items()))
out.append("\nBy self-reported quality (v2): " + "; ".join(f"{k}: median {v['median']}×, P90 {v['p90']}×, max {v['max']}× (n={v['n']})" for k, v in d["estimator_v2_by_quality"].items()))
rows = sorted(d["estimator_v2_cases"], key=lambda x: -ef(x["est"], x["actual"]))
out.append("\n### Worst 12 cases and a sample of the rest\n")
out.append("| case | dataset | query | window | limit | actual rows | estimated | interval | quality / basis | error × |\n|---|---|---|---|---|---|---|---|---|---|")
for x in rows[:12]:
    out.append(f"| {x['id']} | {x['dataset']} | {x['kind']} | {x['fraction']} | {x['limit'] or ''} | {x['actual']} | {x['est']} | {x['low']}–{x['high']} | {x['quality']} / {x['basis']} | {ef(x['est'], x['actual']):.1f} |")
out.append("\n<details><summary>All %d cases</summary>\n" % len(rows))
out.append("| case | dataset | query | window | limit | actual | est | error × |\n|---|---|---|---|---|---|---|---|")
for x in sorted(rows, key=lambda x: x["id"]):
    out.append(f"| {x['id']} | {x['dataset']} | {x['kind']} | {x['fraction']} | {x['limit'] or ''} | {x['actual']} | {x['est']} | {ef(x['est'], x['actual']):.1f} |")
out.append("\n</details>")

tm = d["time_model"]
out.append("\n## Time model accuracy (planner score vs measured end-to-end time)\n")
out.append("| engine | n | median × | P90 × | max × | over-estimates | under-estimates |\n|---|---|---|---|---|---|---|")
for e, v in tm.items():
    out.append(f"| {e} | {v['n']} | {v['median']} | {v['p90']} | {v['max']} | {v['over_rate']:.0%} | {v['under_rate']:.0%} |")
out.append("\nBy network profile (median × / P90 ×): " + "; ".join(f"{n}: server {v['duckdb']['median']}/{v['duckdb']['p90']}, browser {v['duckdb-wasm']['median']}/{v['duckdb-wasm']['p90']}" for n, v in d["time_model_by_network"].items()))

p = d["planner"]
out.append("\n## Planner accuracy and regret (held-out, %d cases)\n" % p["cases"])
out.append(f"accuracy **{p['accuracy']:.1%}** · regret median {p['regret']['median']} · **P90 {p['regret_p90']}** · max {p['regret']['max']} · browser fastest in {p['browser_fastest']} cases, chosen in {p['browser_chosen']}\n")
out.append(f"Baselines: always-server accuracy {d['baseline_always_server']['accuracy']:.1%}, P90 regret {d['baseline_always_server']['regret_p90']}; always-browser (where measurable) median regret {d['baseline_always_browser_where_measurable']['regret_median']}×, P90 {d['baseline_always_browser_where_measurable']['regret_p90']}×.\n")
out.append("| network | cases | accuracy | browser fastest |\n|---|---|---|---|")
for n, v in d["by_network"].items():
    out.append(f"| {n} | {v['cases']} | {v['accuracy']:.0%} | {v['browser_fastest']} |")
out.append("\n| browser state | cases | accuracy |\n|---|---|---|")
for n, v in d["by_state"].items():
    out.append(f"| {n} | {v['cases']} | {v['accuracy']:.0%} |")
out.append("\n| confidence | cases | accuracy | mean confidence |\n|---|---|---|---|")
for b in d["confidence_bins"]:
    out.append(f"| {b['range']} | {b['cases']} | {b['accuracy']:.0%} | {b['mean_confidence']} |")
out.append("\n### Cases where the browser was measured faster, or the planner was wrong, or the browser was chosen\n")
out.append("| case | network | dataset | query | window | state | selected | fastest | server ms | browser ms | correct | regret | confidence |\n|---|---|---|---|---|---|---|---|---|---|---|---|---|")
for c in d["cases"]:
    if c["fastest"] == "duckdb-wasm" or c["chosen"] == "duckdb-wasm" or not c["correct"]:
        out.append(f"| {c['id']} | {c['network']} | {c['dataset']} | {c['kind']} | {c['fraction']} | {c['state']} | {c['chosen']} | {c['fastest']} | {c['server_ms']:.0f} | {c['browser_ms'] and round(c['browser_ms'])} | {'yes' if c['correct'] else '**no**'} | {c['regret']:.2f} | {c['confidence']:.2f} |")
out.append("\n<details><summary>All %d cases</summary>\n" % len(d["cases"]))
out.append("| case | network | dataset | query | window | state | selected | fastest | server ms | browser ms | browser status | correct | regret | confidence |\n|---|---|---|---|---|---|---|---|---|---|---|---|---|---|")
for c in sorted(d["cases"], key=lambda c: c["id"]):
    out.append(f"| {c['id']} | {c['network']} | {c['dataset']} | {c['kind']} | {c['fraction']} | {c['state']} | {c['chosen']} | {c['fastest']} | {c['server_ms']:.0f} | {c['browser_ms'] and round(c['browser_ms'])} | {c['browser_status']} | {'yes' if c['correct'] else 'NO'} | {c['regret']:.2f} | {c['confidence']:.2f} |")
out.append("\n</details>")
b = d["browser"]
out.append(f"\n## Browser stability (held-out)\n\nbrowser runs {b['runs_attempted']} · crashes {b['crashes']} · engine errors {b['errors']} (30 s timeouts on slow links, planner had chosen the server) · skipped above the tab limit {b['skipped_tab_limit']} · digest mismatches {b['digest_mismatch']} of {b['digest_checked']} checked · row-count mismatches {b['row_mismatch']} · browser chosen and then failed: {len(d['unsafe_choices'])}")
open(os.path.join(R, "summary-planner-hardening.md"), "w", encoding="utf-8").write("\n".join(out) + "\n")
print("\n".join(out)[:1500])
