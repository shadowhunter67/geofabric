## Estimator accuracy (result rows, held-out, n=100)

| estimator | median × | P90 × | max × | over-estimates | under-estimates | actual inside [low, high] |
|---|---|---|---|---|---|---|
| v1-equivalent (no calibration) | 1.09 | 15.0 | 267.33 | 41% | 11% | 85% |
| v2 (calibrated on the calibration set) | 1.12 | 11.47 | 29.47 | 27% | 26% | 71% |

By plan shape (v2): count: median 1.0×, P90 1.0×, max 1.0× (n=9); count_by: median 1.12×, P90 9.0×, max 9.0× (n=35); features: median 1.0×, P90 11.89×, max 29.47× (n=56)

By self-reported quality (v2): fair: median 9.0×, P90 11.89×, max 23.33× (n=33); good: median 1.0×, P90 2.04×, max 29.47× (n=38); poor: median 1.12×, P90 9.0×, max 15.0× (n=29)

### Worst 12 cases and a sample of the rest

| case | dataset | query | window | limit | actual rows | estimated | interval | quality / basis | error × |
|---|---|---|---|---|---|---|---|---|---|
| ho037 | clustered_30mb_shuffled | features | 0.02 | 500 | 16 | 500 | 250–500 | good / none | 29.5 |
| ho045 | clustered_100mb_shuffled | features | 0.0005 | 2000 | 2 | 69 | 17–274 | fair / global | 23.3 |
| ho001 | clustered_30mb_shuffled | features | 0.0005 | 500 | 0 | 14 | 1–139 | poor / none | 15.0 |
| ho023 | clustered_30mb_shuffled | features | 0.0005 | 2000 | 0 | 14 | 1–139 | poor / none | 15.0 |
| ho043 | uniform_100mb_spatial | features | 0.0005 | 2000 | 657 | 53 | 13–210 | fair / global | 12.2 |
| ho028 | uniform_30mb_shuffled | features | 0.0005 | 500 | 213 | 17 | 4–67 | fair / global | 11.9 |
| ho039 | uniform_30mb_shuffled | features | 0.0005 | 20000 | 213 | 17 | 4–67 | fair / global | 11.9 |
| ho095 | uniform_30mb_shuffled | features | 0.0005 | 2000 | 213 | 17 | 4–67 | fair / global | 11.9 |
| ho067 | uniform_30mb_shuffled | features | 0.0005 | 500 | 213 | 17 | 4–67 | fair / global | 11.9 |
| ho049 | uniform_30mb_spatial | features | 0.0005 | 20000 | 194 | 16 | 3–61 | fair / global | 11.5 |
| ho094 | uniform_30mb_spatial | features | 0.0005 | 2000 | 194 | 16 | 3–61 | fair / global | 11.5 |
| ho077 | uniform_30mb_spatial | features | 0.0005 | 500 | 194 | 16 | 3–61 | fair / global | 11.5 |

<details><summary>All 100 cases</summary>

