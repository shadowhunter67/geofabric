## Estimator error (held-out, n=100)

| what is estimated | median × | P90 × | P95 × | max × | over | under | under by > 5× | actual inside [low, high] |
|---|---|---|---|---|---|---|---|---|
| **cardinality** (rows matching window and filter; truth = server COUNT) | 1.0 | 1.21 | 1.5 | 2.0 | 53% | 37% | 0.0% | 99% |
| **output rows** (what the query returns: aggregate / LIMIT applied) | 1.0 | 1.12 | 1.7 | 2.0 | 28% | 6% | 0.0% | 87% |

Cardinality by layout class: `coh=high|skew=flat|cov=declared|hist=yes|filt=no`: median 1.0, P90 1.02, max 1.08 (n=17); `coh=high|skew=flat|cov=declared|hist=yes|filt=yes`: median 1.02, P90 1.22, max 1.22 (n=8); `coh=high|skew=peaked|cov=declared|hist=yes|filt=no`: median 1.0, P90 1.08, max 1.25 (n=15); `coh=high|skew=peaked|cov=declared|hist=yes|filt=yes`: median 1.01, P90 1.22, max 1.22 (n=9); `coh=low|skew=flat|cov=declared|hist=yes|filt=no`: median 1.0, P90 1.08, max 2.0 (n=35); `coh=low|skew=flat|cov=declared|hist=yes|filt=yes`: median 1.01, P90 1.5, max 1.7 (n=16)

Output rows by plan shape: count: median 1.0×, P90 1.0×, max 1.0× (n=18); count_by: median 1.0×, P90 1.67×, max 2.0× (n=34); features: median 1.0×, P90 1.16×, max 2.0× (n=48)

### Worst 12 output-row estimates

| case | dataset | query | window | limit | actual rows | estimated | interval | quality / basis | error × |
|---|---|---|---|---|---|---|---|---|---|
| h3039 | clustered_100mb_spatial_s3 | count_by | 0.01183 |  | 0 | 1 | 1–1 | fair / class | 2.0 |
| h3045 | clustered_10mb_shuffled_s3 | count_by | 0.00582 |  | 0 | 1 | 1–1 | fair / class | 2.0 |
| h3047 | clustered_30mb_shuffled_s3 | count_by | 0.00782 |  | 0 | 1 | 1–1 | fair / class | 2.0 |
| h3076 | clustered_100mb_shuffled_s3 | features | 0.02055 | 1500 | 0 | 1 | 0–2 | fair / class | 2.0 |
| h3027 | clustered_30mb_shuffled_s3 | features | 0.00428 | 1500 | 9 | 16 | 9–27 | fair / class | 1.7 |
| h3072 | clustered_10mb_shuffled_s3 | count_by | 0.00151 |  | 14 | 24 | 15–36 | fair / class | 1.7 |
| h3073 | clustered_30mb_spatial_s3 | features | 0.01426 | 1500 | 8 | 10 | 6–15 | fair / class | 1.2 |
| h3009 | uniform_30mb_shuffled_s3 | features | 0.00031 | 1500 | 13 | 16 | 9–28 | fair / class | 1.2 |
| h3016 | clustered_100mb_spatial_s3 | features | 0.00905 | 1500 | 95 | 110 | 73–165 | fair / class | 1.2 |
| h3034 | uniform_100mb_shuffled_s3 | count_by | 0.01246 |  | 7 | 8 | 8–8 | fair / class | 1.1 |
| h3035 | clustered_10mb_spatial_s3 | count_by | 0.07651 |  | 7 | 8 | 8–8 | fair / class | 1.1 |
| h3057 | clustered_10mb_shuffled_s3 | count_by | 0.12678 |  | 7 | 8 | 8–8 | fair / class | 1.1 |

<details><summary>All 100 cases (output rows)</summary>

