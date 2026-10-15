## Estimator error (held-out, n=100)

| what is estimated | median × | P90 × | P95 × | max × | over | under | under by > 5× | actual inside [low, high] |
|---|---|---|---|---|---|---|---|---|
| **cardinality** (rows matching window and filter; truth = server COUNT) | 1.01 | 1.13 | 1.33 | 2.0 | 51% | 36% | 0.0% | 100% |
| **output rows** (what the query returns: aggregate / LIMIT applied) | 1.0 | 1.13 | 1.33 | 2.0 | 20% | 7% | 0.0% | 89% |

Cardinality by layout class: `coh=high|skew=flat|cov=declared|hist=yes|filt=no`: median 1.0, P90 1.04, max 1.13 (n=15); `coh=high|skew=flat|cov=declared|hist=yes|filt=yes`: median 1.01, P90 1.42, max 1.42 (n=9); `coh=high|skew=peaked|cov=declared|hist=yes|filt=no`: median 1.02, P90 1.2, max 1.33 (n=17); `coh=high|skew=peaked|cov=declared|hist=yes|filt=yes`: median 1.02, P90 2.0, max 2.0 (n=8); `coh=low|skew=flat|cov=declared|hist=yes|filt=no`: median 1.0, P90 1.04, max 1.23 (n=26); `coh=low|skew=flat|cov=declared|hist=yes|filt=yes`: median 1.01, P90 1.13, max 1.62 (n=25)

Output rows by plan shape: count: median 1.0×, P90 1.0×, max 1.0× (n=23); count_by: median 1.09×, P90 2.0×, max 2.0× (n=26); features: median 1.0×, P90 1.08×, max 1.33× (n=51)

### Worst 12 output-row estimates

| case | dataset | query | window | limit | actual rows | estimated | interval | quality / basis | error × |
|---|---|---|---|---|---|---|---|---|---|
| h3061 | clustered_100mb_shuffled_s3 | count_by | 0.00239 |  | 0 | 1 | 1–1 | fair / class | 2.0 |
| h3098 | clustered_10mb_spatial_s3 | count_by | 0.01413 |  | 1 | 3 | 1–3 | fair / class | 2.0 |
| h3017 | clustered_100mb_shuffled_s3 | count_by | 0.00445 |  | 0 | 1 | 1–1 | fair / class | 2.0 |
| h3072 | clustered_10mb_shuffled_s3 | count_by | 0.00045 |  | 0 | 1 | 1–1 | fair / class | 2.0 |
| h3039 | clustered_100mb_spatial_s3 | features | 0.01028 | 8000 | 2 | 3 | 1–4 | fair / class | 1.3 |
| h3025 | clustered_10mb_shuffled_s3 | count_by | 0.00168 |  | 2 | 3 | 1–3 | fair / class | 1.3 |
| h3071 | clustered_10mb_spatial_s3 | features | 0.00359 | 300 | 9 | 11 | 6–16 | fair / class | 1.2 |
| h3091 | clustered_100mb_shuffled_s3 | features | 0.0021 | 8000 | 1252 | 1421 | 946–2131 | fair / class | 1.1 |
| h3036 | clustered_10mb_shuffled_s3 | features | 0.01859 | 1500 | 87 | 77 | 51–115 | fair / class | 1.1 |
| h3006 | uniform_10mb_spatial_s3 | features | 0.00042 | 300 | 61 | 54 | 35–81 | good / class | 1.1 |
| h3042 | uniform_100mb_spatial_s3 | count_by | 0.39897 |  | 7 | 8 | 8–8 | fair / class | 1.1 |
| h3043 | uniform_100mb_shuffled_s3 | count_by | 0.00714 |  | 7 | 8 | 8–8 | fair / class | 1.1 |

<details><summary>All 100 cases (output rows)</summary>