| case | dataset | query | window | limit | actual | est | error × |
|---|---|---|---|---|---|---|---|
| ho000 | clustered_30mb_spatial | count_by | 0.02 |  | 8 | 8 | 1.0 |
| ho001 | clustered_30mb_shuffled | features | 0.0005 | 500 | 0 | 14 | 15.0 |
| ho002 | uniform_30mb_spatial | features | 0.02 | 2000 | 982 | 2000 | 2.0 |
| ho003 | uniform_30mb_shuffled | count_by | 0.5 |  | 8 | 8 | 1.0 |
| ho004 | uniform_10mb_spatial | features | 0.5 | 500 | 500 | 500 | 1.0 |
| ho005 | uniform_10mb_shuffled | features | 0.02 | 500 | 500 | 500 | 1.0 |
| ho006 | clustered_10mb_shuffled | features | 0.1 | 2000 | 2000 | 2000 | 1.0 |
| ho007 | uniform_100mb_spatial | count_by | 0.02 |  | 50 | 8 | 5.7 |
| ho008 | uniform_100mb_shuffled | features | 0.02 | 500 | 500 | 500 | 1.0 |
| ho009 | clustered_100mb_shuffled | count_by | 0.0005 |  | 7 | 8 | 1.1 |
| ho010 | uniform_1gb_spatial | count_by | 0.5 |  | 7 | 8 | 1.1 |
| ho011 | clustered_30mb_spatial | count_by | 0.0005 |  | 0 | 8 | 9.0 |
| ho012 | clustered_30mb_shuffled | count_by | 0.5 |  | 8 | 8 | 1.0 |
| ho013 | uniform_30mb_spatial | features | 0.5 | 20000 | 20000 | 20000 | 1.0 |
| ho014 | uniform_30mb_shuffled | features | 0.02 | 2000 | 919 | 2000 | 2.2 |
| ho015 | uniform_10mb_spatial | features | 0.02 | 2000 | 2000 | 2000 | 1.0 |
| ho016 | uniform_10mb_shuffled | features | 0.0005 | 2000 | 13 | 6 | 2.0 |
| ho017 | clustered_10mb_shuffled | count_by | 0.1 |  | 8 | 8 | 1.0 |
| ho018 | uniform_100mb_spatial | count_by | 0.5 |  | 7 | 8 | 1.1 |
| ho019 | uniform_100mb_shuffled | count_by | 0.1 |  | 8 | 8 | 1.0 |
| ho020 | clustered_100mb_shuffled | features | 0.1 | 20000 | 20000 | 20000 | 1.0 |
| ho021 | uniform_1gb_spatial | features | 0.5 | 2000 | 2000 | 2000 | 1.0 |
| ho022 | clustered_30mb_spatial | features | 0.02 | 20000 | 4093 | 393 | 10.4 |
| ho023 | clustered_30mb_shuffled | features | 0.0005 | 2000 | 0 | 14 | 15.0 |
| ho024 | uniform_30mb_spatial | features | 0.0005 | 2000 | 25 | 16 | 1.5 |
| ho025 | uniform_30mb_shuffled | count_by | 0.0005 |  | 8 | 8 | 1.0 |
| ho026 | uniform_10mb_spatial | count | 0.1 |  | 1 | 1 | 1.0 |
| ho027 | uniform_10mb_shuffled | features | 0.0005 | 2000 | 74 | 6 | 10.7 |
| ho028 | uniform_30mb_shuffled | features | 0.0005 | 500 | 213 | 17 | 11.9 |
| ho029 | uniform_10mb_spatial | count_by | 0.02 |  | 50 | 8 | 5.7 |
| ho030 | uniform_10mb_shuffled | features | 0.0005 | 2000 | 74 | 6 | 10.7 |
| ho031 | clustered_10mb_shuffled | features | 0.1 | 2000 | 2000 | 2000 | 1.0 |
| ho032 | uniform_100mb_spatial | count_by | 0.1 |  | 50 | 8 | 5.7 |
| ho033 | uniform_100mb_shuffled | count_by | 0.1 |  | 7 | 8 | 1.1 |
| ho034 | clustered_100mb_shuffled | count | 0.1 |  | 1 | 1 | 1.0 |
| ho035 | clustered_30mb_spatial | features | 0.0005 | 20000 | 0 | 8 | 9.0 |
| ho036 | clustered_30mb_spatial | features | 0.02 | 2000 | 519 | 393 | 1.3 |
| ho037 | clustered_30mb_shuffled | features | 0.02 | 500 | 16 | 500 | 29.5 |
| ho038 | uniform_30mb_spatial | count_by | 0.5 |  | 50 | 8 | 5.7 |
| ho039 | uniform_30mb_shuffled | features | 0.0005 | 20000 | 213 | 17 | 11.9 |
| ho040 | uniform_10mb_spatial | count | 0.5 |  | 1 | 1 | 1.0 |
| ho041 | uniform_10mb_shuffled | features | 0.5 | 500 | 500 | 500 | 1.0 |
| ho042 | clustered_10mb_shuffled | features | 0.1 | 2000 | 2000 | 2000 | 1.0 |
| ho043 | uniform_100mb_spatial | features | 0.0005 | 2000 | 657 | 53 | 12.2 |
| ho044 | uniform_100mb_shuffled | count_by | 0.0005 |  | 50 | 8 | 5.7 |
| ho045 | clustered_100mb_shuffled | features | 0.0005 | 2000 | 2 | 69 | 23.3 |
| ho046 | clustered_30mb_spatial | count_by | 0.0005 |  | 0 | 8 | 9.0 |
| ho047 | clustered_30mb_spatial | features | 0.02 | 2000 | 2000 | 393 | 5.1 |
| ho048 | clustered_30mb_shuffled | count_by | 0.0005 |  | 0 | 8 | 9.0 |
| ho049 | uniform_30mb_spatial | features | 0.0005 | 20000 | 194 | 16 | 11.5 |
| ho050 | uniform_30mb_shuffled | features | 0.02 | 20000 | 7801 | 7801 | 1.0 |
| ho051 | uniform_10mb_spatial | features | 0.0005 | 500 | 59 | 6 | 8.6 |
| ho052 | uniform_10mb_shuffled | count_by | 0.5 |  | 50 | 8 | 5.7 |
| ho053 | clustered_10mb_shuffled | features | 0.5 | 500 | 500 | 500 | 1.0 |
| ho054 | uniform_100mb_spatial | features | 0.1 | 20000 | 20000 | 20000 | 1.0 |
| ho055 | uniform_100mb_shuffled | count_by | 0.5 |  | 8 | 8 | 1.0 |
| ho056 | clustered_100mb_shuffled | count_by | 0.5 |  | 7 | 8 | 1.1 |
| ho057 | clustered_30mb_spatial | count | 0.1 |  | 1 | 1 | 1.0 |
| ho058 | clustered_30mb_spatial | count_by | 0.0005 |  | 0 | 8 | 9.0 |
| ho059 | clustered_30mb_shuffled | count_by | 0.1 |  | 7 | 8 | 1.1 |
| ho060 | uniform_30mb_spatial | count | 0.02 |  | 1 | 1 | 1.0 |
| ho061 | uniform_30mb_shuffled | features | 0.02 | 2000 | 919 | 2000 | 2.2 |
| ho062 | clustered_100mb_shuffled | features | 0.02 | 2000 | 2000 | 2000 | 1.0 |
| ho063 | clustered_30mb_spatial | count | 0.0005 |  | 1 | 1 | 1.0 |
| ho064 | clustered_30mb_spatial | count_by | 0.02 |  | 7 | 8 | 1.1 |
| ho065 | clustered_30mb_shuffled | count | 0.02 |  | 1 | 1 | 1.0 |
| ho066 | uniform_30mb_spatial | features | 0.02 | 500 | 500 | 500 | 1.0 |
| ho067 | uniform_30mb_shuffled | features | 0.0005 | 500 | 213 | 17 | 11.9 |
| ho068 | uniform_10mb_spatial | features | 0.5 | 2000 | 2000 | 2000 | 1.0 |
| ho069 | uniform_10mb_shuffled | features | 0.5 | 20000 | 20000 | 20000 | 1.0 |
| ho070 | clustered_10mb_shuffled | count_by | 0.0005 |  | 0 | 8 | 9.0 |
| ho071 | uniform_100mb_spatial | features | 0.5 | 2000 | 2000 | 2000 | 1.0 |
| ho072 | uniform_100mb_shuffled | count_by | 0.02 |  | 50 | 8 | 5.7 |
| ho073 | clustered_100mb_shuffled | count_by | 0.1 |  | 7 | 8 | 1.1 |
| ho074 | uniform_1gb_spatial | count_by | 0.0005 |  | 8 | 8 | 1.0 |
| ho075 | clustered_30mb_spatial | count_by | 0.5 |  | 8 | 8 | 1.0 |
| ho076 | clustered_30mb_shuffled | count_by | 0.02 |  | 7 | 8 | 1.1 |
| ho077 | uniform_30mb_spatial | features | 0.0005 | 500 | 194 | 16 | 11.5 |
| ho078 | uniform_30mb_shuffled | features | 0.02 | 20000 | 7801 | 7801 | 1.0 |
| ho079 | uniform_10mb_spatial | features | 0.0005 | 500 | 59 | 6 | 8.6 |
| ho080 | uniform_10mb_shuffled | features | 0.02 | 500 | 500 | 500 | 1.0 |
| ho081 | clustered_10mb_shuffled | count_by | 0.02 |  | 7 | 8 | 1.1 |
| ho082 | clustered_30mb_shuffled | count | 0.02 |  | 1 | 1 | 1.0 |
| ho083 | uniform_30mb_spatial | count_by | 0.02 |  | 7 | 8 | 1.1 |
| ho084 | uniform_30mb_shuffled | features | 0.1 | 2000 | 2000 | 2000 | 1.0 |
| ho085 | uniform_10mb_spatial | count_by | 0.02 |  | 50 | 8 | 5.7 |
| ho086 | uniform_10mb_shuffled | features | 0.5 | 20000 | 20000 | 20000 | 1.0 |
| ho087 | clustered_10mb_shuffled | count_by | 0.0005 |  | 0 | 8 | 9.0 |
| ho088 | uniform_100mb_spatial | count | 0.02 |  | 1 | 1 | 1.0 |
| ho089 | uniform_100mb_shuffled | features | 0.5 | 2000 | 2000 | 2000 | 1.0 |
| ho090 | clustered_100mb_shuffled | count_by | 0.0005 |  | 7 | 8 | 1.1 |
| ho091 | clustered_30mb_spatial | features | 0.02 | 500 | 500 | 393 | 1.3 |
| ho092 | clustered_30mb_spatial | features | 0.1 | 20000 | 20000 | 20000 | 1.0 |
| ho093 | clustered_30mb_shuffled | features | 0.5 | 2000 | 2000 | 2000 | 1.0 |
| ho094 | uniform_30mb_spatial | features | 0.0005 | 2000 | 194 | 16 | 11.5 |
| ho095 | uniform_30mb_shuffled | features | 0.0005 | 2000 | 213 | 17 | 11.9 |
| ho096 | uniform_10mb_spatial | features | 0.5 | 500 | 500 | 500 | 1.0 |
| ho097 | uniform_10mb_shuffled | features | 0.5 | 2000 | 2000 | 2000 | 1.0 |
| ho098 | clustered_10mb_shuffled | features | 0.0005 | 20000 | 0 | 8 | 9.0 |
| ho099 | uniform_100mb_spatial | features | 0.5 | 500 | 500 | 500 | 1.0 |