| case | dataset | query | window | limit | actual | est | error × |
|---|---|---|---|---|---|---|---|
| h3000 | clustered_10mb_spatial_s3 | features | 0.00895 | 1500 | 23 | 23 | 1.0 |
| h3001 | clustered_10mb_shuffled_s3 | count_by | 0.02222 |  | 7 | 8 | 1.1 |
| h3002 | clustered_30mb_spatial_s3 | count | 0.00651 |  | 1 | 1 | 1.0 |
| h3003 | clustered_30mb_shuffled_s3 | count | 0.00979 |  | 1 | 1 | 1.0 |
| h3004 | clustered_100mb_spatial_s3 | features | 0.00033 | 1500 | 0 | 0 | 1.0 |
| h3005 | clustered_100mb_shuffled_s3 | features | 0.55581 | 8000 | 8000 | 8000 | 1.0 |
| h3006 | uniform_10mb_spatial_s3 | features | 0.00313 | 1500 | 387 | 381 | 1.0 |
| h3007 | uniform_10mb_shuffled_s3 | count_by | 0.33803 |  | 50 | 50 | 1.0 |
| h3008 | uniform_30mb_spatial_s3 | count | 0.00103 |  | 1 | 1 | 1.0 |
| h3009 | uniform_30mb_shuffled_s3 | features | 0.00031 | 1500 | 13 | 16 | 1.2 |
| h3010 | uniform_100mb_spatial_s3 | count | 0.11569 |  | 1 | 1 | 1.0 |
| h3011 | uniform_100mb_shuffled_s3 | features | 0.00137 | 1500 | 227 | 213 | 1.1 |
| h3012 | clustered_10mb_spatial_s3 | features | 0.02147 | 1500 | 455 | 462 | 1.0 |
| h3013 | clustered_10mb_shuffled_s3 | count | 0.0017 |  | 1 | 1 | 1.0 |
| h3014 | clustered_30mb_spatial_s3 | features | 0.03846 | 40000 | 24645 | 24562 | 1.0 |
| h3015 | clustered_30mb_shuffled_s3 | count_by | 0.00943 |  | 8 | 8 | 1.0 |
| h3016 | clustered_100mb_spatial_s3 | features | 0.00905 | 1500 | 95 | 110 | 1.2 |
| h3017 | clustered_100mb_shuffled_s3 | features | 0.02019 | 40000 | 40000 | 40000 | 1.0 |
| h3018 | uniform_10mb_spatial_s3 | count | 0.00275 |  | 1 | 1 | 1.0 |
| h3019 | uniform_10mb_shuffled_s3 | count_by | 0.00895 |  | 8 | 8 | 1.0 |
| h3020 | uniform_30mb_spatial_s3 | count_by | 0.00578 |  | 7 | 8 | 1.1 |
| h3021 | uniform_30mb_shuffled_s3 | count_by | 0.00069 |  | 50 | 50 | 1.0 |
| h3022 | uniform_100mb_spatial_s3 | count | 0.01232 |  | 1 | 1 | 1.0 |
| h3023 | uniform_100mb_shuffled_s3 | features | 0.01766 | 1500 | 1500 | 1500 | 1.0 |
| h3024 | clustered_10mb_spatial_s3 | count_by | 0.02546 |  | 7 | 8 | 1.1 |
| h3025 | clustered_10mb_shuffled_s3 | count | 0.0176 |  | 1 | 1 | 1.0 |
| h3026 | clustered_30mb_spatial_s3 | features | 0.00086 | 40000 | 0 | 0 | 1.0 |
| h3027 | clustered_30mb_shuffled_s3 | features | 0.00428 | 1500 | 9 | 16 | 1.7 |
| h3028 | clustered_100mb_shuffled_s3 | count | 0.01117 |  | 1 | 1 | 1.0 |
| h3029 | uniform_10mb_spatial_s3 | count | 0.19127 |  | 1 | 1 | 1.0 |
| h3030 | uniform_10mb_shuffled_s3 | features | 0.10996 | 1500 | 1500 | 1500 | 1.0 |
| h3031 | uniform_30mb_spatial_s3 | count_by | 0.11201 |  | 8 | 8 | 1.0 |
| h3032 | uniform_30mb_shuffled_s3 | count | 0.05609 |  | 1 | 1 | 1.0 |
| h3033 | uniform_100mb_spatial_s3 | features | 0.25566 | 1500 | 1500 | 1500 | 1.0 |
| h3034 | uniform_100mb_shuffled_s3 | count_by | 0.01246 |  | 7 | 8 | 1.1 |
| h3035 | clustered_10mb_spatial_s3 | count_by | 0.07651 |  | 7 | 8 | 1.1 |
| h3036 | clustered_10mb_shuffled_s3 | count | 0.19912 |  | 1 | 1 | 1.0 |
| h3037 | clustered_30mb_spatial_s3 | count_by | 0.17184 |  | 50 | 50 | 1.0 |
| h3038 | clustered_30mb_shuffled_s3 | features | 0.00343 | 1500 | 0 | 0 | 1.0 |
| h3039 | clustered_100mb_spatial_s3 | count_by | 0.01183 |  | 0 | 1 | 2.0 |
| h3040 | clustered_100mb_shuffled_s3 | count_by | 0.38523 |  | 50 | 50 | 1.0 |
| h3041 | uniform_10mb_spatial_s3 | features | 0.00048 | 40000 | 51 | 55 | 1.1 |
| h3042 | uniform_100mb_spatial_s3 | features | 0.56746 | 1500 | 1500 | 1500 | 1.0 |
| h3043 | uniform_100mb_shuffled_s3 | features | 0.08224 | 40000 | 40000 | 40000 | 1.0 |
| h3044 | clustered_10mb_spatial_s3 | features | 0.02194 | 1500 | 1500 | 1500 | 1.0 |
| h3045 | clustered_10mb_shuffled_s3 | count_by | 0.00582 |  | 0 | 1 | 2.0 |
| h3046 | clustered_30mb_spatial_s3 | features | 0.2071 | 8000 | 8000 | 8000 | 1.0 |
| h3047 | clustered_30mb_shuffled_s3 | count_by | 0.00782 |  | 0 | 1 | 2.0 |
| h3048 | clustered_100mb_spatial_s3 | count | 0.00179 |  | 1 | 1 | 1.0 |
| h3049 | clustered_100mb_shuffled_s3 | count_by | 0.53355 |  | 50 | 50 | 1.0 |
| h3050 | uniform_10mb_spatial_s3 | features | 0.21258 | 1500 | 1500 | 1500 | 1.0 |
| h3051 | uniform_10mb_shuffled_s3 | features | 0.00129 | 40000 | 158 | 160 | 1.0 |
| h3052 | uniform_30mb_spatial_s3 | count_by | 0.00535 |  | 50 | 50 | 1.0 |
| h3053 | uniform_30mb_shuffled_s3 | count_by | 0.04349 |  | 7 | 8 | 1.1 |
| h3054 | uniform_100mb_spatial_s3 | features | 0.00106 | 40000 | 1380 | 1392 | 1.0 |
| h3055 | uniform_100mb_shuffled_s3 | features | 0.20929 | 40000 | 40000 | 40000 | 1.0 |
| h3056 | clustered_10mb_spatial_s3 | count_by | 0.17487 |  | 50 | 50 | 1.0 |
| h3057 | clustered_10mb_shuffled_s3 | count_by | 0.12678 |  | 7 | 8 | 1.1 |
| h3058 | clustered_30mb_spatial_s3 | count_by | 0.00136 |  | 7 | 8 | 1.1 |
| h3059 | clustered_30mb_shuffled_s3 | features | 0.13482 | 40000 | 40000 | 40000 | 1.0 |
| h3060 | clustered_100mb_spatial_s3 | features | 0.00269 | 1500 | 0 | 0 | 1.0 |
| h3061 | clustered_100mb_shuffled_s3 | count | 0.00684 |  | 1 | 1 | 1.0 |
| h3062 | clustered_30mb_shuffled_s3 | features | 0.17554 | 300 | 300 | 300 | 1.0 |
| h3063 | clustered_100mb_spatial_s3 | features | 0.01878 | 1500 | 821 | 772 | 1.1 |
| h3064 | clustered_100mb_shuffled_s3 | features | 0.0273 | 40000 | 23392 | 23315 | 1.0 |
| h3065 | uniform_10mb_spatial_s3 | features | 0.00205 | 40000 | 274 | 278 | 1.0 |
| h3066 | uniform_10mb_shuffled_s3 | count_by | 0.01111 |  | 8 | 8 | 1.0 |
| h3067 | uniform_30mb_spatial_s3 | features | 0.01268 | 300 | 300 | 300 | 1.0 |
| h3068 | uniform_30mb_shuffled_s3 | count_by | 0.02204 |  | 50 | 50 | 1.0 |
| h3069 | uniform_100mb_spatial_s3 | count_by | 0.00728 |  | 8 | 8 | 1.0 |
| h3070 | uniform_100mb_shuffled_s3 | features | 0.00068 | 1500 | 105 | 108 | 1.0 |
| h3071 | clustered_10mb_spatial_s3 | features | 0.23329 | 1500 | 1500 | 1500 | 1.0 |
| h3072 | clustered_10mb_shuffled_s3 | count_by | 0.00151 |  | 14 | 24 | 1.7 |
| h3073 | clustered_30mb_spatial_s3 | features | 0.01426 | 1500 | 8 | 10 | 1.2 |
| h3074 | clustered_30mb_shuffled_s3 | count_by | 0.16456 |  | 50 | 50 | 1.0 |
| h3075 | clustered_100mb_spatial_s3 | count_by | 0.00363 |  | 1 | 1 | 1.0 |
| h3076 | clustered_100mb_shuffled_s3 | features | 0.02055 | 1500 | 0 | 1 | 2.0 |
| h3077 | uniform_10mb_spatial_s3 | features | 0.01343 | 300 | 300 | 300 | 1.0 |
| h3078 | uniform_10mb_shuffled_s3 | features | 0.0042 | 1500 | 554 | 561 | 1.0 |
| h3079 | uniform_30mb_spatial_s3 | count | 0.02954 |  | 1 | 1 | 1.0 |
| h3080 | uniform_30mb_shuffled_s3 | count_by | 0.0407 |  | 50 | 50 | 1.0 |
| h3081 | uniform_100mb_spatial_s3 | features | 0.67762 | 300 | 300 | 300 | 1.0 |
| h3082 | uniform_30mb_spatial_s3 | count | 0.61429 |  | 1 | 1 | 1.0 |
| h3083 | uniform_30mb_shuffled_s3 | count | 0.00808 |  | 1 | 1 | 1.0 |
| h3084 | uniform_100mb_spatial_s3 | features | 0.33886 | 8000 | 8000 | 8000 | 1.0 |
| h3085 | uniform_100mb_shuffled_s3 | count_by | 0.00304 |  | 7 | 8 | 1.1 |
| h3086 | clustered_10mb_spatial_s3 | features | 0.03839 | 1500 | 1500 | 1500 | 1.0 |
| h3087 | clustered_10mb_shuffled_s3 | count_by | 0.05036 |  | 50 | 50 | 1.0 |
| h3088 | uniform_1gb_shuffled | features | 0.00269 | 40000 | 35013 | 35029 | 1.0 |
| h3089 | clustered_30mb_shuffled_s3 | count_by | 0.01775 |  | 50 | 50 | 1.0 |
| h3090 | clustered_100mb_spatial_s3 | features | 0.00072 | 40000 | 0 | 0 | 1.0 |
| h3091 | clustered_100mb_shuffled_s3 | features | 0.00037 | 1500 | 1500 | 1469 | 1.0 |
| h3092 | uniform_10mb_spatial_s3 | count | 0.00042 |  | 1 | 1 | 1.0 |
| h3093 | uniform_10mb_shuffled_s3 | count_by | 0.18984 |  | 8 | 8 | 1.0 |
| h3094 | uniform_30mb_spatial_s3 | features | 0.04506 | 1500 | 1500 | 1500 | 1.0 |
| h3095 | uniform_30mb_shuffled_s3 | count_by | 0.15119 |  | 7 | 8 | 1.1 |
| h3096 | uniform_100mb_spatial_s3 | count_by | 0.40898 |  | 50 | 50 | 1.0 |
| h3097 | uniform_100mb_shuffled_s3 | features | 0.00749 | 300 | 300 | 300 | 1.0 |
| h3098 | clustered_10mb_spatial_s3 | features | 0.0368 | 300 | 78 | 84 | 1.1 |
| h3099 | clustered_10mb_shuffled_s3 | features | 0.13326 | 300 | 300 | 300 | 1.0 |