| case | dataset | query | window | limit | actual | est | error × |
|---|---|---|---|---|---|---|---|
| h3000 | clustered_10mb_spatial_s3 | features | 0.00142 | 1500 | 52 | 48 | 1.1 |
| h3001 | clustered_10mb_shuffled_s3 | features | 0.00166 | 300 | 1 | 1 | 1.0 |
| h3002 | clustered_30mb_spatial_s3 | count_by | 0.08736 |  | 7 | 8 | 1.1 |
| h3003 | clustered_30mb_shuffled_s3 | count | 0.00071 |  | 1 | 1 | 1.0 |
| h3004 | clustered_100mb_spatial_s3 | features | 0.02095 | 1500 | 1500 | 1500 | 1.0 |
| h3005 | clustered_100mb_shuffled_s3 | count_by | 0.17505 |  | 7 | 8 | 1.1 |
| h3006 | uniform_10mb_spatial_s3 | features | 0.00042 | 300 | 61 | 54 | 1.1 |
| h3007 | uniform_10mb_shuffled_s3 | features | 0.02391 | 1500 | 1500 | 1500 | 1.0 |
| h3008 | uniform_30mb_spatial_s3 | count_by | 0.0118 |  | 7 | 8 | 1.1 |
| h3009 | uniform_1gb_shuffled | features | 0.02535 | 40000 | 40000 | 40000 | 1.0 |
| h3010 | uniform_100mb_spatial_s3 | features | 0.00691 | 40000 | 8836 | 8882 | 1.0 |
| h3011 | uniform_100mb_shuffled_s3 | count | 0.11928 |  | 1 | 1 | 1.0 |
| h3012 | clustered_10mb_spatial_s3 | count_by | 0.03443 |  | 1 | 1 | 1.0 |
| h3013 | clustered_10mb_shuffled_s3 | count | 0.00665 |  | 1 | 1 | 1.0 |
| h3014 | clustered_30mb_spatial_s3 | count_by | 0.01144 |  | 50 | 50 | 1.0 |
| h3015 | clustered_30mb_shuffled_s3 | count_by | 0.02569 |  | 50 | 50 | 1.0 |
| h3016 | clustered_100mb_spatial_s3 | count | 0.11848 |  | 1 | 1 | 1.0 |
| h3017 | clustered_100mb_shuffled_s3 | count_by | 0.00445 |  | 0 | 1 | 2.0 |
| h3018 | uniform_10mb_spatial_s3 | count | 0.0011 |  | 1 | 1 | 1.0 |
| h3019 | uniform_10mb_shuffled_s3 | features | 0.07765 | 8000 | 8000 | 8000 | 1.0 |
| h3020 | uniform_30mb_spatial_s3 | count_by | 0.00583 |  | 7 | 8 | 1.1 |
| h3021 | uniform_30mb_shuffled_s3 | count | 0.02383 |  | 1 | 1 | 1.0 |
| h3022 | uniform_100mb_spatial_s3 | features | 0.00362 | 300 | 300 | 300 | 1.0 |
| h3023 | uniform_100mb_shuffled_s3 | count | 0.0008 |  | 1 | 1 | 1.0 |
| h3024 | clustered_10mb_spatial_s3 | count_by | 0.00246 |  | 8 | 8 | 1.0 |
| h3025 | clustered_10mb_shuffled_s3 | count_by | 0.00168 |  | 2 | 3 | 1.3 |
| h3026 | clustered_30mb_spatial_s3 | features | 0.07432 | 300 | 300 | 300 | 1.0 |
| h3027 | clustered_30mb_shuffled_s3 | features | 0.57823 | 8000 | 8000 | 8000 | 1.0 |
| h3028 | clustered_100mb_shuffled_s3 | features | 0.4436 | 8000 | 8000 | 8000 | 1.0 |
| h3029 | uniform_10mb_spatial_s3 | features | 0.12555 | 8000 | 8000 | 8000 | 1.0 |
| h3030 | uniform_10mb_shuffled_s3 | features | 0.00951 | 1500 | 175 | 171 | 1.0 |
| h3031 | uniform_30mb_spatial_s3 | count | 0.00071 |  | 1 | 1 | 1.0 |
| h3032 | uniform_30mb_shuffled_s3 | features | 0.50839 | 1500 | 1500 | 1500 | 1.0 |
| h3033 | uniform_100mb_spatial_s3 | features | 0.05099 | 40000 | 40000 | 40000 | 1.0 |
| h3034 | uniform_100mb_shuffled_s3 | count | 0.00283 |  | 1 | 1 | 1.0 |
| h3035 | clustered_10mb_spatial_s3 | count | 0.20977 |  | 1 | 1 | 1.0 |
| h3036 | clustered_10mb_shuffled_s3 | features | 0.01859 | 1500 | 87 | 77 | 1.1 |
| h3037 | clustered_30mb_spatial_s3 | count_by | 0.00035 |  | 41 | 43 | 1.0 |
| h3038 | clustered_30mb_shuffled_s3 | count | 0.00037 |  | 1 | 1 | 1.0 |
| h3039 | clustered_100mb_spatial_s3 | features | 0.01028 | 8000 | 2 | 3 | 1.3 |
| h3040 | clustered_100mb_shuffled_s3 | count_by | 0.00994 |  | 8 | 8 | 1.0 |
| h3041 | uniform_10mb_spatial_s3 | count_by | 0.00062 |  | 8 | 8 | 1.0 |
| h3042 | uniform_100mb_spatial_s3 | count_by | 0.39897 |  | 7 | 8 | 1.1 |
| h3043 | uniform_100mb_shuffled_s3 | count_by | 0.00714 |  | 7 | 8 | 1.1 |
| h3044 | clustered_10mb_spatial_s3 | features | 0.03303 | 40000 | 1233 | 1248 | 1.0 |
| h3045 | clustered_10mb_shuffled_s3 | features | 0.03412 | 1500 | 0 | 0 | 1.0 |
| h3046 | clustered_30mb_spatial_s3 | count_by | 0.21397 |  | 50 | 50 | 1.0 |
| h3047 | clustered_30mb_shuffled_s3 | features | 0.28961 | 8000 | 8000 | 8000 | 1.0 |
| h3048 | clustered_100mb_spatial_s3 | features | 0.018 | 1500 | 1500 | 1500 | 1.0 |
| h3049 | clustered_1gb_shuffled | count_by | 0.16246 |  | 50 | 50 | 1.0 |
| h3050 | uniform_10mb_spatial_s3 | count | 0.00364 |  | 1 | 1 | 1.0 |
| h3051 | uniform_10mb_shuffled_s3 | count | 0.00053 |  | 1 | 1 | 1.0 |
| h3052 | uniform_30mb_spatial_s3 | count_by | 0.05439 |  | 50 | 50 | 1.0 |
| h3053 | uniform_30mb_shuffled_s3 | features | 0.31987 | 8000 | 8000 | 8000 | 1.0 |
| h3054 | uniform_100mb_spatial_s3 | count | 0.02808 |  | 1 | 1 | 1.0 |
| h3055 | uniform_100mb_shuffled_s3 | features | 0.08275 | 8000 | 8000 | 8000 | 1.0 |
| h3056 | clustered_10mb_spatial_s3 | features | 0.00107 | 300 | 300 | 300 | 1.0 |
| h3057 | clustered_1gb_shuffled | features | 0.26143 | 1500 | 1500 | 1500 | 1.0 |
| h3058 | clustered_30mb_spatial_s3 | features | 0.04454 | 1500 | 1500 | 1500 | 1.0 |
| h3059 | clustered_30mb_shuffled_s3 | features | 0.62891 | 300 | 300 | 300 | 1.0 |
| h3060 | clustered_100mb_spatial_s3 | features | 0.00359 | 1500 | 1500 | 1500 | 1.0 |
| h3061 | clustered_100mb_shuffled_s3 | count_by | 0.00239 |  | 0 | 1 | 2.0 |
| h3062 | clustered_30mb_shuffled_s3 | count | 0.05572 |  | 1 | 1 | 1.0 |
| h3063 | clustered_100mb_spatial_s3 | count | 0.03213 |  | 1 | 1 | 1.0 |
| h3064 | clustered_100mb_shuffled_s3 | features | 0.0014 | 1500 | 1500 | 1500 | 1.0 |
| h3065 | uniform_10mb_spatial_s3 | features | 0.18299 | 1500 | 1500 | 1500 | 1.0 |
| h3066 | uniform_10mb_shuffled_s3 | features | 0.00137 | 1500 | 198 | 198 | 1.0 |
| h3067 | uniform_30mb_spatial_s3 | features | 0.56695 | 300 | 300 | 300 | 1.0 |
| h3068 | uniform_30mb_shuffled_s3 | features | 0.00215 | 40000 | 833 | 828 | 1.0 |
| h3069 | uniform_100mb_spatial_s3 | count | 0.03692 |  | 1 | 1 | 1.0 |
| h3070 | uniform_100mb_shuffled_s3 | count | 0.00583 |  | 1 | 1 | 1.0 |
| h3071 | clustered_10mb_spatial_s3 | features | 0.00359 | 300 | 9 | 11 | 1.2 |
| h3072 | clustered_10mb_shuffled_s3 | count_by | 0.00045 |  | 0 | 1 | 2.0 |
| h3073 | clustered_30mb_spatial_s3 | count | 0.00785 |  | 1 | 1 | 1.0 |
| h3074 | clustered_30mb_shuffled_s3 | count_by | 0.05808 |  | 7 | 8 | 1.1 |
| h3075 | clustered_100mb_spatial_s3 | features | 0.12057 | 300 | 300 | 300 | 1.0 |
| h3076 | clustered_100mb_shuffled_s3 | count | 0.00382 |  | 1 | 1 | 1.0 |
| h3077 | uniform_10mb_spatial_s3 | features | 0.07864 | 1500 | 1500 | 1500 | 1.0 |
| h3078 | uniform_10mb_shuffled_s3 | count | 0.58936 |  | 1 | 1 | 1.0 |
| h3079 | uniform_30mb_spatial_s3 | features | 0.0693 | 8000 | 8000 | 8000 | 1.0 |
| h3080 | uniform_30mb_shuffled_s3 | count_by | 0.00055 |  | 8 | 8 | 1.0 |
| h3081 | uniform_100mb_spatial_s3 | features | 0.03683 | 300 | 300 | 300 | 1.0 |
| h3082 | uniform_1gb_shuffled | count | 0.0005 |  | 1 | 1 | 1.0 |
| h3083 | uniform_30mb_shuffled_s3 | count | 0.08911 |  | 1 | 1 | 1.0 |
| h3084 | uniform_100mb_spatial_s3 | features | 0.00655 | 8000 | 8000 | 8000 | 1.0 |
| h3085 | uniform_100mb_shuffled_s3 | features | 0.00648 | 300 | 300 | 300 | 1.0 |
| h3086 | clustered_10mb_spatial_s3 | features | 0.13199 | 1500 | 565 | 575 | 1.0 |
| h3087 | clustered_10mb_shuffled_s3 | features | 0.00252 | 1500 | 0 | 0 | 1.0 |
| h3088 | clustered_30mb_spatial_s3 | features | 0.09463 | 1500 | 1500 | 1500 | 1.0 |
| h3089 | clustered_30mb_shuffled_s3 | count_by | 0.54501 |  | 50 | 50 | 1.0 |
| h3090 | clustered_100mb_spatial_s3 | features | 0.20122 | 1500 | 1500 | 1500 | 1.0 |
| h3091 | clustered_100mb_shuffled_s3 | features | 0.0021 | 8000 | 1252 | 1421 | 1.1 |
| h3092 | uniform_10mb_spatial_s3 | count_by | 0.01946 |  | 7 | 8 | 1.1 |
| h3093 | uniform_10mb_shuffled_s3 | features | 0.00078 | 1500 | 106 | 102 | 1.0 |
| h3094 | uniform_30mb_spatial_s3 | features | 0.00139 | 8000 | 531 | 522 | 1.0 |
| h3095 | uniform_30mb_shuffled_s3 | features | 0.04048 | 8000 | 8000 | 8000 | 1.0 |
| h3096 | uniform_100mb_spatial_s3 | features | 0.00194 | 1500 | 1500 | 1500 | 1.0 |
| h3097 | uniform_100mb_shuffled_s3 | count_by | 0.06112 |  | 50 | 50 | 1.0 |
| h3098 | clustered_10mb_spatial_s3 | count_by | 0.01413 |  | 1 | 3 | 2.0 |
| h3099 | clustered_10mb_shuffled_s3 | features | 0.4432 | 1500 | 1500 | 1500 | 1.0 |

