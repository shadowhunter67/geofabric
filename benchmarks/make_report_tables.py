#!/usr/bin/env python3
"""Markdown tables for the planner-hardening reports from results/heldout-summary.json (analyze_heldout.py)."""
import json
import os

R = os.path.join(os.path.dirname(__file__), "results")
d = json.load(open(os.path.join(R, "heldout-summary.json")))


def ef(e, a):
    x = (e + 1) / (a + 1)
    return max(x, 1 / x)


out = []
est = d["estimator_v2"]
card = d.get("cardinality")
out.append("## Estimator error (held-out, n=%d)\n" % est["n"])
out.append("| what is estimated | median × | P90 × | P95 × | max × | over | under | under by > 5× | actual inside [low, high] |")
out.append("|---|---|---|---|---|---|---|---|---|")
if card:
    out.append(f"| **cardinality** (rows matching window and filter; truth = server COUNT) | {card['median']} | {card['p90']} | {card['p95']} | {card['max']} | {card['over_rate']:.0%} | {card['under_rate']:.0%} | {card['large_under_rate_gt5x']:.1%} | {card['interval_coverage']:.0%} |")
out.append(f"| **output rows** (what the query returns: aggregate / LIMIT applied) | {est['median']} | {est['p90']} | {est['p95']} | {est['max']} | {est['over_rate']:.0%} | {est['under_rate']:.0%} | {est['large_under_rate_gt5x']:.1%} | {est['interval_coverage']:.0%} |")
if card and d.get("cardinality_by_layout"):
    out.append("\nCardinality by layout class: " + "; ".join(f"`{k}`: median {v['median']}, P90 {v['p90']}, max {v['max']} (n={v['n']})" for k, v in d["cardinality_by_layout"].items()))
out.append("\nOutput rows by plan shape: " + "; ".join(f"{k}: median {v['median']}×, P90 {v['p90']}×, max {v['max']}× (n={v['n']})" for k, v in d["estimator_v2_by_kind"].items()))
rows = sorted(d["estimator_v2_cases"], key=lambda x: -ef(x["est"], x["actual"]))
out.append("\n### Worst 12 output-row estimates\n")
out.append("| case | dataset | query | window | limit | actual rows | estimated | interval | quality / basis | error × |")
out.append("|---|---|---|---|---|---|---|---|---|---|")
for x in rows[:12]:
    out.append(f"| {x['id']} | {x['dataset']} | {x['kind']} | {x['fraction']} | {x['limit'] or ''} | {x['actual']} | {x['est']} | {x['low']}–{x['high']} | {x['quality']} / {x['basis']} | {ef(x['est'], x['actual']):.1f} |")
out.append("\n<details><summary>All %d cases (output rows)</summary>\n" % len(rows))
out.append("| case | dataset | query | window | limit | actual | est | error × |")
out.append("|---|---|---|---|---|---|---|---|")
for x in sorted(rows, key=lambda x: x["id"]):
    out.append(f"| {x['id']} | {x['dataset']} | {x['kind']} | {x['fraction']} | {x['limit'] or ''} | {x['actual']} | {x['est']} | {ef(x['est'], x['actual']):.1f} |")
out.append("\n</details>")
if d.get("cardinality_cases"):
    out.append("\n<details><summary>All cardinality estimates</summary>\n")
    out.append("| case | dataset | window | filter | actual | est | interval | error × |")
    out.append("|---|---|---|---|---|---|---|---|")
    for c in sorted(d["cardinality_cases"], key=lambda c: c["id"]):
        out.append(f"| {c['id']} | {c['dataset']} | {c['fraction']} | {json.dumps(c['filter']) if c['filter'] else ''} | {c['actual']} | {c['est']} | {c['low']}–{c['high']} | {ef(c['est'], c['actual']):.1f} |")
    out.append("\n</details>")

tm = d["time_model"]
out.append("\n## Time model accuracy (planner score vs measured end-to-end time, Chrome-throttled network)\n")
out.append("| engine | n | median × | P90 × | P95 × | max × | over-estimates | under-estimates |")
out.append("|---|---|---|---|---|---|---|---|")
for e, v in tm.items():
    out.append(f"| {e} | {v['n']} | {v['median']} | {v['p90']} | {v['p95']} | {v['max']} | {v['over_rate']:.0%} | {v['under_rate']:.0%} |")
