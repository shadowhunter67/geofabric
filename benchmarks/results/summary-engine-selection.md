| dataset | window (% extent) | query | planner selected (default config) | scores s / b (ms) | measured s / b (ms) | actual fastest | planner correct? |
|---|---|---|---|---|---|---|---|
| clustered_10mb | 0.2% | count_by | browser | 84 / 53 | 75 / 49 | browser | yes |
| clustered_10mb | 0.2% | features (limit 2000) | browser | 87 / 70 | 79 / 56 | browser | yes |
| clustered_10mb | 0.2% | features (limit 50000) | browser | 87 / 70 | 80 / 35 | browser | yes |
| clustered_10mb | 1% | count_by | browser | 84 / 54 | 74 / 71 | browser | yes |
| clustered_10mb | 1% | features (limit 2000) | server | 105 / 166 | 82 / 41 | browser | NO |
| clustered_10mb | 1% | features (limit 50000) | server | 105 / 166 | 87 / 55 | browser | NO |
| clustered_10mb | 5% | count_by | browser | 84 / 55 | 139 / 79 | browser | yes |
| clustered_10mb | 5% | features (limit 2000) | server | 120 / 242 | 192 / 410 | server | yes |
| clustered_10mb | 5% | features (limit 50000) | server | 183 / 578 | 248 / 781 | server | yes |
| clustered_10mb | 25% | count_by | browser | 85 / 58 | 128 / 75 | browser | yes |
| clustered_10mb | 25% | features (limit 2000) | server | 120 / 245 | 172 / 413 | server | yes |
| clustered_10mb | 25% | features (limit 50000) | server | 547 / 2511 | 651 / 3369 | server | yes |
| clustered_100mb | 0.2% | count_by | server | 85 / 311 | 132 / 603 | server | yes |
| clustered_100mb | 0.2% | features (limit 2000) | server | 106 / 423 | 132 / 687 | server | yes |
| clustered_100mb | 0.2% | features (limit 50000) | server | 106 / 423 | 147 / 629 | server | yes |
| clustered_100mb | 1% | count_by | server | 85 / 316 | 138 / 556 | server | yes |
| clustered_100mb | 1% | features (limit 2000) | server | 121 / 503 | 159 / 711 | server | yes |
| clustered_100mb | 1% | features (limit 50000) | server | 199 / 921 | 162 / 573 | server | yes |
| clustered_100mb | 5% | count_by | server | 86 / 323 | 135 / 502 | server | yes |
| clustered_100mb | 5% | features (limit 2000) | server | 121 / 510 | 209 / 715 | server | yes |
| clustered_100mb | 5% | features (limit 50000) | server | 990 / 5118 | 1251 / 4118 | server | yes |
| clustered_100mb | 25% | count_by | server | 91 / 359 | 83 / 322 | server | yes |
| clustered_100mb | 25% | features (limit 2000) | server | 126 / 546 | 110 / 350 | server | yes |
| clustered_100mb | 25% | features (limit 50000) | server | 994 / 5154 | 803 / 4923 | server | yes |
| clustered_1gb | 0.2% | count_by | server | 88 / infeasible | 86 / not run | server (browser not run) | yes (browser infeasible under the default guard) |
| clustered_1gb | 0.2% | features (limit 2000) | server | 123 / infeasible | 100 / not run | server (browser not run) | yes (browser infeasible under the default guard) |
| clustered_1gb | 0.2% | features (limit 50000) | server | 234 / infeasible | 115 / not run | server (browser not run) | yes (browser infeasible under the default guard) |
| clustered_1gb | 1% | count_by | server | 91 / infeasible | 94 / not run | server (browser not run) | yes (browser infeasible under the default guard) |
| clustered_1gb | 1% | features (limit 2000) | server | 126 / infeasible | 122 / not run | server (browser not run) | yes (browser infeasible under the default guard) |
| clustered_1gb | 1% | features (limit 50000) | server | 769 / infeasible | 213 / not run | server (browser not run) | yes (browser infeasible under the default guard) |
| clustered_1gb | 5% | count_by | server | 102 / infeasible | 110 / not run | server (browser not run) | yes (browser infeasible under the default guard) |
| clustered_1gb | 5% | features (limit 2000) | server | 138 / infeasible | 121 / not run | server (browser not run) | yes (browser infeasible under the default guard) |
| clustered_1gb | 5% | features (limit 50000) | server | 1006 / infeasible | 825 / not run | server (browser not run) | yes (browser infeasible under the default guard) |
| clustered_1gb | 25% | count_by | server | 138 / infeasible | 156 / not run | server (browser not run) | yes (browser infeasible under the default guard) |
| clustered_1gb | 25% | features (limit 2000) | server | 173 / infeasible | 136 / not run | server (browser not run) | yes (browser infeasible under the default guard) |
| clustered_1gb | 25% | features (limit 50000) | server | 1041 / infeasible | 811 / not run | server (browser not run) | yes (browser infeasible under the default guard) |
| overture_buildings_vn_south | 0.2% | count_by | server | 105 / 1038 | 125 / not run | server (browser not run) | yes (browser infeasible under the default guard) |
| overture_buildings_vn_south | 0.2% | features (limit 2000) | server | 141 / 1225 | 189 / not run | server (browser not run) | yes (browser infeasible under the default guard) |
| overture_buildings_vn_south | 0.2% | features (limit 50000) | server | 1009 / 5833 | 1357 / not run | server (browser not run) | yes (browser infeasible under the default guard) |
| overture_buildings_vn_south | 1% | count_by | server | 134 / infeasible | 139 / not run | server (browser not run) | yes (browser infeasible under the default guard) |
| overture_buildings_vn_south | 1% | features (limit 2000) | server | 169 / infeasible | 186 / not run | server (browser not run) | yes (browser infeasible under the default guard) |
| overture_buildings_vn_south | 1% | features (limit 50000) | server | 1038 / infeasible | 1458 / not run | server (browser not run) | yes (browser infeasible under the default guard) |
| overture_buildings_vn_south | 5% | count_by | server | 186 / infeasible | 180 / not run | server (browser not run) | yes (browser infeasible under the default guard) |
| overture_buildings_vn_south | 5% | features (limit 2000) | server | 222 / infeasible | 191 / not run | server (browser not run) | yes (browser infeasible under the default guard) |
| overture_buildings_vn_south | 5% | features (limit 50000) | server | 1090 / infeasible | 1415 / not run | server (browser not run) | yes (browser infeasible under the default guard) |
| overture_buildings_vn_south | 25% | count_by | server | 272 / infeasible | 244 / not run | server (browser not run) | yes (browser infeasible under the default guard) |
| overture_buildings_vn_south | 25% | features (limit 2000) | server | 308 / infeasible | 185 / not run | server (browser not run) | yes (browser infeasible under the default guard) |
| overture_buildings_vn_south | 25% | features (limit 50000) | server | 1176 / infeasible | 1371 / not run | server (browser not run) | yes (browser infeasible under the default guard) |