</details>

<details><summary>All cardinality estimates</summary>

| case | dataset | window | filter | actual | est | interval | error × |
|---|---|---|---|---|---|---|---|
| h3000 | clustered_10mb_spatial_s3 | 0.00142 |  | 52 | 48 | 31–72 | 1.1 |
| h3001 | clustered_10mb_shuffled_s3 | 0.00166 |  | 1 | 1 | 0–1 | 1.0 |
| h3002 | clustered_30mb_spatial_s3 | 0.08736 | {"column": "category", "op": "ne", "value": "other"} | 28554 | 28931 | 19287–43397 | 1.0 |
| h3003 | clustered_30mb_shuffled_s3 | 0.00071 | {"column": "height", "op": "gt", "value": 20} | 9 | 10 | 5–16 | 1.1 |
| h3004 | clustered_100mb_spatial_s3 | 0.02095 |  | 9782 | 11040 | 7359–16559 | 1.1 |
| h3005 | clustered_100mb_shuffled_s3 | 0.17505 | {"column": "category", "op": "ne", "value": "other"} | 327774 | 328086 | 218723–492128 | 1.0 |
| h3006 | uniform_10mb_spatial_s3 | 0.00042 |  | 61 | 54 | 35–81 | 1.1 |
| h3007 | uniform_10mb_shuffled_s3 | 0.02391 |  | 3169 | 3179 | 2118–4768 | 1.0 |
| h3008 | uniform_30mb_spatial_s3 | 0.0118 | {"column": "category", "op": "ne", "value": "other"} | 3948 | 3943 | 2628–5914 | 1.0 |
| h3009 | uniform_1gb_shuffled | 0.02535 |  | 330094 | 329662 | 219774–494492 | 1.0 |
| h3010 | uniform_100mb_spatial_s3 | 0.00691 |  | 8836 | 8882 | 5921–13323 | 1.0 |
| h3011 | uniform_100mb_shuffled_s3 | 0.11928 | {"column": "height", "op": "gt", "value": 20} | 23826 | 23955 | 15969–35932 | 1.0 |
| h3012 | clustered_10mb_spatial_s3 | 0.03443 | {"column": "category", "op": "ne", "value": "other"} | 1 | 1 | 0–2 | 1.0 |
| h3013 | clustered_10mb_shuffled_s3 | 0.00665 | {"column": "height", "op": "gt", "value": 20} | 0 | 0 | 0–0 | 1.0 |
| h3014 | clustered_30mb_spatial_s3 | 0.01144 |  | 2874 | 2953 | 1968–4430 | 1.0 |
| h3015 | clustered_30mb_shuffled_s3 | 0.02569 |  | 1314 | 1327 | 884–1990 | 1.0 |
| h3016 | clustered_100mb_spatial_s3 | 0.11848 |  | 487612 | 486965 | 324643–730447 | 1.0 |
| h3017 | clustered_100mb_shuffled_s3 | 0.00445 | {"column": "category", "op": "ne", "value": "other"} | 0 | 0 | 0–0 | 1.0 |
| h3018 | uniform_10mb_spatial_s3 | 0.0011 | {"column": "height", "op": "gt", "value": 20} | 22 | 20 | 13–30 | 1.1 |
| h3019 | uniform_10mb_shuffled_s3 | 0.07765 |  | 10181 | 10210 | 6806–15315 | 1.0 |
| h3020 | uniform_30mb_spatial_s3 | 0.00583 | {"column": "category", "op": "ne", "value": "other"} | 1901 | 1921 | 1280–2882 | 1.0 |
| h3021 | uniform_30mb_shuffled_s3 | 0.02383 | {"column": "height", "op": "gt", "value": 20} | 1339 | 1409 | 939–2114 | 1.1 |
| h3022 | uniform_100mb_spatial_s3 | 0.00362 |  | 4738 | 4727 | 3151–7091 | 1.0 |
| h3023 | uniform_100mb_shuffled_s3 | 0.0008 | {"column": "height", "op": "gt", "value": 20} | 163 | 152 | 101–228 | 1.1 |
| h3024 | clustered_10mb_spatial_s3 | 0.00246 |  | 43 | 49 | 32–74 | 1.1 |
| h3025 | clustered_10mb_shuffled_s3 | 0.00168 | {"column": "category", "op": "ne", "value": "other"} | 2 | 3 | 1–5 | 1.3 |
| h3026 | clustered_30mb_spatial_s3 | 0.07432 |  | 15357 | 15373 | 10248–23060 | 1.0 |
| h3027 | clustered_30mb_shuffled_s3 | 0.57823 |  | 293970 | 294159 | 196105–390000 | 1.0 |
| h3028 | clustered_100mb_shuffled_s3 | 0.4436 |  | 942959 | 943375 | 628916–1300000 | 1.0 |
| h3029 | uniform_10mb_spatial_s3 | 0.12555 |  | 16401 | 16423 | 10948–24634 | 1.0 |
| h3030 | uniform_10mb_shuffled_s3 | 0.00951 | {"column": "category", "op": "eq", "value": "school"} | 175 | 171 | 113–256 | 1.0 |
| h3031 | uniform_30mb_spatial_s3 | 0.00071 | {"column": "height", "op": "gt", "value": 20} | 63 | 44 | 28–66 | 1.4 |
| h3032 | uniform_30mb_shuffled_s3 | 0.50839 | {"column": "category", "op": "eq", "value": "school"} | 24734 | 24828 | 16551–37242 | 1.0 |
| h3033 | uniform_100mb_spatial_s3 | 0.05099 |  | 66511 | 66509 | 44338–99763 | 1.0 |
| h3034 | uniform_100mb_shuffled_s3 | 0.00283 | {"column": "height", "op": "gt", "value": 20} | 550 | 540 | 359–810 | 1.0 |
| h3035 | clustered_10mb_spatial_s3 | 0.20977 | {"column": "height", "op": "gt", "value": 20} | 1447 | 1488 | 991–2231 | 1.0 |
| h3036 | clustered_10mb_shuffled_s3 | 0.01859 | {"column": "category", "op": "eq", "value": "school"} | 87 | 77 | 51–115 | 1.1 |
| h3037 | clustered_30mb_spatial_s3 | 0.00035 |  | 42 | 43 | 28–65 | 1.0 |
| h3038 | clustered_30mb_shuffled_s3 | 0.00037 | {"column": "height", "op": "gt", "value": 20} | 2 | 2 | 0–3 | 1.0 |
| h3039 | clustered_100mb_spatial_s3 | 0.01028 |  | 2 | 3 | 1–4 | 1.3 |
| h3040 | clustered_100mb_shuffled_s3 | 0.00994 |  | 145 | 178 | 118–267 | 1.2 |
| h3041 | uniform_10mb_spatial_s3 | 0.00062 |  | 74 | 71 | 47–107 | 1.0 |
| h3042 | uniform_100mb_spatial_s3 | 0.39897 | {"column": "category", "op": "ne", "value": "other"} | 454177 | 454095 | 302729–681142 | 1.0 |
| h3043 | uniform_100mb_shuffled_s3 | 0.00714 | {"column": "category", "op": "ne", "value": "other"} | 8269 | 8208 | 5471–12311 | 1.0 |
| h3044 | clustered_10mb_spatial_s3 | 0.03303 |  | 1233 | 1248 | 831–1872 | 1.0 |
| h3045 | clustered_10mb_shuffled_s3 | 0.03412 | {"column": "category", "op": "eq", "value": "school"} | 0 | 0 | 0–0 | 1.0 |
| h3046 | clustered_30mb_spatial_s3 | 0.21397 |  | 76827 | 76967 | 51311–100000 | 1.0 |
| h3047 | clustered_30mb_shuffled_s3 | 0.28961 |  | 91891 | 91805 | 61202–137707 | 1.0 |
| h3048 | clustered_100mb_spatial_s3 | 0.018 | {"column": "category", "op": "eq", "value": "school"} | 11195 | 11226 | 7483–16839 | 1.0 |
| h3049 | clustered_1gb_shuffled | 0.16246 |  | 1208781 | 1211646 | 807763–1817469 | 1.0 |
| h3050 | uniform_10mb_spatial_s3 | 0.00364 |  | 474 | 474 | 315–711 | 1.0 |
| h3051 | uniform_10mb_shuffled_s3 | 0.00053 | {"column": "height", "op": "gt", "value": 20} | 7 | 12 | 6–20 | 1.6 |
| h3052 | uniform_30mb_spatial_s3 | 0.05439 |  | 21374 | 21374 | 14249–32061 | 1.0 |
| h3053 | uniform_30mb_shuffled_s3 | 0.31987 |  | 125017 | 124983 | 83321–187474 | 1.0 |
| h3054 | uniform_100mb_spatial_s3 | 0.02808 | {"column": "height", "op": "gt", "value": 20} | 5789 | 5666 | 3776–8498 | 1.0 |
| h3055 | uniform_100mb_shuffled_s3 | 0.08275 |  | 107694 | 107688 | 71791–161532 | 1.0 |
| h3056 | clustered_10mb_spatial_s3 | 0.00107 |  | 560 | 542 | 361–813 | 1.0 |
| h3057 | clustered_1gb_shuffled | 0.26143 |  | 3249165 | 3248938 | 2165958–4873407 | 1.0 |
| h3058 | clustered_30mb_spatial_s3 | 0.04454 |  | 28152 | 28116 | 18743–42174 | 1.0 |
| h3059 | clustered_30mb_shuffled_s3 | 0.62891 |  | 254688 | 254985 | 169989–382477 | 1.0 |
| h3060 | clustered_100mb_spatial_s3 | 0.00359 |  | 3287 | 3392 | 2261–5088 | 1.0 |
| h3061 | clustered_100mb_shuffled_s3 | 0.00239 | {"column": "category", "op": "ne", "value": "other"} | 0 | 0 | 0–0 | 1.0 |
| h3062 | clustered_30mb_shuffled_s3 | 0.05572 | {"column": "height", "op": "gt", "value": 20} | 59 | 57 | 37–85 | 1.0 |
| h3063 | clustered_100mb_spatial_s3 | 0.03213 | {"column": "height", "op": "gt", "value": 20} | 8484 | 8158 | 5438–12237 | 1.0 |
| h3064 | clustered_100mb_shuffled_s3 | 0.0014 | {"column": "category", "op": "eq", "value": "school"} | 2507 | 2536 | 1690–3804 | 1.0 |
| h3065 | uniform_10mb_spatial_s3 | 0.18299 | {"column": "category", "op": "eq", "value": "school"} | 3024 | 3045 | 2029–4567 | 1.0 |
| h3066 | uniform_10mb_shuffled_s3 | 0.00137 |  | 198 | 198 | 131–297 | 1.0 |
| h3067 | uniform_30mb_spatial_s3 | 0.56695 |  | 221515 | 221551 | 147700–332327 | 1.0 |
| h3068 | uniform_30mb_shuffled_s3 | 0.00215 |  | 833 | 828 | 551–1241 | 1.0 |
| h3069 | uniform_100mb_spatial_s3 | 0.03692 | {"column": "height", "op": "gt", "value": 20} | 7494 | 7426 | 4950–11139 | 1.0 |
| h3070 | uniform_100mb_shuffled_s3 | 0.00583 |  | 7502 | 7542 | 5027–11313 | 1.0 |
| h3071 | clustered_10mb_spatial_s3 | 0.00359 |  | 9 | 11 | 6–16 | 1.2 |
| h3072 | clustered_10mb_shuffled_s3 | 0.00045 |  | 0 | 0 | 0–0 | 1.0 |
| h3073 | clustered_30mb_spatial_s3 | 0.00785 | {"column": "height", "op": "gt", "value": 20} | 22 | 16 | 10–24 | 1.4 |
| h3074 | clustered_30mb_shuffled_s3 | 0.05808 | {"column": "category", "op": "ne", "value": "other"} | 7512 | 7525 | 5016–11288 | 1.0 |
| h3075 | clustered_100mb_spatial_s3 | 0.12057 |  | 27937 | 28040 | 18693–42060 | 1.0 |
| h3076 | clustered_100mb_shuffled_s3 | 0.00382 | {"column": "height", "op": "gt", "value": 20} | 90 | 90 | 52–153 | 1.0 |
| h3077 | uniform_10mb_spatial_s3 | 0.07864 |  | 10180 | 10170 | 6779–15255 | 1.0 |
| h3078 | uniform_10mb_shuffled_s3 | 0.58936 | {"column": "height", "op": "gt", "value": 20} | 11851 | 11946 | 7963–17919 | 1.0 |
| h3079 | uniform_30mb_spatial_s3 | 0.0693 |  | 26991 | 26994 | 17995–40491 | 1.0 |
| h3080 | uniform_30mb_shuffled_s3 | 0.00055 |  | 192 | 196 | 130–294 | 1.0 |
| h3081 | uniform_100mb_spatial_s3 | 0.03683 |  | 48143 | 48165 | 32109–72247 | 1.0 |
| h3082 | uniform_1gb_shuffled | 0.0005 | {"column": "height", "op": "gt", "value": 20} | 1089 | 1019 | 600–1731 | 1.1 |
| h3083 | uniform_30mb_shuffled_s3 | 0.08911 | {"column": "height", "op": "gt", "value": 20} | 5410 | 5350 | 3566–8024 | 1.0 |
| h3084 | uniform_100mb_spatial_s3 | 0.00655 |  | 8598 | 8633 | 5755–12950 | 1.0 |
| h3085 | uniform_100mb_shuffled_s3 | 0.00648 |  | 8426 | 8428 | 5618–12642 | 1.0 |
| h3086 | clustered_10mb_spatial_s3 | 0.13199 | {"column": "category", "op": "eq", "value": "school"} | 565 | 575 | 383–863 | 1.0 |
| h3087 | clustered_10mb_shuffled_s3 | 0.00252 |  | 0 | 0 | 0–0 | 1.0 |
| h3088 | clustered_30mb_spatial_s3 | 0.09463 |  | 29106 | 29180 | 19452–43769 | 1.0 |
| h3089 | clustered_30mb_shuffled_s3 | 0.54501 |  | 290073 | 290246 | 193496–390000 | 1.0 |
| h3090 | clustered_100mb_spatial_s3 | 0.20122 |  | 131514 | 131909 | 87939–197864 | 1.0 |
| h3091 | clustered_100mb_shuffled_s3 | 0.0021 |  | 1252 | 1421 | 946–2131 | 1.1 |
| h3092 | uniform_10mb_spatial_s3 | 0.01946 | {"column": "category", "op": "ne", "value": "other"} | 2148 | 2135 | 1423–3203 | 1.0 |
| h3093 | uniform_10mb_shuffled_s3 | 0.00078 |  | 106 | 102 | 67–153 | 1.0 |
| h3094 | uniform_30mb_spatial_s3 | 0.00139 |  | 531 | 522 | 347–783 | 1.0 |
| h3095 | uniform_30mb_shuffled_s3 | 0.04048 |  | 15796 | 15769 | 10512–23653 | 1.0 |
| h3096 | uniform_100mb_spatial_s3 | 0.00194 |  | 2441 | 2459 | 1638–3688 | 1.0 |
| h3097 | uniform_100mb_shuffled_s3 | 0.06112 |  | 79610 | 79629 | 53085–119443 | 1.0 |
| h3098 | clustered_10mb_spatial_s3 | 0.01413 | {"column": "category", "op": "ne", "value": "other"} | 1 | 3 | 1–5 | 2.0 |
| h3099 | clustered_10mb_shuffled_s3 | 0.4432 | {"column": "category", "op": "eq", "value": "school"} | 12487 | 12422 | 8280–18632 | 1.0 |