out.append("\nBy network profile (median × / P90 ×): " + "; ".join(f"{n}: server {v['duckdb']['median']}/{v['duckdb']['p90']}, browser {v['duckdb-wasm']['median']}/{v['duckdb-wasm']['p90']}" for n, v in d["time_model_by_network"].items()))

p = d["planner"]
out.append("\n## Planner accuracy and regret (held-out, %d cases)\n" % p["cases"])
out.append(f"accuracy **{p['accuracy']:.1%}** · regret median {p['regret']['median']} · **P90 {p['regret_p90']}** · P95 {p['regret']['p95']} · max {p['regret']['max']} · browser fastest in {p['browser_fastest']} cases, chosen in {p['browser_chosen']}\n")
out.append(f"Baselines: always-server accuracy {d['baseline_always_server']['accuracy']:.1%}, P90 regret {d['baseline_always_server']['regret_p90']}; always-browser (where measurable) median regret {d['baseline_always_browser_where_measurable']['regret_median']}×, P90 {d['baseline_always_browser_where_measurable']['regret_p90']}×.\n")
out.append("| network | cases | accuracy | browser fastest |")
out.append("|---|---|---|---|")
for n, v in d["by_network"].items():
    out.append(f"| {n} | {v['cases']} | {v['accuracy']:.0%} | {v['browser_fastest']} |")
out.append("\n| browser state | cases | accuracy |")
out.append("|---|---|---|")
for n, v in d["by_state"].items():
    out.append(f"| {n} | {v['cases']} | {v['accuracy']:.0%} |")
out.append("\n| confidence | cases | accuracy | mean confidence |")
out.append("|---|---|---|---|")
for b in d["confidence_bins"]:
    out.append(f"| {b['range']} | {b['cases']} | {b['accuracy']:.0%} | {b['mean_confidence']} |")
out.append("\n### Cases where the browser was measured faster, or the planner was wrong, or the browser was chosen\n")
out.append("| case | network | dataset | query | window | state | selected | fastest | server ms | browser ms | correct | regret | confidence |")
out.append("|---|---|---|---|---|---|---|---|---|---|---|---|---|")
for c in d["cases"]:
    if c["fastest"] == "duckdb-wasm" or c["chosen"] == "duckdb-wasm" or not c["correct"]:
        out.append(f"| {c['id']} | {c['network']} | {c['dataset']} | {c['kind']} | {c['fraction']} | {c['state']} | {c['chosen']} | {c['fastest']} | {c['server_ms']:.0f} | {c['browser_ms'] and round(c['browser_ms'])} | {'yes' if c['correct'] else '**no**'} | {c['regret']:.2f} | {c['confidence']:.2f} |")
out.append("\n<details><summary>All %d cases</summary>\n" % len(d["cases"]))
out.append("| case | network | dataset | query | window | state | selected | fastest | server ms | browser ms | browser status | correct | regret | confidence |")
out.append("|---|---|---|---|---|---|---|---|---|---|---|---|---|---|")
for c in sorted(d["cases"], key=lambda c: c["id"]):
    out.append(f"| {c['id']} | {c['network']} | {c['dataset']} | {c['kind']} | {c['fraction']} | {c['state']} | {c['chosen']} | {c['fastest']} | {c['server_ms']:.0f} | {c['browser_ms'] and round(c['browser_ms'])} | {c['browser_status']} | {'yes' if c['correct'] else 'NO'} | {c['regret']:.2f} | {c['confidence']:.2f} |")
out.append("\n</details>")
b = d["browser"]
out.append(f"\n## Browser stability (held-out)\n\nbrowser runs {b['runs_attempted']} · crashes {b['crashes']} · engine errors {b['errors']} · skipped above the safety envelope {b['skipped_tab_limit']} · digest mismatches {b['digest_mismatch']} of {b['digest_checked']} checked · row-count mismatches {b['row_mismatch']} · browser chosen and then failed: {len(d['unsafe_choices'])}")
open(os.path.join(R, "summary-planner-hardening-ii.md"), "w", encoding="utf-8").write("\n".join(out) + "\n")
print("\n".join(out)[:1200])