</details>

## Time model accuracy (planner score vs measured end-to-end time)

| engine | n | median × | P90 × | max × | over-estimates | under-estimates |
|---|---|---|---|---|---|---|
| duckdb | 100 | 1.08 | 1.3 | 1.87 | 15% | 85% |
| duckdb-wasm | 96 | 1.09 | 2.69 | 4.13 | 51% | 49% |

By network profile (median × / P90 ×): FAST_LAN: server 1.04/1.33, browser 1.91/3.56; GOOD_BROADBAND: server 1.08/1.35, browser 1.03/2.69; HIGH_LATENCY: server 1.06/1.24, browser 1.01/1.36; LOCAL: server 1.13/1.33, browser 1.38/3.8; MOBILE: server 1.06/1.32, browser 1.02/2.15

## Planner accuracy and regret (held-out, 100 cases)

accuracy **98.0%** · regret median 1.0 · **P90 1.0** · max 1.08 · browser fastest in 4 cases, chosen in 4

Baselines: always-server accuracy 96.0%, P90 regret 1.0; always-browser (where measurable) median regret 9.518×, P90 49.029×.

| network | cases | accuracy | browser fastest |
|---|---|---|---|
| FAST_LAN | 14 | 100% | 0 |
| GOOD_BROADBAND | 20 | 100% | 0 |
| HIGH_LATENCY | 18 | 100% | 0 |
| LOCAL | 28 | 93% | 4 |
| MOBILE | 20 | 100% | 0 |