</details>

<details><summary>All cardinality estimates</summary>

| case | dataset | window | filter | actual | est | interval | error × |
|---|---|---|---|---|---|---|---|
| h3000 | clustered_10mb_spatial_s3 | 0.00895 |  | 23 | 23 | 15–35 | 1.0 |
| h3001 | clustered_10mb_shuffled_s3 | 0.02222 | {"column": "category", "op": "ne", "value": "other"} | 678 | 681 | 453–1021 | 1.0 |
| h3002 | clustered_30mb_spatial_s3 | 0.00651 | {"column": "height", "op": "gt", "value": 20} | 715 | 710 | 473–1065 | 1.0 |
| h3003 | clustered_30mb_shuffled_s3 | 0.00979 | {"column": "height", "op": "gt", "value": 20} | 273 | 259 | 172–389 | 1.1 |
| h3004 | clustered_100mb_spatial_s3 | 0.00033 |  | 0 | 0 | 0–0 | 1.0 |
| h3005 | clustered_100mb_shuffled_s3 | 0.55581 |  | 1144680 | 1145156 | 763437–1300000 | 1.0 |
| h3006 | uniform_10mb_spatial_s3 | 0.00313 |  | 387 | 381 | 253–571 | 1.0 |
| h3007 | uniform_10mb_shuffled_s3 | 0.33803 |  | 44147 | 44154 | 29435–66231 | 1.0 |
| h3008 | uniform_30mb_spatial_s3 | 0.00103 |  | 432 | 430 | 286–644 | 1.0 |
| h3009 | uniform_30mb_shuffled_s3 | 0.00031 | {"column": "category", "op": "eq", "value": "school"} | 13 | 16 | 9–28 | 1.2 |
| h3010 | uniform_100mb_spatial_s3 | 0.11569 |  | 150440 | 150338 | 100225–225507 | 1.0 |
| h3011 | uniform_100mb_shuffled_s3 | 0.00137 | {"column": "category", "op": "eq", "value": "school"} | 227 | 213 | 141–319 | 1.1 |
| h3012 | clustered_10mb_spatial_s3 | 0.02147 |  | 455 | 462 | 307–692 | 1.0 |
| h3013 | clustered_10mb_shuffled_s3 | 0.0017 | {"column": "height", "op": "gt", "value": 20} | 6 | 9 | 4–14 | 1.4 |
| h3014 | clustered_30mb_spatial_s3 | 0.03846 |  | 24645 | 24562 | 16374–36843 | 1.0 |
| h3015 | clustered_30mb_shuffled_s3 | 0.00943 |  | 1193 | 1289 | 859–1933 | 1.1 |
| h3016 | clustered_100mb_spatial_s3 | 0.00905 | {"column": "category", "op": "eq", "value": "school"} | 95 | 110 | 73–165 | 1.2 |
| h3017 | clustered_100mb_shuffled_s3 | 0.02019 |  | 50617 | 50485 | 33656–75727 | 1.0 |
| h3018 | uniform_10mb_spatial_s3 | 0.00275 | {"column": "height", "op": "gt", "value": 20} | 54 | 57 | 37–86 | 1.1 |
| h3019 | uniform_10mb_shuffled_s3 | 0.00895 |  | 1193 | 1171 | 780–1757 | 1.0 |
| h3020 | uniform_30mb_spatial_s3 | 0.00578 | {"column": "category", "op": "ne", "value": "other"} | 1940 | 1903 | 1268–2854 | 1.0 |
| h3021 | uniform_30mb_shuffled_s3 | 0.00069 |  | 210 | 221 | 146–331 | 1.1 |
| h3022 | uniform_100mb_spatial_s3 | 0.01232 | {"column": "height", "op": "gt", "value": 20} | 2373 | 2459 | 1639–3689 | 1.0 |
| h3023 | uniform_100mb_shuffled_s3 | 0.01766 | {"column": "category", "op": "eq", "value": "school"} | 2850 | 2841 | 1893–4261 | 1.0 |
| h3024 | clustered_10mb_spatial_s3 | 0.02546 | {"column": "category", "op": "ne", "value": "other"} | 15230 | 15201 | 10133–22802 | 1.0 |
| h3025 | clustered_10mb_shuffled_s3 | 0.0176 | {"column": "height", "op": "gt", "value": 20} | 1997 | 2014 | 1342–3021 | 1.0 |
| h3026 | clustered_30mb_spatial_s3 | 0.00086 |  | 0 | 0 | 0–0 | 1.0 |
| h3027 | clustered_30mb_shuffled_s3 | 0.00428 | {"column": "category", "op": "eq", "value": "school"} | 9 | 16 | 9–27 | 1.7 |
| h3028 | clustered_100mb_shuffled_s3 | 0.01117 |  | 0 | 1 | 0–1 | 2.0 |
| h3029 | uniform_10mb_spatial_s3 | 0.19127 |  | 24698 | 24699 | 16465–35000 | 1.0 |
| h3030 | uniform_10mb_shuffled_s3 | 0.10996 | {"column": "category", "op": "eq", "value": "school"} | 1831 | 1822 | 1214–2732 | 1.0 |
| h3031 | uniform_30mb_spatial_s3 | 0.11201 |  | 43671 | 43678 | 29118–65517 | 1.0 |
| h3032 | uniform_30mb_shuffled_s3 | 0.05609 |  | 22022 | 22026 | 14683–33039 | 1.0 |
| h3033 | uniform_100mb_spatial_s3 | 0.25566 | {"column": "category", "op": "eq", "value": "school"} | 41437 | 41278 | 27518–61917 | 1.0 |
| h3034 | uniform_100mb_shuffled_s3 | 0.01246 | {"column": "category", "op": "ne", "value": "other"} | 14081 | 14048 | 9365–21072 | 1.0 |
| h3035 | clustered_10mb_spatial_s3 | 0.07651 | {"column": "category", "op": "ne", "value": "other"} | 4342 | 4406 | 2937–6609 | 1.0 |
| h3036 | clustered_10mb_shuffled_s3 | 0.19912 |  | 64088 | 64008 | 42671–96012 | 1.0 |
| h3037 | clustered_30mb_spatial_s3 | 0.17184 |  | 105713 | 106066 | 70710–159099 | 1.0 |
| h3038 | clustered_30mb_shuffled_s3 | 0.00343 |  | 0 | 0 | 0–0 | 1.0 |
| h3039 | clustered_100mb_spatial_s3 | 0.01183 | {"column": "category", "op": "ne", "value": "other"} | 0 | 0 | 0–0 | 1.0 |
| h3040 | clustered_100mb_shuffled_s3 | 0.38523 |  | 885127 | 885354 | 590235–1300000 | 1.0 |
| h3041 | uniform_10mb_spatial_s3 | 0.00048 |  | 51 | 55 | 36–82 | 1.1 |
| h3042 | uniform_100mb_spatial_s3 | 0.56746 |  | 737656 | 737620 | 491746–1106429 | 1.0 |
| h3043 | uniform_100mb_shuffled_s3 | 0.08224 |  | 107200 | 107221 | 71480–160832 | 1.0 |
| h3044 | clustered_10mb_spatial_s3 | 0.02194 |  | 5373 | 5361 | 3573–8042 | 1.0 |
| h3045 | clustered_10mb_shuffled_s3 | 0.00582 |  | 0 | 0 | 0–0 | 1.0 |
| h3046 | clustered_30mb_spatial_s3 | 0.2071 |  | 171730 | 171919 | 114612–240000 | 1.0 |
| h3047 | clustered_30mb_shuffled_s3 | 0.00782 |  | 0 | 0 | 0–0 | 1.0 |
| h3048 | clustered_100mb_spatial_s3 | 0.00179 |  | 7 | 9 | 5–13 | 1.2 |
| h3049 | clustered_100mb_shuffled_s3 | 0.53355 |  | 1133257 | 1134305 | 756203–1300000 | 1.0 |
| h3050 | uniform_10mb_spatial_s3 | 0.21258 |  | 27554 | 27574 | 18382–41361 | 1.0 |
| h3051 | uniform_10mb_shuffled_s3 | 0.00129 |  | 158 | 160 | 106–240 | 1.0 |
| h3052 | uniform_30mb_spatial_s3 | 0.00535 |  | 2073 | 2060 | 1372–3089 | 1.0 |
| h3053 | uniform_30mb_shuffled_s3 | 0.04349 | {"column": "category", "op": "ne", "value": "other"} | 14995 | 14907 | 9937–22360 | 1.0 |
| h3054 | uniform_100mb_spatial_s3 | 0.00106 |  | 1380 | 1392 | 927–2087 | 1.0 |
| h3055 | uniform_100mb_shuffled_s3 | 0.20929 |  | 272081 | 271944 | 181295–407916 | 1.0 |
| h3056 | clustered_10mb_spatial_s3 | 0.17487 |  | 40637 | 40599 | 27065–60898 | 1.0 |
| h3057 | clustered_10mb_shuffled_s3 | 0.12678 | {"column": "category", "op": "ne", "value": "other"} | 14529 | 14554 | 9702–21830 | 1.0 |
| h3058 | clustered_30mb_spatial_s3 | 0.00136 | {"column": "category", "op": "ne", "value": "other"} | 51 | 60 | 4–714 | 1.2 |
| h3059 | clustered_30mb_shuffled_s3 | 0.13482 |  | 152580 | 152629 | 101752–228943 | 1.0 |
| h3060 | clustered_100mb_spatial_s3 | 0.00269 | {"column": "category", "op": "eq", "value": "school"} | 0 | 0 | 0–0 | 1.0 |
| h3061 | clustered_100mb_shuffled_s3 | 0.00684 | {"column": "height", "op": "gt", "value": 20} | 1 | 2 | 0–2 | 1.5 |
| h3062 | clustered_30mb_shuffled_s3 | 0.17554 |  | 142126 | 142258 | 94838–213387 | 1.0 |
| h3063 | clustered_100mb_spatial_s3 | 0.01878 | {"column": "category", "op": "eq", "value": "school"} | 821 | 772 | 64–9254 | 1.1 |
| h3064 | clustered_100mb_shuffled_s3 | 0.0273 |  | 23392 | 23315 | 15543–34973 | 1.0 |
| h3065 | uniform_10mb_spatial_s3 | 0.00205 |  | 274 | 278 | 185–417 | 1.0 |
| h3066 | uniform_10mb_shuffled_s3 | 0.01111 |  | 1467 | 1459 | 972–2188 | 1.0 |
| h3067 | uniform_30mb_spatial_s3 | 0.01268 |  | 4922 | 4921 | 3280–7381 | 1.0 |
| h3068 | uniform_30mb_shuffled_s3 | 0.02204 |  | 8629 | 8613 | 5741–12920 | 1.0 |
| h3069 | uniform_100mb_spatial_s3 | 0.00728 |  | 9501 | 9502 | 6334–14253 | 1.0 |
| h3070 | uniform_100mb_shuffled_s3 | 0.00068 | {"column": "category", "op": "eq", "value": "school"} | 105 | 108 | 63–183 | 1.0 |
| h3071 | clustered_10mb_spatial_s3 | 0.23329 |  | 10299 | 10341 | 6893–15511 | 1.0 |
| h3072 | clustered_10mb_shuffled_s3 | 0.00151 |  | 14 | 24 | 15–36 | 1.7 |
| h3073 | clustered_30mb_spatial_s3 | 0.01426 | {"column": "category", "op": "eq", "value": "school"} | 8 | 10 | 6–15 | 1.2 |
| h3074 | clustered_30mb_shuffled_s3 | 0.16456 |  | 69137 | 69064 | 46042–103596 | 1.0 |
| h3075 | clustered_100mb_spatial_s3 | 0.00363 |  | 1 | 1 | 0–1 | 1.0 |
| h3076 | clustered_100mb_shuffled_s3 | 0.02055 |  | 0 | 1 | 0–2 | 2.0 |
| h3077 | uniform_10mb_spatial_s3 | 0.01343 |  | 1725 | 1732 | 1154–2598 | 1.0 |
| h3078 | uniform_10mb_shuffled_s3 | 0.0042 |  | 554 | 561 | 373–841 | 1.0 |
| h3079 | uniform_30mb_spatial_s3 | 0.02954 | {"column": "height", "op": "gt", "value": 20} | 1756 | 1759 | 1172–2639 | 1.0 |
| h3080 | uniform_30mb_shuffled_s3 | 0.0407 |  | 15806 | 15802 | 10534–23703 | 1.0 |
| h3081 | uniform_100mb_spatial_s3 | 0.67762 |  | 881221 | 881447 | 587631–1200000 | 1.0 |
| h3082 | uniform_30mb_spatial_s3 | 0.61429 | {"column": "height", "op": "gt", "value": 20} | 36636 | 36827 | 24551–55240 | 1.0 |
| h3083 | uniform_30mb_shuffled_s3 | 0.00808 |  | 3187 | 3203 | 2134–4804 | 1.0 |
| h3084 | uniform_100mb_spatial_s3 | 0.33886 |  | 441078 | 440984 | 293989–660000 | 1.0 |
| h3085 | uniform_100mb_shuffled_s3 | 0.00304 | {"column": "category", "op": "ne", "value": "other"} | 3445 | 3415 | 2276–5123 | 1.0 |
| h3086 | clustered_10mb_spatial_s3 | 0.03839 |  | 9217 | 9223 | 6148–10000 | 1.0 |
| h3087 | clustered_10mb_shuffled_s3 | 0.05036 |  | 8523 | 8538 | 5691–12807 | 1.0 |
| h3088 | uniform_1gb_shuffled | 0.00269 |  | 35013 | 35029 | 23352–52544 | 1.0 |
| h3089 | clustered_30mb_shuffled_s3 | 0.01775 |  | 17474 | 17348 | 11564–26021 | 1.0 |
| h3090 | clustered_100mb_spatial_s3 | 0.00072 |  | 0 | 0 | 0–0 | 1.0 |
| h3091 | clustered_100mb_shuffled_s3 | 0.00037 |  | 1527 | 1469 | 978–2203 | 1.0 |
| h3092 | uniform_10mb_spatial_s3 | 0.00042 | {"column": "height", "op": "gt", "value": 20} | 8 | 10 | 0–111 | 1.2 |
| h3093 | uniform_10mb_shuffled_s3 | 0.18984 |  | 24827 | 24812 | 16540–37218 | 1.0 |
| h3094 | uniform_30mb_spatial_s3 | 0.04506 | {"column": "category", "op": "eq", "value": "school"} | 2248 | 2193 | 1461–3289 | 1.0 |
| h3095 | uniform_30mb_shuffled_s3 | 0.15119 | {"column": "category", "op": "ne", "value": "other"} | 51684 | 51679 | 34452–77518 | 1.0 |
| h3096 | uniform_100mb_spatial_s3 | 0.40898 |  | 531565 | 531586 | 354390–797378 | 1.0 |
| h3097 | uniform_100mb_shuffled_s3 | 0.00749 |  | 9782 | 9803 | 6534–14704 | 1.0 |
| h3098 | clustered_10mb_spatial_s3 | 0.0368 |  | 78 | 84 | 55–126 | 1.1 |
| h3099 | clustered_10mb_shuffled_s3 | 0.13326 |  | 44725 | 44652 | 29767–66977 | 1.0 |

