| query (clustered_10mb_spatial, viewport 104-105E, 12-13N) | rows | DuckDB (server) hash | DuckDB-Wasm (browser) hash | match |
|---|---|---|---|---|
| features in viewport (bbox, limit 100000) | 33 | 84159c1b8a962d16… (count 33) | 84159c1b8a962d16… (count 33) | **yes** |
| count by category in viewport (aggregate) | 8 | 333ce7595d2ad459… (count 8) | 333ce7595d2ad459… (count 8) | **yes** |
| features, category = school (attribute filter) | 6 | 2b00384a7bb5a588… (count 6) | 2b00384a7bb5a588… (count 6) | **yes** |