</details>

## Time model accuracy (planner score vs measured end-to-end time, Chrome-throttled network)

| engine | n | median × | P90 × | P95 × | max × | over-estimates | under-estimates |
|---|---|---|---|---|---|---|---|
| duckdb | 100 | 1.11 | 1.5 | 1.63 | 2.29 | 79% | 21% |
| duckdb-wasm | 98 | 1.37 | 2.68 | 3.53 | 5.06 | 55% | 45% |

By network profile (median × / P90 ×): FAST_LAN: server 1.09/1.16, browser 1.38/1.59; GOOD_BROADBAND: server 1.09/1.6, browser 1.22/1.88; HIGH_LATENCY: server 1.16/1.4, browser 1.36/3.43; LOCAL: server 1.07/1.22, browser 1.71/3.53; MOBILE: server 1.44/1.65, browser 1.06/2.17

## Planner accuracy and regret (held-out, 100 cases)

accuracy **97.0%** · regret median 1.0 · **P90 1.0** · P95 1.0 · max 1.28 · browser fastest in 10 cases, chosen in 7

Baselines: always-server accuracy 90.0%, P90 regret 1.04; always-browser (where measurable) median regret 10.976×, P90 118.565×.

| network | cases | accuracy | browser fastest |
|---|---|---|---|
| FAST_LAN | 14 | 100% | 0 |
| GOOD_BROADBAND | 20 | 100% | 0 |
| HIGH_LATENCY | 18 | 100% | 0 |
| LOCAL | 28 | 89% | 10 |
| MOBILE | 20 | 100% | 0 |