</details>

## Time model accuracy (planner score vs measured end-to-end time, Chrome-throttled network)

| engine | n | median × | P90 × | P95 × | max × | over-estimates | under-estimates |
|---|---|---|---|---|---|---|---|
| duckdb | 100 | 1.54 | 2.26 | 2.74 | 5.73 | 76% | 24% |
| duckdb-wasm | 99 | 4.21 | 78.0 | 91.98 | 159.7 | 72% | 28% |

By network profile (median × / P90 ×): FAST_LAN: server 1.6/2.21, browser 14.79/21.34; GOOD_BROADBAND: server 1.62/2.34, browser 4.23/7.09; HIGH_LATENCY: server 1.12/1.38, browser 1.56/2.91; LOCAL: server 1.92/4.44, browser 54.1/124.31; MOBILE: server 1.38/1.53, browser 1.07/3.12

## Planner accuracy and regret (held-out, 100 cases)

accuracy **86.0%** · regret median 1.0 · **P90 1.406** · P95 1.57 · max 2.26 · browser fastest in 14 cases, chosen in 0

Baselines: always-server accuracy 86.0%, P90 regret 1.406; always-browser (where measurable) median regret 11.276×, P90 132.765×.

| network | cases | accuracy | browser fastest |
|---|---|---|---|
| FAST_LAN | 14 | 100% | 0 |
| GOOD_BROADBAND | 20 | 100% | 0 |
| HIGH_LATENCY | 18 | 100% | 0 |
| LOCAL | 28 | 50% | 14 |
| MOBILE | 20 | 100% | 0 |