| browser state | cases | accuracy |
|---|---|---|
| warm | 74 | 97% |
| cold | 26 | 100% |

| confidence | cases | accuracy | mean confidence |
|---|---|---|---|
| 0.00-0.50 | 0 | 0% | None |
| 0.50-0.65 | 1 | 100% | 0.62 |
| 0.65-0.80 | 35 | 94% | 0.73 |
| 0.80-1.00 | 64 | 100% | 0.89 |

### Cases where the browser was measured faster, or the planner was wrong, or the browser was chosen

| case | network | dataset | query | window | state | selected | fastest | server ms | browser ms | correct | regret | confidence |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| ho004 | LOCAL | uniform_10mb_spatial | features | 0.5 | warm | duckdb | duckdb-wasm | 111 | 107 | **no** | 1.04 | 0.65 |
| ho026 | LOCAL | uniform_10mb_spatial | count | 0.1 | warm | duckdb-wasm | duckdb-wasm | 99 | 75 | yes | 1.00 | 0.81 |
| ho016 | LOCAL | uniform_10mb_shuffled | features | 0.0005 | warm | duckdb-wasm | duckdb-wasm | 110 | 92 | yes | 1.00 | 0.71 |
| ho027 | LOCAL | uniform_10mb_shuffled | features | 0.0005 | warm | duckdb-wasm | duckdb | 92 | 100 | **no** | 1.08 | 0.71 |
| ho017 | LOCAL | clustered_10mb_shuffled | count_by | 0.1 | warm | duckdb-wasm | duckdb-wasm | 98 | 75 | yes | 1.00 | 0.62 |

<details><summary>All 100 cases</summary>

