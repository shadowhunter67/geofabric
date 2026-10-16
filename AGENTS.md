# AGENTS.md — luật chung cho Claude (Engineering Lead) và Codex (Implementation Agent)

- Never modify Spatial IR semantics without RFC/ADR review.
- CRS must never be implicit.
- Units must never be implicit.
- Do not introduce GeoJSON into internal execution hot paths.
- Core code cannot depend on MapLibre/UI code.
- Core IR cannot depend on DuckDB/PostGIS.
- Every optimization requires benchmark evidence.
- Public API changes require documentation.
- Never silently change output semantics to make tests pass.
- If specification conflicts with implementation feasibility, report the conflict instead of bypassing the specification.
- Prefer small, reviewable commits.
- Add tests for every bug fix.
- Keep benchmark reproducible.

## Phối hợp

- Claude: phân tích, RFC/ADR, chia task, review, kiểm drift. Codex: code, test, benchmark, docs.
- Task handoff: TASK / GOAL / CONTEXT / FILES / INPUT CONTRACT / OUTPUT CONTRACT / INVARIANTS / ACCEPTANCE CRITERIA / TEST REQ / BENCHMARK REQ / OUT OF SCOPE.
- Codex trả: IMPLEMENTED / FILES CHANGED / DESIGN NOTES / TEST RESULTS / BENCHMARK RESULTS / LIMITATIONS / SPEC DEVIATIONS / OPEN QUESTIONS.
- Cần đổi Spatial IR, execution contract, CRS/unit semantics, planner, public API, storage model → báo Claude, không tự đổi.
- Không đọc file `.env`. Không build/cài nặng khi chưa được user cho phép.


## Process safety (mandatory, added 2026-10-16)

NEVER terminate processes globally by executable name.

Forbidden: `taskkill /IM node.exe`, `taskkill /IM python.exe`, `pkill node`, `killall python` (and any other name-based kill).

Only terminate processes this project started, by a verified PID or an owned process handle (`taskkill /F /PID <pid>`; check with `netstat -ano` / `tasklist` first, and check that a port is free before starting a server on it: a stale server on the same port silently serves every test, see `docs/claude/planner-hardening.md`).

Do not change the global Node installation, the nvm default, or any shared toolchain. If a specific version is needed, put it on PATH for the command only (here Node 24 at `C:/Users/<user>/AppData/Local/nvm/v24.16.0`; never `nvm use`).
