| query | result count | result hash (12) | oracle | row groups skipped (est.) / total (obs.) | bytes <= (est.) | bytes read | median ms (obs.) | DuckDB peak buffer MB (obs.) |
|---|---|---|---|---|---|---|---|---|
| ov_bbox_count_small | 812,006 | 6e433193fc37 | pass | 140/154 | 131 MB | unknown | 47 | 29 |
| ov_bbox_count_large | 12,648,395 | 6099a19e218a | pass | 9/154 | 1371 MB | unknown | 206 | 37 |
| ov_attr_filter_fullscan | 8,599,896 | 0b8161df6651 | pass | 0/154 | 1447 MB | unknown | 41 | 5 |
| ov_intersects_polygon | 284,071 | 06c475ea8076 | pass | 140/154 | 131 MB | unknown | 236 | 180 |
| ov_aggregate_version_small | 10 | 60bb979d0b28 | pass | 140/154 | 131 MB | unknown | 41 | 28 |
| ov_spatial_join_small | 145 | be47addbcb8f | pass | 150/154 | 38 MB | unknown | 93 | 161 |
| ov_return_rows_small(id,version) | 28,169 | 041309ba7902 | pass | 150/154 | 38 MB | unknown | 50 | 16 |