| browser state | cases | accuracy |
|---|---|---|
| warm | 72 | 96% |
| cold | 28 | 100% |

| confidence | cases | accuracy | mean confidence |
|---|---|---|---|
| 0.00-0.50 | 0 | 0% | None |
| 0.50-0.65 | 0 | 0% | None |
| 0.65-0.80 | 10 | 80% | 0.75 |
| 0.80-1.00 | 90 | 99% | 0.9 |

### Cases where the browser was measured faster, or the planner was wrong, or the browser was chosen

| case | network | dataset | query | window | state | selected | fastest | server ms | browser ms | correct | regret | confidence |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| h3000 | LOCAL | clustered_10mb_spatial_s3 | features | 0.00142 | warm | duckdb-wasm | duckdb-wasm | 106 | 66 | yes | 1.00 | 0.72 |
| h3012 | LOCAL | clustered_10mb_spatial_s3 | count_by | 0.03443 | warm | duckdb-wasm | duckdb-wasm | 77 | 43 | yes | 1.00 | 0.75 |
| h3024 | LOCAL | clustered_10mb_spatial_s3 | count_by | 0.00246 | warm | duckdb-wasm | duckdb-wasm | 75 | 40 | yes | 1.00 | 0.74 |
| h3001 | LOCAL | clustered_10mb_shuffled_s3 | features | 0.00166 | warm | duckdb-wasm | duckdb-wasm | 87 | 58 | yes | 1.00 | 0.71 |
| h3013 | LOCAL | clustered_10mb_shuffled_s3 | count | 0.00665 | warm | duckdb-wasm | duckdb-wasm | 80 | 46 | yes | 1.00 | 0.78 |
| h3025 | LOCAL | clustered_10mb_shuffled_s3 | count_by | 0.00168 | warm | duckdb-wasm | duckdb-wasm | 99 | 61 | yes | 1.00 | 0.71 |
| h3002 | LOCAL | clustered_30mb_spatial_s3 | count_by | 0.08736 | warm | duckdb | duckdb-wasm | 81 | 69 | **no** | 1.17 | 0.74 |
| h3018 | LOCAL | uniform_10mb_spatial_s3 | count | 0.0011 | warm | duckdb-wasm | duckdb-wasm | 78 | 36 | yes | 1.00 | 0.82 |
| h3008 | LOCAL | uniform_30mb_spatial_s3 | count_by | 0.0118 | warm | duckdb | duckdb-wasm | 78 | 61 | **no** | 1.28 | 0.81 |
| h3022 | LOCAL | uniform_100mb_spatial_s3 | features | 0.00362 | warm | duckdb | duckdb-wasm | 83 | 80 | **no** | 1.04 | 0.71 |