| case | network | dataset | query | window | state | selected | fastest | server ms | browser ms | browser status | correct | regret | confidence |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| ho000 | LOCAL | clustered_30mb_spatial | count_by | 0.02 | warm | duckdb | duckdb | 97 | 108 | measured | yes | 1.00 | 0.74 |
| ho001 | LOCAL | clustered_30mb_shuffled | features | 0.0005 | cold | duckdb | duckdb | 92 | 667 | measured | yes | 1.00 | 0.76 |
| ho002 | LOCAL | uniform_30mb_spatial | features | 0.02 | warm | duckdb | duckdb | 98 | 203 | measured | yes | 1.00 | 0.95 |
| ho003 | LOCAL | uniform_30mb_shuffled | count_by | 0.5 | warm | duckdb | duckdb | 99 | 137 | measured | yes | 1.00 | 0.67 |
| ho004 | LOCAL | uniform_10mb_spatial | features | 0.5 | warm | duckdb | duckdb-wasm | 111 | 107 | measured | NO | 1.04 | 0.65 |
| ho005 | LOCAL | uniform_10mb_shuffled | features | 0.02 | warm | duckdb | duckdb | 104 | 129 | measured | yes | 1.00 | 0.66 |
| ho006 | LOCAL | clustered_10mb_shuffled | features | 0.1 | warm | duckdb | duckdb | 122 | 271 | measured | yes | 1.00 | 0.95 |
| ho007 | LOCAL | uniform_100mb_spatial | count_by | 0.02 | warm | duckdb | duckdb | 97 | 672 | measured | yes | 1.00 | 0.76 |
| ho008 | LOCAL | uniform_100mb_shuffled | features | 0.02 | cold | duckdb | duckdb | 111 | 779 | measured | yes | 1.00 | 0.95 |
| ho009 | LOCAL | clustered_100mb_shuffled | count_by | 0.0005 | warm | duckdb | duckdb | 110 | 2405 | measured | yes | 1.00 | 0.87 |
| ho010 | LOCAL | uniform_1gb_spatial | count_by | 0.5 | cold | duckdb | duckdb | 309 | None | skipped (tab limit) | yes | 1.00 | 0.76 |
| ho011 | LOCAL | clustered_30mb_spatial | count_by | 0.0005 | warm | duckdb | duckdb | 95 | 133 | measured | yes | 1.00 | 0.74 |
| ho012 | LOCAL | clustered_30mb_shuffled | count_by | 0.5 | warm | duckdb | duckdb | 107 | 150 | measured | yes | 1.00 | 0.67 |
| ho013 | LOCAL | uniform_30mb_spatial | features | 0.5 | warm | duckdb | duckdb | 385 | 1895 | measured | yes | 1.00 | 0.95 |
| ho014 | LOCAL | uniform_30mb_shuffled | features | 0.02 | cold | duckdb | duckdb | 132 | 790 | measured | yes | 1.00 | 0.95 |
| ho015 | LOCAL | uniform_10mb_spatial | features | 0.02 | warm | duckdb | duckdb | 124 | 241 | measured | yes | 1.00 | 0.76 |
| ho016 | LOCAL | uniform_10mb_shuffled | features | 0.0005 | warm | duckdb-wasm | duckdb-wasm | 110 | 92 | measured | yes | 1.00 | 0.71 |
| ho017 | LOCAL | clustered_10mb_shuffled | count_by | 0.1 | warm | duckdb-wasm | duckdb-wasm | 98 | 75 | measured | yes | 1.00 | 0.62 |
| ho018 | LOCAL | uniform_100mb_spatial | count_by | 0.5 | warm | duckdb | duckdb | 110 | 2143 | measured | yes | 1.00 | 0.76 |
| ho019 | LOCAL | uniform_100mb_shuffled | count_by | 0.1 | cold | duckdb | duckdb | 108 | 2712 | measured | yes | 1.00 | 0.76 |
| ho020 | LOCAL | clustered_100mb_shuffled | features | 0.1 | warm | duckdb | duckdb | 528 | 2857 | measured | yes | 1.00 | 0.95 |
| ho021 | LOCAL | uniform_1gb_spatial | features | 0.5 | warm | duckdb | duckdb | 143 | 584 | measured | yes | 1.00 | 0.95 |
| ho022 | LOCAL | clustered_30mb_spatial | features | 0.02 | warm | duckdb | duckdb | 161 | 475 | measured | yes | 1.00 | 0.82 |
| ho023 | LOCAL | clustered_30mb_shuffled | features | 0.0005 | warm | duckdb | duckdb | 96 | 120 | measured | yes | 1.00 | 0.66 |
| ho024 | LOCAL | uniform_30mb_spatial | features | 0.0005 | warm | duckdb | duckdb | 99 | 109 | measured | yes | 1.00 | 0.74 |
| ho025 | LOCAL | uniform_30mb_shuffled | count_by | 0.0005 | cold | duckdb | duckdb | 96 | 694 | measured | yes | 1.00 | 0.87 |
| ho026 | LOCAL | uniform_10mb_spatial | count | 0.1 | warm | duckdb-wasm | duckdb-wasm | 99 | 75 | measured | yes | 1.00 | 0.81 |
| ho027 | LOCAL | uniform_10mb_shuffled | features | 0.0005 | warm | duckdb-wasm | duckdb | 92 | 100 | measured | NO | 1.08 | 0.71 |
| ho028 | FAST_LAN | uniform_30mb_shuffled | features | 0.0005 | cold | duckdb | duckdb | 127 | 992 | measured | yes | 1.00 | 0.87 |
| ho029 | FAST_LAN | uniform_10mb_spatial | count_by | 0.02 | warm | duckdb | duckdb | 90 | 157 | measured | yes | 1.00 | 0.66 |
| ho030 | FAST_LAN | uniform_10mb_shuffled | features | 0.0005 | cold | duckdb | duckdb | 98 | 709 | measured | yes | 1.00 | 0.87 |
| ho031 | FAST_LAN | clustered_10mb_shuffled | features | 0.1 | warm | duckdb | duckdb | 147 | 364 | measured | yes | 1.00 | 0.95 |
| ho032 | FAST_LAN | uniform_100mb_spatial | count_by | 0.1 | warm | duckdb | duckdb | 106 | 2092 | measured | yes | 1.00 | 0.75 |
| ho033 | FAST_LAN | uniform_100mb_shuffled | count_by | 0.1 | cold | duckdb | duckdb | 107 | 4307 | measured | yes | 1.00 | 0.75 |
| ho034 | FAST_LAN | clustered_100mb_shuffled | count | 0.1 | warm | duckdb | duckdb | 108 | 3754 | measured | yes | 1.00 | 0.95 |
| ho035 | FAST_LAN | clustered_30mb_spatial | features | 0.0005 | cold | duckdb | duckdb | 90 | 864 | measured | yes | 1.00 | 0.87 |
| ho036 | FAST_LAN | clustered_30mb_spatial | features | 0.02 | cold | duckdb | duckdb | 102 | 922 | measured | yes | 1.00 | 0.87 |
| ho037 | FAST_LAN | clustered_30mb_shuffled | features | 0.02 | cold | duckdb | duckdb | 119 | 956 | measured | yes | 1.00 | 0.95 |
| ho038 | FAST_LAN | uniform_30mb_spatial | count_by | 0.5 | warm | duckdb | duckdb | 92 | 347 | measured | yes | 1.00 | 0.75 |
| ho039 | FAST_LAN | uniform_30mb_shuffled | features | 0.0005 | cold | duckdb | duckdb | 178 | 973 | measured | yes | 1.00 | 0.87 |
| ho040 | FAST_LAN | uniform_10mb_spatial | count | 0.5 | cold | duckdb | duckdb | 92 | 678 | measured | yes | 1.00 | 0.95 |
| ho041 | FAST_LAN | uniform_10mb_shuffled | features | 0.5 | warm | duckdb | duckdb | 105 | 196 | measured | yes | 1.00 | 0.92 |
| ho042 | GOOD_BROADBAND | clustered_10mb_shuffled | features | 0.1 | cold | duckdb | duckdb | 185 | 1233 | measured | yes | 1.00 | 0.93 |
| ho043 | GOOD_BROADBAND | uniform_100mb_spatial | features | 0.0005 | warm | duckdb | duckdb | 137 | 1787 | measured | yes | 1.00 | 0.86 |
| ho044 | GOOD_BROADBAND | uniform_100mb_shuffled | count_by | 0.0005 | warm | duckdb | duckdb | 176 | 9477 | measured | yes | 1.00 | 0.86 |
| ho045 | GOOD_BROADBAND | clustered_100mb_shuffled | features | 0.0005 | cold | duckdb | duckdb | 137 | 10111 | measured | yes | 1.00 | 0.86 |
| ho046 | GOOD_BROADBAND | clustered_30mb_spatial | count_by | 0.0005 | warm | duckdb | duckdb | 105 | 1303 | measured | yes | 1.00 | 0.86 |
| ho047 | GOOD_BROADBAND | clustered_30mb_spatial | features | 0.02 | warm | duckdb | duckdb | 160 | 1499 | measured | yes | 1.00 | 0.86 |
| ho048 | GOOD_BROADBAND | clustered_30mb_shuffled | count_by | 0.0005 | warm | duckdb | duckdb | 106 | 1365 | measured | yes | 1.00 | 0.75 |
| ho049 | GOOD_BROADBAND | uniform_30mb_spatial | features | 0.0005 | warm | duckdb | duckdb | 129 | 1355 | measured | yes | 1.00 | 0.86 |
| ho050 | GOOD_BROADBAND | uniform_30mb_shuffled | features | 0.02 | warm | duckdb | duckdb | 392 | 2117 | measured | yes | 1.00 | 0.74 |
| ho051 | GOOD_BROADBAND | uniform_10mb_spatial | features | 0.0005 | warm | duckdb | duckdb | 115 | 510 | measured | yes | 1.00 | 0.86 |
| ho052 | GOOD_BROADBAND | uniform_10mb_shuffled | count_by | 0.5 | cold | duckdb | duckdb | 105 | 1017 | measured | yes | 1.00 | 0.75 |
| ho053 | GOOD_BROADBAND | clustered_10mb_shuffled | features | 0.5 | warm | duckdb | duckdb | 134 | 567 | measured | yes | 1.00 | 0.94 |
| ho054 | GOOD_BROADBAND | uniform_100mb_spatial | features | 0.1 | warm | duckdb | duckdb | 679 | 4331 | measured | yes | 1.00 | 0.93 |
| ho055 | GOOD_BROADBAND | uniform_100mb_shuffled | count_by | 0.5 | cold | duckdb | duckdb | 123 | 10072 | measured | yes | 1.00 | 0.75 |
| ho056 | GOOD_BROADBAND | clustered_100mb_shuffled | count_by | 0.5 | warm | duckdb | duckdb | 134 | 9474 | measured | yes | 1.00 | 0.75 |
| ho057 | GOOD_BROADBAND | clustered_30mb_spatial | count | 0.1 | warm | duckdb | duckdb | 105 | 1312 | measured | yes | 1.00 | 0.94 |
| ho058 | GOOD_BROADBAND | clustered_30mb_spatial | count_by | 0.0005 | warm | duckdb | duckdb | 107 | 1305 | measured | yes | 1.00 | 0.86 |
| ho059 | GOOD_BROADBAND | clustered_30mb_shuffled | count_by | 0.1 | warm | duckdb | duckdb | 121 | 1380 | measured | yes | 1.00 | 0.75 |
| ho060 | GOOD_BROADBAND | uniform_30mb_spatial | count | 0.02 | warm | duckdb | duckdb | 106 | 1327 | measured | yes | 1.00 | 0.94 |
| ho061 | GOOD_BROADBAND | uniform_30mb_shuffled | features | 0.02 | warm | duckdb | duckdb | 178 | 1538 | measured | yes | 1.00 | 0.93 |
| ho062 | MOBILE | clustered_100mb_shuffled | features | 0.02 | warm | duckdb | duckdb | 493 | 23531 | measured | yes | 1.00 | 0.91 |
| ho063 | MOBILE | clustered_30mb_spatial | count | 0.0005 | warm | duckdb | duckdb | 169 | 8165 | measured | yes | 1.00 | 0.92 |
| ho064 | MOBILE | clustered_30mb_spatial | count_by | 0.02 | cold | duckdb | duckdb | 166 | 8692 | measured | yes | 1.00 | 0.84 |
| ho065 | MOBILE | clustered_30mb_shuffled | count | 0.02 | warm | duckdb | duckdb | 164 | 8372 | measured | yes | 1.00 | 0.92 |
| ho066 | MOBILE | uniform_30mb_spatial | features | 0.02 | cold | duckdb | duckdb | 220 | 8819 | measured | yes | 1.00 | 0.92 |
| ho067 | MOBILE | uniform_30mb_shuffled | features | 0.0005 | warm | duckdb | duckdb | 223 | 8479 | measured | yes | 1.00 | 0.84 |
| ho068 | MOBILE | uniform_10mb_spatial | features | 0.5 | warm | duckdb | duckdb | 382 | 3046 | measured | yes | 1.00 | 0.91 |
| ho069 | MOBILE | uniform_10mb_shuffled | features | 0.5 | warm | duckdb | duckdb | 2212 | 4710 | measured | yes | 1.00 | 0.90 |
| ho070 | MOBILE | clustered_10mb_shuffled | count_by | 0.0005 | warm | duckdb | duckdb | 166 | 2913 | measured | yes | 1.00 | 0.84 |
| ho071 | MOBILE | uniform_100mb_spatial | features | 0.5 | warm | duckdb | duckdb | 401 | 4170 | measured | yes | 1.00 | 0.91 |
| ho072 | MOBILE | uniform_100mb_shuffled | count_by | 0.02 | cold | duckdb | duckdb | 181 | None | error | yes | 1.00 | 0.73 |
| ho073 | MOBILE | clustered_100mb_shuffled | count_by | 0.1 | warm | duckdb | duckdb | 184 | None | error | yes | 1.00 | 0.73 |
| ho074 | MOBILE | uniform_1gb_spatial | count_by | 0.0005 | warm | duckdb | duckdb | 178 | 16853 | measured | yes | 1.00 | 0.84 |
| ho075 | MOBILE | clustered_30mb_spatial | count_by | 0.5 | warm | duckdb | duckdb | 167 | 8178 | measured | yes | 1.00 | 0.73 |
| ho076 | MOBILE | clustered_30mb_shuffled | count_by | 0.02 | warm | duckdb | duckdb | 178 | 8371 | measured | yes | 1.00 | 0.73 |
| ho077 | MOBILE | uniform_30mb_spatial | features | 0.0005 | cold | duckdb | duckdb | 194 | 8791 | measured | yes | 1.00 | 0.84 |
| ho078 | MOBILE | uniform_30mb_shuffled | features | 0.02 | cold | duckdb | duckdb | 1045 | 9675 | measured | yes | 1.00 | 0.72 |
| ho079 | MOBILE | uniform_10mb_spatial | features | 0.0005 | warm | duckdb | duckdb | 176 | 2872 | measured | yes | 1.00 | 0.84 |
| ho080 | MOBILE | uniform_10mb_shuffled | features | 0.02 | warm | duckdb | duckdb | 236 | 2949 | measured | yes | 1.00 | 0.92 |
| ho081 | MOBILE | clustered_10mb_shuffled | count_by | 0.02 | warm | duckdb | duckdb | 166 | 2927 | measured | yes | 1.00 | 0.73 |
| ho082 | HIGH_LATENCY | clustered_30mb_shuffled | count | 0.02 | warm | duckdb | duckdb | 258 | 12632 | measured | yes | 1.00 | 0.91 |
| ho083 | HIGH_LATENCY | uniform_30mb_spatial | count_by | 0.02 | warm | duckdb | duckdb | 247 | 12359 | measured | yes | 1.00 | 0.72 |
| ho084 | HIGH_LATENCY | uniform_30mb_shuffled | features | 0.1 | warm | duckdb | duckdb | 679 | 12829 | measured | yes | 1.00 | 0.90 |
| ho085 | HIGH_LATENCY | uniform_10mb_spatial | count_by | 0.02 | warm | duckdb | duckdb | 249 | 4363 | measured | yes | 1.00 | 0.72 |
| ho086 | HIGH_LATENCY | uniform_10mb_shuffled | features | 0.5 | warm | duckdb | duckdb | 3156 | 6332 | measured | yes | 1.00 | 0.89 |
| ho087 | HIGH_LATENCY | clustered_10mb_shuffled | count_by | 0.0005 | warm | duckdb | duckdb | 266 | 4412 | measured | yes | 1.00 | 0.83 |
| ho088 | HIGH_LATENCY | uniform_100mb_spatial | count | 0.02 | cold | duckdb | duckdb | 247 | 14416 | measured | yes | 1.00 | 0.91 |
| ho089 | HIGH_LATENCY | uniform_100mb_shuffled | features | 0.5 | warm | duckdb | duckdb | 568 | 4899 | measured | yes | 1.00 | 0.90 |
| ho090 | HIGH_LATENCY | clustered_100mb_shuffled | count_by | 0.0005 | cold | duckdb | duckdb | 272 | None | error | yes | 1.00 | 0.83 |
| ho091 | HIGH_LATENCY | clustered_30mb_spatial | features | 0.02 | warm | duckdb | duckdb | 337 | 12303 | measured | yes | 1.00 | 0.83 |
| ho092 | HIGH_LATENCY | clustered_30mb_spatial | features | 0.1 | warm | duckdb | duckdb | 3171 | 14133 | measured | yes | 1.00 | 0.71 |
| ho093 | HIGH_LATENCY | clustered_30mb_shuffled | features | 0.5 | warm | duckdb | duckdb | 567 | 12778 | measured | yes | 1.00 | 0.90 |
| ho094 | HIGH_LATENCY | uniform_30mb_spatial | features | 0.0005 | cold | duckdb | duckdb | 284 | 12945 | measured | yes | 1.00 | 0.83 |
| ho095 | HIGH_LATENCY | uniform_30mb_shuffled | features | 0.0005 | warm | duckdb | duckdb | 308 | 12692 | measured | yes | 1.00 | 0.83 |
| ho096 | HIGH_LATENCY | uniform_10mb_spatial | features | 0.5 | warm | duckdb | duckdb | 327 | 4381 | measured | yes | 1.00 | 0.90 |
| ho097 | HIGH_LATENCY | uniform_10mb_shuffled | features | 0.5 | warm | duckdb | duckdb | 586 | 4599 | measured | yes | 1.00 | 0.90 |
| ho098 | HIGH_LATENCY | clustered_10mb_shuffled | features | 0.0005 | warm | duckdb | duckdb | 246 | 4405 | measured | yes | 1.00 | 0.83 |
| ho099 | HIGH_LATENCY | uniform_100mb_spatial | features | 0.5 | warm | duckdb | duckdb | 355 | 6503 | measured | yes | 1.00 | 0.91 |

</details>

## Browser stability (held-out)

browser runs 96 · crashes 0 · engine errors 3 (30 s timeouts on slow links, planner had chosen the server) · skipped above the tab limit 1 · digest mismatches 0 of 29 checked · row-count mismatches 0 · browser chosen and then failed: 0
