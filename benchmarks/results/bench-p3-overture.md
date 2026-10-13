| dataset | size MB | dist | order | query | result count | result hash | validation | rg skipped (est.) / total (obs.) | rows scanned <= (est.) | bytes <= (est.) | bytes read | median ms (obs.) | peak buf MB (obs.) |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| overture_buildings_vn_south | 1451.9 | real | overture-order | ov_bbox_count_small | 812006 | 6e433193fc37 | pass | 140/154 | 1413747 | 131253660 | unknown | 46.7 | 28.8 |
| overture_buildings_vn_south | 1451.9 | real | overture-order | ov_bbox_count_large | 12648395 | 6099a19e218a | pass | 9/154 | 14641856 | 1371226912 | unknown | 205.7 | 37.4 |
| overture_buildings_vn_south | 1451.9 | real | overture-order | ov_attr_filter_fullscan | 8599896 | 0b8161df6651 | pass | 0/154 | 15453032 | 1447492609 | unknown | 41.4 | 4.5 |
| overture_buildings_vn_south | 1451.9 | real | overture-order | ov_intersects_polygon | 284071 | 06c475ea8076 | pass | 140/154 | 1413747 | 131253660 | unknown | 235.8 | 180.1 |
| overture_buildings_vn_south | 1451.9 | real | overture-order | ov_aggregate_version_small | 10 | 60bb979d0b28 | pass | 140/154 | 1413747 | 131253660 | unknown | 41.0 | 28.5 |
| overture_buildings_vn_south | 1451.9 | real | overture-order | ov_spatial_join_small | 145 | be47addbcb8f | pass | 150/154 | 403174 | 37520702 | unknown | 93.3 | 161.2 |
| overture_buildings_vn_south | 1451.9 | real | overture-order | ov_return_rows_small(id,version) | 28169 | 041309ba7902 | pass | 150/154 | 403174 | 37520702 | unknown | 50.3 | 15.5 |