<details><summary>All 100 cases</summary>

| case | network | dataset | query | window | state | selected | fastest | server ms | browser ms | browser status | correct | regret | confidence |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| h3000 | LOCAL | clustered_10mb_spatial_s3 | features | 0.00142 | warm | duckdb-wasm | duckdb-wasm | 106 | 66 | measured | yes | 1.00 | 0.72 |
| h3001 | LOCAL | clustered_10mb_shuffled_s3 | features | 0.00166 | warm | duckdb-wasm | duckdb-wasm | 87 | 58 | measured | yes | 1.00 | 0.71 |
| h3002 | LOCAL | clustered_30mb_spatial_s3 | count_by | 0.08736 | warm | duckdb | duckdb-wasm | 81 | 69 | measured | NO | 1.17 | 0.74 |
| h3003 | LOCAL | clustered_30mb_shuffled_s3 | count | 0.00071 | warm | duckdb | duckdb | 84 | 98 | measured | yes | 1.00 | 0.84 |
| h3004 | LOCAL | clustered_100mb_spatial_s3 | features | 0.02095 | warm | duckdb | duckdb | 102 | 190 | measured | yes | 1.00 | 0.95 |
| h3005 | LOCAL | clustered_100mb_shuffled_s3 | count_by | 0.17505 | cold | duckdb | duckdb | 98 | 1082 | measured | yes | 1.00 | 0.87 |
| h3006 | LOCAL | uniform_10mb_spatial_s3 | features | 0.00042 | cold | duckdb | duckdb | 79 | 563 | measured | yes | 1.00 | 0.95 |
| h3007 | LOCAL | uniform_10mb_shuffled_s3 | features | 0.02391 | cold | duckdb | duckdb | 124 | 752 | measured | yes | 1.00 | 0.95 |
| h3008 | LOCAL | uniform_30mb_spatial_s3 | count_by | 0.0118 | warm | duckdb | duckdb-wasm | 78 | 61 | measured | NO | 1.28 | 0.81 |
| h3009 | LOCAL | uniform_1gb_shuffled | features | 0.02535 | cold | duckdb | duckdb | 970 | 5196 | measured | yes | 1.00 | 0.95 |
| h3010 | LOCAL | uniform_100mb_spatial_s3 | features | 0.00691 | warm | duckdb | duckdb | 191 | 864 | measured | yes | 1.00 | 0.95 |
| h3011 | LOCAL | uniform_100mb_shuffled_s3 | count | 0.11928 | warm | duckdb | duckdb | 104 | 514 | measured | yes | 1.00 | 0.95 |
| h3012 | LOCAL | clustered_10mb_spatial_s3 | count_by | 0.03443 | warm | duckdb-wasm | duckdb-wasm | 77 | 43 | measured | yes | 1.00 | 0.75 |
| h3013 | LOCAL | clustered_10mb_shuffled_s3 | count | 0.00665 | warm | duckdb-wasm | duckdb-wasm | 80 | 46 | measured | yes | 1.00 | 0.78 |
| h3014 | LOCAL | clustered_30mb_spatial_s3 | count_by | 0.01144 | cold | duckdb | duckdb | 77 | 570 | measured | yes | 1.00 | 0.87 |
| h3015 | LOCAL | clustered_30mb_shuffled_s3 | count_by | 0.02569 | warm | duckdb | duckdb | 82 | 115 | measured | yes | 1.00 | 0.78 |
| h3016 | LOCAL | clustered_100mb_spatial_s3 | count | 0.11848 | warm | duckdb | duckdb | 89 | 430 | measured | yes | 1.00 | 0.95 |
| h3017 | LOCAL | clustered_100mb_shuffled_s3 | count_by | 0.00445 | cold | duckdb | duckdb | 99 | 979 | measured | yes | 1.00 | 0.87 |
| h3018 | LOCAL | uniform_10mb_spatial_s3 | count | 0.0011 | warm | duckdb-wasm | duckdb-wasm | 78 | 36 | measured | yes | 1.00 | 0.82 |
| h3019 | LOCAL | uniform_10mb_shuffled_s3 | features | 0.07765 | warm | duckdb | duckdb | 189 | 776 | measured | yes | 1.00 | 0.87 |
| h3020 | LOCAL | uniform_30mb_spatial_s3 | count_by | 0.00583 | cold | duckdb | duckdb | 80 | 660 | measured | yes | 1.00 | 0.95 |
| h3021 | LOCAL | uniform_30mb_shuffled_s3 | count | 0.02383 | cold | duckdb | duckdb | 85 | 626 | measured | yes | 1.00 | 0.95 |
| h3022 | LOCAL | uniform_100mb_spatial_s3 | features | 0.00362 | warm | duckdb | duckdb-wasm | 83 | 80 | measured | NO | 1.04 | 0.71 |
| h3023 | LOCAL | uniform_100mb_shuffled_s3 | count | 0.0008 | cold | duckdb | duckdb | 96 | 946 | measured | yes | 1.00 | 0.95 |
| h3024 | LOCAL | clustered_10mb_spatial_s3 | count_by | 0.00246 | warm | duckdb-wasm | duckdb-wasm | 75 | 40 | measured | yes | 1.00 | 0.74 |
| h3025 | LOCAL | clustered_10mb_shuffled_s3 | count_by | 0.00168 | warm | duckdb-wasm | duckdb-wasm | 99 | 61 | measured | yes | 1.00 | 0.71 |
| h3026 | LOCAL | clustered_30mb_spatial_s3 | features | 0.07432 | warm | duckdb | duckdb | 86 | 97 | measured | yes | 1.00 | 0.88 |
| h3027 | LOCAL | clustered_30mb_shuffled_s3 | features | 0.57823 | warm | duckdb | duckdb | 194 | 778 | measured | yes | 1.00 | 0.95 |
| h3028 | FAST_LAN | clustered_100mb_shuffled_s3 | features | 0.4436 | warm | duckdb | duckdb | 232 | 1213 | measured | yes | 1.00 | 0.94 |
| h3029 | FAST_LAN | uniform_10mb_spatial_s3 | features | 0.12555 | warm | duckdb | duckdb | 223 | 888 | measured | yes | 1.00 | 0.94 |
| h3030 | FAST_LAN | uniform_10mb_shuffled_s3 | features | 0.00951 | warm | duckdb | duckdb | 86 | 239 | measured | yes | 1.00 | 0.80 |
| h3031 | FAST_LAN | uniform_30mb_spatial_s3 | count | 0.00071 | warm | duckdb | duckdb | 106 | 363 | measured | yes | 1.00 | 0.95 |
| h3032 | FAST_LAN | uniform_30mb_shuffled_s3 | features | 0.50839 | warm | duckdb | duckdb | 136 | 554 | measured | yes | 1.00 | 0.95 |
| h3033 | FAST_LAN | uniform_100mb_spatial_s3 | features | 0.05099 | warm | duckdb | duckdb | 783 | 4541 | measured | yes | 1.00 | 0.94 |
| h3034 | FAST_LAN | uniform_100mb_shuffled_s3 | count | 0.00283 | cold | duckdb | duckdb | 106 | 6056 | measured | yes | 1.00 | 0.95 |
| h3035 | FAST_LAN | clustered_10mb_spatial_s3 | count | 0.20977 | cold | duckdb | duckdb | 90 | 978 | measured | yes | 1.00 | 0.95 |
| h3036 | FAST_LAN | clustered_10mb_shuffled_s3 | features | 0.01859 | cold | duckdb | duckdb | 94 | 1065 | measured | yes | 1.00 | 0.87 |
| h3037 | FAST_LAN | clustered_30mb_spatial_s3 | count_by | 0.00035 | warm | duckdb | duckdb | 91 | 343 | measured | yes | 1.00 | 0.87 |
| h3038 | FAST_LAN | clustered_30mb_shuffled_s3 | count | 0.00037 | cold | duckdb | duckdb | 91 | 1226 | measured | yes | 1.00 | 0.95 |
| h3039 | FAST_LAN | clustered_100mb_spatial_s3 | features | 0.01028 | cold | duckdb | duckdb | 90 | 1143 | measured | yes | 1.00 | 0.87 |
| h3040 | FAST_LAN | clustered_100mb_shuffled_s3 | count_by | 0.00994 | warm | duckdb | duckdb | 121 | 5034 | measured | yes | 1.00 | 0.87 |
| h3041 | FAST_LAN | uniform_10mb_spatial_s3 | count_by | 0.00062 | cold | duckdb | duckdb | 104 | 968 | measured | yes | 1.00 | 0.95 |
| h3042 | GOOD_BROADBAND | uniform_100mb_spatial_s3 | count_by | 0.39897 | warm | duckdb | duckdb | 121 | 7888 | measured | yes | 1.00 | 0.86 |
| h3043 | GOOD_BROADBAND | uniform_100mb_shuffled_s3 | count_by | 0.00714 | warm | duckdb | duckdb | 120 | 10614 | measured | yes | 1.00 | 0.86 |
| h3044 | GOOD_BROADBAND | clustered_10mb_spatial_s3 | features | 0.03303 | warm | duckdb | duckdb | 121 | 688 | measured | yes | 1.00 | 0.85 |
| h3045 | GOOD_BROADBAND | clustered_10mb_shuffled_s3 | features | 0.03412 | warm | duckdb | duckdb | 119 | 581 | measured | yes | 1.00 | 0.86 |
| h3046 | GOOD_BROADBAND | clustered_30mb_spatial_s3 | count_by | 0.21397 | cold | duckdb | duckdb | 91 | 3451 | measured | yes | 1.00 | 0.86 |
| h3047 | GOOD_BROADBAND | clustered_30mb_shuffled_s3 | features | 0.28961 | warm | duckdb | duckdb | 493 | 2209 | measured | yes | 1.00 | 0.93 |
| h3048 | GOOD_BROADBAND | clustered_100mb_spatial_s3 | features | 0.018 | cold | duckdb | duckdb | 146 | 3170 | measured | yes | 1.00 | 0.93 |
| h3049 | GOOD_BROADBAND | clustered_1gb_shuffled | count_by | 0.16246 | warm | duckdb | duckdb | 371 | None | skipped (tab limit) | yes | 1.00 | 0.87 |
| h3050 | GOOD_BROADBAND | uniform_10mb_spatial_s3 | count | 0.00364 | cold | duckdb | duckdb | 101 | 2599 | measured | yes | 1.00 | 0.94 |
| h3051 | GOOD_BROADBAND | uniform_10mb_shuffled_s3 | count | 0.00053 | warm | duckdb | duckdb | 99 | 603 | measured | yes | 1.00 | 0.94 |
| h3052 | GOOD_BROADBAND | uniform_30mb_spatial_s3 | count_by | 0.05439 | warm | duckdb | duckdb | 90 | 1444 | measured | yes | 1.00 | 0.86 |
| h3053 | GOOD_BROADBAND | uniform_30mb_shuffled_s3 | features | 0.31987 | warm | duckdb | duckdb | 314 | 2177 | measured | yes | 1.00 | 0.93 |
| h3054 | GOOD_BROADBAND | uniform_100mb_spatial_s3 | count | 0.02808 | warm | duckdb | duckdb | 105 | 3208 | measured | yes | 1.00 | 0.94 |
| h3055 | GOOD_BROADBAND | uniform_100mb_shuffled_s3 | features | 0.08275 | warm | duckdb | duckdb | 351 | 2340 | measured | yes | 1.00 | 0.93 |
| h3056 | GOOD_BROADBAND | clustered_10mb_spatial_s3 | features | 0.00107 | warm | duckdb | duckdb | 110 | 595 | measured | yes | 1.00 | 0.94 |
| h3057 | GOOD_BROADBAND | clustered_1gb_shuffled | features | 0.26143 | warm | duckdb | duckdb | 148 | 914 | measured | yes | 1.00 | 0.94 |
| h3058 | GOOD_BROADBAND | clustered_30mb_spatial_s3 | features | 0.04454 | warm | duckdb | duckdb | 242 | 1555 | measured | yes | 1.00 | 0.93 |
| h3059 | GOOD_BROADBAND | clustered_30mb_shuffled_s3 | features | 0.62891 | warm | duckdb | duckdb | 118 | 1484 | measured | yes | 1.00 | 0.94 |
| h3060 | GOOD_BROADBAND | clustered_100mb_spatial_s3 | features | 0.00359 | warm | duckdb | duckdb | 144 | 1250 | measured | yes | 1.00 | 0.93 |
| h3061 | GOOD_BROADBAND | clustered_100mb_shuffled_s3 | count_by | 0.00239 | warm | duckdb | duckdb | 114 | 10424 | measured | yes | 1.00 | 0.86 |
| h3062 | MOBILE | clustered_30mb_shuffled_s3 | count | 0.05572 | warm | duckdb | duckdb | 114 | 8554 | measured | yes | 1.00 | 0.92 |
| h3063 | MOBILE | clustered_100mb_spatial_s3 | count | 0.03213 | cold | duckdb | duckdb | 97 | 15997 | measured | yes | 1.00 | 0.92 |
| h3064 | MOBILE | clustered_100mb_shuffled_s3 | features | 0.0014 | warm | duckdb | duckdb | 336 | 24557 | measured | yes | 1.00 | 0.91 |
| h3065 | MOBILE | uniform_10mb_spatial_s3 | features | 0.18299 | warm | duckdb | duckdb | 272 | 3123 | measured | yes | 1.00 | 0.91 |
| h3066 | MOBILE | uniform_10mb_shuffled_s3 | features | 0.00137 | warm | duckdb | duckdb | 126 | 3051 | measured | yes | 1.00 | 0.84 |
| h3067 | MOBILE | uniform_30mb_spatial_s3 | features | 0.56695 | warm | duckdb | duckdb | 144 | 8491 | measured | yes | 1.00 | 0.92 |
| h3068 | MOBILE | uniform_30mb_shuffled_s3 | features | 0.00215 | warm | duckdb | duckdb | 275 | 8930 | measured | yes | 1.00 | 0.84 |
| h3069 | MOBILE | uniform_100mb_spatial_s3 | count | 0.03692 | warm | duckdb | duckdb | 108 | 7735 | measured | yes | 1.00 | 0.92 |
| h3070 | MOBILE | uniform_100mb_shuffled_s3 | count | 0.00583 | warm | duckdb | duckdb | 128 | 40155 | measured | yes | 1.00 | 0.92 |
| h3071 | MOBILE | clustered_10mb_spatial_s3 | features | 0.00359 | warm | duckdb | duckdb | 109 | 2922 | measured | yes | 1.00 | 0.84 |
| h3072 | MOBILE | clustered_10mb_shuffled_s3 | count_by | 0.00045 | warm | duckdb | duckdb | 96 | 2984 | measured | yes | 1.00 | 0.84 |
| h3073 | MOBILE | clustered_30mb_spatial_s3 | count | 0.00785 | cold | duckdb | duckdb | 98 | 18791 | measured | yes | 1.00 | 0.92 |
| h3074 | MOBILE | clustered_30mb_shuffled_s3 | count_by | 0.05808 | cold | duckdb | duckdb | 108 | 18912 | measured | yes | 1.00 | 0.84 |
| h3075 | MOBILE | clustered_100mb_spatial_s3 | features | 0.12057 | warm | duckdb | duckdb | 134 | 6704 | measured | yes | 1.00 | 0.92 |
| h3076 | MOBILE | clustered_100mb_shuffled_s3 | count | 0.00382 | cold | duckdb | duckdb | 115 | 50624 | measured | yes | 1.00 | 0.92 |
| h3077 | MOBILE | uniform_10mb_spatial_s3 | features | 0.07864 | warm | duckdb | duckdb | 261 | 3107 | measured | yes | 1.00 | 0.91 |
| h3078 | MOBILE | uniform_10mb_shuffled_s3 | count | 0.58936 | warm | duckdb | duckdb | 99 | 3002 | measured | yes | 1.00 | 0.92 |
| h3079 | MOBILE | uniform_30mb_spatial_s3 | features | 0.0693 | warm | duckdb | duckdb | 930 | 9204 | measured | yes | 1.00 | 0.90 |
| h3080 | MOBILE | uniform_30mb_shuffled_s3 | count_by | 0.00055 | cold | duckdb | duckdb | 107 | 19148 | measured | yes | 1.00 | 0.84 |
| h3081 | MOBILE | uniform_100mb_spatial_s3 | features | 0.03683 | warm | duckdb | duckdb | 150 | 1723 | measured | yes | 1.00 | 0.92 |
| h3082 | HIGH_LATENCY | uniform_1gb_shuffled | count | 0.0005 | warm | duckdb | duckdb | 394 | None | skipped (tab limit) | yes | 1.00 | 0.93 |
| h3083 | HIGH_LATENCY | uniform_30mb_shuffled_s3 | count | 0.08911 | cold | duckdb | duckdb | 186 | 28155 | measured | yes | 1.00 | 0.91 |
| h3084 | HIGH_LATENCY | uniform_100mb_spatial_s3 | features | 0.00655 | warm | duckdb | duckdb | 1456 | 9344 | measured | yes | 1.00 | 0.89 |
| h3085 | HIGH_LATENCY | uniform_100mb_shuffled_s3 | features | 0.00648 | warm | duckdb | duckdb | 231 | 6198 | measured | yes | 1.00 | 0.91 |
| h3086 | HIGH_LATENCY | clustered_10mb_spatial_s3 | features | 0.13199 | warm | duckdb | duckdb | 278 | 4471 | measured | yes | 1.00 | 0.83 |
| h3087 | HIGH_LATENCY | clustered_10mb_shuffled_s3 | features | 0.00252 | cold | duckdb | duckdb | 167 | 19753 | measured | yes | 1.00 | 0.83 |
| h3088 | HIGH_LATENCY | clustered_30mb_spatial_s3 | features | 0.09463 | warm | duckdb | duckdb | 412 | 12689 | measured | yes | 1.00 | 0.90 |
| h3089 | HIGH_LATENCY | clustered_30mb_shuffled_s3 | count_by | 0.54501 | warm | duckdb | duckdb | 181 | 12704 | measured | yes | 1.00 | 0.83 |
| h3090 | HIGH_LATENCY | clustered_100mb_spatial_s3 | features | 0.20122 | cold | duckdb | duckdb | 407 | 25062 | measured | yes | 1.00 | 0.90 |
| h3091 | HIGH_LATENCY | clustered_100mb_shuffled_s3 | features | 0.0021 | cold | duckdb | duckdb | 414 | 72012 | measured | yes | 1.00 | 0.83 |
| h3092 | HIGH_LATENCY | uniform_10mb_spatial_s3 | count_by | 0.01946 | cold | duckdb | duckdb | 187 | 19486 | measured | yes | 1.00 | 0.91 |
| h3093 | HIGH_LATENCY | uniform_10mb_shuffled_s3 | features | 0.00078 | warm | duckdb | duckdb | 201 | 4518 | measured | yes | 1.00 | 0.83 |
| h3094 | HIGH_LATENCY | uniform_30mb_spatial_s3 | features | 0.00139 | warm | duckdb | duckdb | 268 | 12533 | measured | yes | 1.00 | 0.90 |
| h3095 | HIGH_LATENCY | uniform_30mb_shuffled_s3 | features | 0.04048 | warm | duckdb | duckdb | 1337 | 14285 | measured | yes | 1.00 | 0.89 |
| h3096 | HIGH_LATENCY | uniform_100mb_spatial_s3 | features | 0.00194 | warm | duckdb | duckdb | 403 | 4939 | measured | yes | 1.00 | 0.90 |
| h3097 | HIGH_LATENCY | uniform_100mb_shuffled_s3 | count_by | 0.06112 | warm | duckdb | duckdb | 197 | 64750 | measured | yes | 1.00 | 0.83 |
| h3098 | HIGH_LATENCY | clustered_10mb_spatial_s3 | count_by | 0.01413 | warm | duckdb | duckdb | 192 | 4417 | measured | yes | 1.00 | 0.83 |
| h3099 | HIGH_LATENCY | clustered_10mb_shuffled_s3 | features | 0.4432 | warm | duckdb | duckdb | 410 | 4674 | measured | yes | 1.00 | 0.90 |

</details>

## Browser stability (held-out)

browser runs 98 · crashes 0 · engine errors 0 · skipped above the safety envelope 2 · digest mismatches 0 of 40 checked · row-count mismatches 0 · browser chosen and then failed: 0