| browser state | cases | accuracy |
|---|---|---|
| warm | 74 | 81% |
| cold | 26 | 100% |

| confidence | cases | accuracy | mean confidence |
|---|---|---|---|
| 0.00-0.50 | 0 | 0% | None |
| 0.50-0.65 | 0 | 0% | None |
| 0.65-0.80 | 2 | 100% | 0.73 |
| 0.80-1.00 | 98 | 86% | 0.87 |

### Cases where the browser was measured faster, or the planner was wrong, or the browser was chosen

| case | network | dataset | query | window | state | selected | fastest | server ms | browser ms | correct | regret | confidence |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| h3000 | LOCAL | clustered_10mb_spatial_s3 | features | 0.00895 | warm | duckdb | duckdb-wasm | 78 | 45 | **no** | 1.72 | 0.84 |
| h3012 | LOCAL | clustered_10mb_spatial_s3 | features | 0.02147 | warm | duckdb | duckdb-wasm | 88 | 81 | **no** | 1.09 | 0.84 |
| h3024 | LOCAL | clustered_10mb_spatial_s3 | count_by | 0.02546 | warm | duckdb | duckdb-wasm | 92 | 59 | **no** | 1.55 | 0.84 |
| h3001 | LOCAL | clustered_10mb_shuffled_s3 | count_by | 0.02222 | warm | duckdb | duckdb-wasm | 89 | 57 | **no** | 1.57 | 0.84 |
| h3013 | LOCAL | clustered_10mb_shuffled_s3 | count | 0.0017 | warm | duckdb | duckdb-wasm | 84 | 78 | **no** | 1.07 | 0.92 |
| h3025 | LOCAL | clustered_10mb_shuffled_s3 | count | 0.0176 | warm | duckdb | duckdb-wasm | 84 | 51 | **no** | 1.64 | 0.92 |
| h3026 | LOCAL | clustered_30mb_spatial_s3 | features | 0.00086 | warm | duckdb | duckdb-wasm | 80 | 50 | **no** | 1.59 | 0.84 |
| h3003 | LOCAL | clustered_30mb_shuffled_s3 | count | 0.00979 | warm | duckdb | duckdb-wasm | 109 | 84 | **no** | 1.30 | 0.92 |
| h3004 | LOCAL | clustered_100mb_spatial_s3 | features | 0.00033 | warm | duckdb | duckdb-wasm | 83 | 57 | **no** | 1.46 | 0.84 |
| h3016 | LOCAL | clustered_100mb_spatial_s3 | features | 0.00905 | warm | duckdb | duckdb-wasm | 88 | 58 | **no** | 1.51 | 0.84 |
| h3018 | LOCAL | uniform_10mb_spatial_s3 | count | 0.00275 | warm | duckdb | duckdb-wasm | 86 | 38 | **no** | 2.26 | 0.92 |
| h3007 | LOCAL | uniform_10mb_shuffled_s3 | count_by | 0.33803 | warm | duckdb | duckdb-wasm | 82 | 55 | **no** | 1.48 | 0.84 |
| h3020 | LOCAL | uniform_30mb_spatial_s3 | count_by | 0.00578 | warm | duckdb | duckdb-wasm | 91 | 65 | **no** | 1.41 | 0.92 |
| h3022 | LOCAL | uniform_100mb_spatial_s3 | count | 0.01232 | warm | duckdb | duckdb-wasm | 77 | 64 | **no** | 1.21 | 0.92 |

<details><summary>All 100 cases</summary>

| case | network | dataset | query | window | state | selected | fastest | server ms | browser ms | browser status | correct | regret | confidence |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| h3000 | LOCAL | clustered_10mb_spatial_s3 | features | 0.00895 | warm | duckdb | duckdb-wasm | 78 | 45 | measured | NO | 1.72 | 0.84 |
| h3001 | LOCAL | clustered_10mb_shuffled_s3 | count_by | 0.02222 | warm | duckdb | duckdb-wasm | 89 | 57 | measured | NO | 1.57 | 0.84 |
| h3002 | LOCAL | clustered_30mb_spatial_s3 | count | 0.00651 | cold | duckdb | duckdb | 89 | 647 | measured | yes | 1.00 | 0.92 |
| h3003 | LOCAL | clustered_30mb_shuffled_s3 | count | 0.00979 | warm | duckdb | duckdb-wasm | 109 | 84 | measured | NO | 1.30 | 0.92 |
| h3004 | LOCAL | clustered_100mb_spatial_s3 | features | 0.00033 | warm | duckdb | duckdb-wasm | 83 | 57 | measured | NO | 1.46 | 0.84 |
| h3005 | LOCAL | clustered_100mb_shuffled_s3 | features | 0.55581 | warm | duckdb | duckdb | 208 | 886 | measured | yes | 1.00 | 0.90 |
| h3006 | LOCAL | uniform_10mb_spatial_s3 | features | 0.00313 | cold | duckdb | duckdb | 86 | 628 | measured | yes | 1.00 | 0.92 |
| h3007 | LOCAL | uniform_10mb_shuffled_s3 | count_by | 0.33803 | warm | duckdb | duckdb-wasm | 82 | 55 | measured | NO | 1.48 | 0.84 |
| h3008 | LOCAL | uniform_30mb_spatial_s3 | count | 0.00103 | cold | duckdb | duckdb | 79 | 624 | measured | yes | 1.00 | 0.92 |
| h3009 | LOCAL | uniform_30mb_shuffled_s3 | features | 0.00031 | warm | duckdb | duckdb | 92 | 106 | measured | yes | 1.00 | 0.84 |
| h3010 | LOCAL | uniform_100mb_spatial_s3 | count | 0.11569 | warm | duckdb | duckdb | 85 | 118 | measured | yes | 1.00 | 0.92 |
| h3011 | LOCAL | uniform_100mb_shuffled_s3 | features | 0.00137 | cold | duckdb | duckdb | 146 | 1269 | measured | yes | 1.00 | 0.84 |
| h3012 | LOCAL | clustered_10mb_spatial_s3 | features | 0.02147 | warm | duckdb | duckdb-wasm | 88 | 81 | measured | NO | 1.09 | 0.84 |
| h3013 | LOCAL | clustered_10mb_shuffled_s3 | count | 0.0017 | warm | duckdb | duckdb-wasm | 84 | 78 | measured | NO | 1.07 | 0.92 |
| h3014 | LOCAL | clustered_30mb_spatial_s3 | features | 0.03846 | cold | duckdb | duckdb | 446 | 2933 | measured | yes | 1.00 | 0.82 |
| h3015 | LOCAL | clustered_30mb_shuffled_s3 | count_by | 0.00943 | warm | duckdb | duckdb | 94 | 101 | measured | yes | 1.00 | 0.84 |
| h3016 | LOCAL | clustered_100mb_spatial_s3 | features | 0.00905 | warm | duckdb | duckdb-wasm | 88 | 58 | measured | NO | 1.51 | 0.84 |
| h3017 | LOCAL | clustered_100mb_shuffled_s3 | features | 0.02019 | cold | duckdb | duckdb | 920 | 5058 | measured | yes | 1.00 | 0.82 |
| h3018 | LOCAL | uniform_10mb_spatial_s3 | count | 0.00275 | warm | duckdb | duckdb-wasm | 86 | 38 | measured | NO | 2.26 | 0.92 |
| h3019 | LOCAL | uniform_10mb_shuffled_s3 | count_by | 0.00895 | cold | duckdb | duckdb | 80 | 632 | measured | yes | 1.00 | 0.84 |
| h3020 | LOCAL | uniform_30mb_spatial_s3 | count_by | 0.00578 | warm | duckdb | duckdb-wasm | 91 | 65 | measured | NO | 1.41 | 0.92 |
| h3021 | LOCAL | uniform_30mb_shuffled_s3 | count_by | 0.00069 | warm | duckdb | duckdb | 86 | 90 | measured | yes | 1.00 | 0.84 |
| h3022 | LOCAL | uniform_100mb_spatial_s3 | count | 0.01232 | warm | duckdb | duckdb-wasm | 77 | 64 | measured | NO | 1.21 | 0.92 |
| h3023 | LOCAL | uniform_100mb_shuffled_s3 | features | 0.01766 | warm | duckdb | duckdb | 205 | 529 | measured | yes | 1.00 | 0.91 |
| h3024 | LOCAL | clustered_10mb_spatial_s3 | count_by | 0.02546 | warm | duckdb | duckdb-wasm | 92 | 59 | measured | NO | 1.55 | 0.84 |
| h3025 | LOCAL | clustered_10mb_shuffled_s3 | count | 0.0176 | warm | duckdb | duckdb-wasm | 84 | 51 | measured | NO | 1.64 | 0.92 |
| h3026 | LOCAL | clustered_30mb_spatial_s3 | features | 0.00086 | warm | duckdb | duckdb-wasm | 80 | 50 | measured | NO | 1.59 | 0.84 |
| h3027 | LOCAL | clustered_30mb_shuffled_s3 | features | 0.00428 | warm | duckdb | duckdb | 93 | 106 | measured | yes | 1.00 | 0.84 |
| h3028 | FAST_LAN | clustered_100mb_shuffled_s3 | count | 0.01117 | warm | duckdb | duckdb | 113 | 4793 | measured | yes | 1.00 | 0.92 |
| h3029 | FAST_LAN | uniform_10mb_spatial_s3 | count | 0.19127 | warm | duckdb | duckdb | 114 | 189 | measured | yes | 1.00 | 0.92 |
| h3030 | FAST_LAN | uniform_10mb_shuffled_s3 | features | 0.10996 | cold | duckdb | duckdb | 112 | 1188 | measured | yes | 1.00 | 0.83 |
| h3031 | FAST_LAN | uniform_30mb_spatial_s3 | count_by | 0.11201 | warm | duckdb | duckdb | 102 | 353 | measured | yes | 1.00 | 0.92 |
| h3032 | FAST_LAN | uniform_30mb_shuffled_s3 | count | 0.05609 | warm | duckdb | duckdb | 114 | 407 | measured | yes | 1.00 | 0.92 |
| h3033 | FAST_LAN | uniform_100mb_spatial_s3 | features | 0.25566 | warm | duckdb | duckdb | 142 | 740 | measured | yes | 1.00 | 0.91 |
| h3034 | FAST_LAN | uniform_100mb_shuffled_s3 | count_by | 0.01246 | warm | duckdb | duckdb | 121 | 5219 | measured | yes | 1.00 | 0.84 |
| h3035 | FAST_LAN | clustered_10mb_spatial_s3 | count_by | 0.07651 | cold | duckdb | duckdb | 90 | 1016 | measured | yes | 1.00 | 0.84 |
| h3036 | FAST_LAN | clustered_10mb_shuffled_s3 | count | 0.19912 | cold | duckdb | duckdb | 91 | 976 | measured | yes | 1.00 | 0.92 |
| h3037 | FAST_LAN | clustered_30mb_spatial_s3 | count_by | 0.17184 | cold | duckdb | duckdb | 104 | 1213 | measured | yes | 1.00 | 0.84 |
| h3038 | FAST_LAN | clustered_30mb_shuffled_s3 | features | 0.00343 | warm | duckdb | duckdb | 89 | 390 | measured | yes | 1.00 | 0.84 |
| h3039 | FAST_LAN | clustered_100mb_spatial_s3 | count_by | 0.01183 | warm | duckdb | duckdb | 105 | 192 | measured | yes | 1.00 | 0.84 |
| h3040 | FAST_LAN | clustered_100mb_shuffled_s3 | count_by | 0.38523 | warm | duckdb | duckdb | 115 | 4969 | measured | yes | 1.00 | 0.84 |
| h3041 | FAST_LAN | uniform_10mb_spatial_s3 | features | 0.00048 | warm | duckdb | duckdb | 95 | 177 | measured | yes | 1.00 | 0.92 |
| h3042 | GOOD_BROADBAND | uniform_100mb_spatial_s3 | features | 0.56746 | warm | duckdb | duckdb | 150 | 1991 | measured | yes | 1.00 | 0.91 |
| h3043 | GOOD_BROADBAND | uniform_100mb_shuffled_s3 | features | 0.08224 | warm | duckdb | duckdb | 1991 | 11496 | measured | yes | 1.00 | 0.90 |
| h3044 | GOOD_BROADBAND | clustered_10mb_spatial_s3 | features | 0.02194 | cold | duckdb | duckdb | 152 | 2761 | measured | yes | 1.00 | 0.91 |
| h3045 | GOOD_BROADBAND | clustered_10mb_shuffled_s3 | count_by | 0.00582 | warm | duckdb | duckdb | 135 | 575 | measured | yes | 1.00 | 0.84 |
| h3046 | GOOD_BROADBAND | clustered_30mb_spatial_s3 | features | 0.2071 | cold | duckdb | duckdb | 307 | 4189 | measured | yes | 1.00 | 0.90 |
| h3047 | GOOD_BROADBAND | clustered_30mb_shuffled_s3 | count_by | 0.00782 | warm | duckdb | duckdb | 178 | 1504 | measured | yes | 1.00 | 0.84 |
| h3048 | GOOD_BROADBAND | clustered_100mb_spatial_s3 | count | 0.00179 | cold | duckdb | duckdb | 148 | 3334 | measured | yes | 1.00 | 0.92 |
| h3049 | GOOD_BROADBAND | clustered_100mb_shuffled_s3 | count_by | 0.53355 | warm | duckdb | duckdb | 205 | 10811 | measured | yes | 1.00 | 0.84 |
| h3050 | GOOD_BROADBAND | uniform_10mb_spatial_s3 | features | 0.21258 | warm | duckdb | duckdb | 131 | 699 | measured | yes | 1.00 | 0.91 |
| h3051 | GOOD_BROADBAND | uniform_10mb_shuffled_s3 | features | 0.00129 | warm | duckdb | duckdb | 143 | 615 | measured | yes | 1.00 | 0.84 |
| h3052 | GOOD_BROADBAND | uniform_30mb_spatial_s3 | count_by | 0.00535 | warm | duckdb | duckdb | 98 | 1427 | measured | yes | 1.00 | 0.84 |
| h3053 | GOOD_BROADBAND | uniform_30mb_shuffled_s3 | count_by | 0.04349 | warm | duckdb | duckdb | 95 | 1454 | measured | yes | 1.00 | 0.84 |
| h3054 | GOOD_BROADBAND | uniform_100mb_spatial_s3 | features | 0.00106 | warm | duckdb | duckdb | 157 | 2061 | measured | yes | 1.00 | 0.91 |
| h3055 | GOOD_BROADBAND | uniform_100mb_shuffled_s3 | features | 0.20929 | warm | duckdb | duckdb | 1812 | 7110 | measured | yes | 1.00 | 0.90 |
| h3056 | GOOD_BROADBAND | clustered_10mb_spatial_s3 | count_by | 0.17487 | warm | duckdb | duckdb | 106 | 578 | measured | yes | 1.00 | 0.84 |
| h3057 | GOOD_BROADBAND | clustered_10mb_shuffled_s3 | count_by | 0.12678 | warm | duckdb | duckdb | 105 | 605 | measured | yes | 1.00 | 0.84 |
| h3058 | GOOD_BROADBAND | clustered_30mb_spatial_s3 | count_by | 0.00136 | cold | duckdb | duckdb | 154 | 3929 | measured | yes | 1.00 | 0.73 |
| h3059 | GOOD_BROADBAND | clustered_30mb_shuffled_s3 | features | 0.13482 | warm | duckdb | duckdb | 1831 | 8196 | measured | yes | 1.00 | 0.90 |
| h3060 | GOOD_BROADBAND | clustered_100mb_spatial_s3 | features | 0.00269 | warm | duckdb | duckdb | 144 | 1243 | measured | yes | 1.00 | 0.84 |
| h3061 | GOOD_BROADBAND | clustered_100mb_shuffled_s3 | count | 0.00684 | warm | duckdb | duckdb | 202 | 10345 | measured | yes | 1.00 | 0.92 |
| h3062 | MOBILE | clustered_30mb_shuffled_s3 | features | 0.17554 | cold | duckdb | duckdb | 127 | 18870 | measured | yes | 1.00 | 0.92 |
| h3063 | MOBILE | clustered_100mb_spatial_s3 | features | 0.01878 | warm | duckdb | duckdb | 257 | 3057 | measured | yes | 1.00 | 0.73 |
| h3064 | MOBILE | clustered_100mb_shuffled_s3 | features | 0.0273 | warm | duckdb | duckdb | 3267 | 39511 | measured | yes | 1.00 | 0.82 |
| h3065 | MOBILE | uniform_10mb_spatial_s3 | features | 0.00205 | warm | duckdb | duckdb | 125 | 2989 | measured | yes | 1.00 | 0.92 |
| h3066 | MOBILE | uniform_10mb_shuffled_s3 | count_by | 0.01111 | cold | duckdb | duckdb | 118 | 13336 | measured | yes | 1.00 | 0.84 |
| h3067 | MOBILE | uniform_30mb_spatial_s3 | features | 0.01268 | warm | duckdb | duckdb | 133 | 8467 | measured | yes | 1.00 | 0.92 |
| h3068 | MOBILE | uniform_30mb_shuffled_s3 | count_by | 0.02204 | warm | duckdb | duckdb | 135 | 8579 | measured | yes | 1.00 | 0.84 |
| h3069 | MOBILE | uniform_100mb_spatial_s3 | count_by | 0.00728 | warm | duckdb | duckdb | 110 | 3273 | measured | yes | 1.00 | 0.92 |
| h3070 | MOBILE | uniform_100mb_shuffled_s3 | features | 0.00068 | warm | duckdb | duckdb | 250 | 37782 | measured | yes | 1.00 | 0.84 |
| h3071 | MOBILE | clustered_10mb_spatial_s3 | features | 0.23329 | warm | duckdb | duckdb | 271 | 3085 | measured | yes | 1.00 | 0.91 |
| h3072 | MOBILE | clustered_10mb_shuffled_s3 | count_by | 0.00151 | warm | duckdb | duckdb | 115 | 3033 | measured | yes | 1.00 | 0.84 |
| h3073 | MOBILE | clustered_30mb_spatial_s3 | features | 0.01426 | warm | duckdb | duckdb | 160 | 8454 | measured | yes | 1.00 | 0.84 |
| h3074 | MOBILE | clustered_30mb_shuffled_s3 | count_by | 0.16456 | cold | duckdb | duckdb | 109 | 19131 | measured | yes | 1.00 | 0.84 |
| h3075 | MOBILE | clustered_100mb_spatial_s3 | count_by | 0.00363 | warm | duckdb | duckdb | 146 | 2164 | measured | yes | 1.00 | 0.84 |
| h3076 | MOBILE | clustered_100mb_shuffled_s3 | features | 0.02055 | cold | duckdb | duckdb | 142 | 49855 | measured | yes | 1.00 | 0.84 |
| h3077 | MOBILE | uniform_10mb_spatial_s3 | features | 0.01343 | cold | duckdb | duckdb | 136 | 13262 | measured | yes | 1.00 | 0.92 |
| h3078 | MOBILE | uniform_10mb_shuffled_s3 | features | 0.0042 | warm | duckdb | duckdb | 171 | 3068 | measured | yes | 1.00 | 0.84 |
| h3079 | MOBILE | uniform_30mb_spatial_s3 | count | 0.02954 | warm | duckdb | duckdb | 111 | 8420 | measured | yes | 1.00 | 0.92 |
| h3080 | MOBILE | uniform_30mb_shuffled_s3 | count_by | 0.0407 | cold | duckdb | duckdb | 117 | 19031 | measured | yes | 1.00 | 0.84 |
| h3081 | MOBILE | uniform_100mb_spatial_s3 | features | 0.67762 | warm | duckdb | duckdb | 142 | 1771 | measured | yes | 1.00 | 0.92 |
| h3082 | HIGH_LATENCY | uniform_30mb_spatial_s3 | count | 0.61429 | warm | duckdb | duckdb | 180 | 12579 | measured | yes | 1.00 | 0.92 |
| h3083 | HIGH_LATENCY | uniform_30mb_shuffled_s3 | count | 0.00808 | cold | duckdb | duckdb | 184 | 28049 | measured | yes | 1.00 | 0.92 |
| h3084 | HIGH_LATENCY | uniform_100mb_spatial_s3 | features | 0.33886 | warm | duckdb | duckdb | 1316 | 5978 | measured | yes | 1.00 | 0.90 |
| h3085 | HIGH_LATENCY | uniform_100mb_shuffled_s3 | count_by | 0.00304 | cold | duckdb | duckdb | 196 | 80245 | measured | yes | 1.00 | 0.84 |
| h3086 | HIGH_LATENCY | clustered_10mb_spatial_s3 | features | 0.03839 | warm | duckdb | duckdb | 415 | 4589 | measured | yes | 1.00 | 0.91 |
| h3087 | HIGH_LATENCY | clustered_10mb_shuffled_s3 | count_by | 0.05036 | warm | duckdb | duckdb | 186 | 4509 | measured | yes | 1.00 | 0.84 |
| h3088 | HIGH_LATENCY | uniform_1gb_shuffled | features | 0.00269 | warm | duckdb | duckdb | 10214 | None | skipped (tab limit) | yes | 1.00 | 0.82 |
| h3089 | HIGH_LATENCY | clustered_30mb_shuffled_s3 | count_by | 0.01775 | cold | duckdb | duckdb | 186 | 28341 | measured | yes | 1.00 | 0.84 |
| h3090 | HIGH_LATENCY | clustered_100mb_spatial_s3 | features | 0.00072 | warm | duckdb | duckdb | 177 | 6676 | measured | yes | 1.00 | 0.84 |
| h3091 | HIGH_LATENCY | clustered_100mb_shuffled_s3 | features | 0.00037 | warm | duckdb | duckdb | 432 | 57288 | measured | yes | 1.00 | 0.83 |
| h3092 | HIGH_LATENCY | uniform_10mb_spatial_s3 | count | 0.00042 | warm | duckdb | duckdb | 180 | 4423 | measured | yes | 1.00 | 0.92 |
| h3093 | HIGH_LATENCY | uniform_10mb_shuffled_s3 | count_by | 0.18984 | warm | duckdb | duckdb | 199 | 4491 | measured | yes | 1.00 | 0.84 |
| h3094 | HIGH_LATENCY | uniform_30mb_spatial_s3 | features | 0.04506 | warm | duckdb | duckdb | 397 | 12687 | measured | yes | 1.00 | 0.91 |
| h3095 | HIGH_LATENCY | uniform_30mb_shuffled_s3 | count_by | 0.15119 | warm | duckdb | duckdb | 185 | 12837 | measured | yes | 1.00 | 0.84 |
| h3096 | HIGH_LATENCY | uniform_100mb_spatial_s3 | count_by | 0.40898 | cold | duckdb | duckdb | 197 | 61938 | measured | yes | 1.00 | 0.84 |
| h3097 | HIGH_LATENCY | uniform_100mb_shuffled_s3 | features | 0.00749 | warm | duckdb | duckdb | 220 | 4229 | measured | yes | 1.00 | 0.92 |
| h3098 | HIGH_LATENCY | clustered_10mb_spatial_s3 | features | 0.0368 | warm | duckdb | duckdb | 181 | 4400 | measured | yes | 1.00 | 0.84 |
| h3099 | HIGH_LATENCY | clustered_10mb_shuffled_s3 | features | 0.13326 | cold | duckdb | duckdb | 230 | 19780 | measured | yes | 1.00 | 0.92 |

</details>

## Browser stability (held-out)

browser runs 99 · crashes 0 · engine errors 0 · skipped above the safety envelope 1 · digest mismatches 0 of 35 checked · row-count mismatches 0 · browser chosen and then failed: 0
