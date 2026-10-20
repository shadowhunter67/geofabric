# Data licences and attribution

These are engineering review notes derived from the committed dataset manifests. They are not legal advice. Dataset files are not redistributed by GeoFabric: they remain under `data/local/` (gitignored). The repository commits manifests, fetch scripts, checksums, and small quality reports.

| Dataset | Provider | Licence | Redistribution | Required attribution (verbatim) | Release / retrieved | Coverage | Changes |
|---|---|---|---|---|---|---|---|
| `copernicus_dem_hcm` | European Space Agency / Copernicus | [Copernicus DEM GLO-30 licence (free for the general public)](https://registry.opendata.aws/copernicus-dem/) | with-attribution | Copernicus Digital Elevation Model (DEM) accessed on 2026-10-16 from https://registry.opendata.aws/copernicus-dem | COP-DEM GLO-30 2021 release 1 / 2026-10-16 | Ho Chi Minh City | Source COG values retained unchanged. |
| `overture_admin_hcm` | Overture Maps Foundation | [ODbL](https://opendatacommons.org/licenses/odbl/) | share-alike | © OpenStreetMap contributors; geoBoundaries (CC BY 4.0); Esri Community Maps contributors (CC BY 4.0); Overture Maps Foundation | 2026-09-23.1 / 2026-10-16 | Ho Chi Minh City | Flat columns; first source record kept; no geometry repair. |
| `overture_buildings_vn_south` | Overture Maps Foundation | [ODbL](https://opendatacommons.org/licenses/odbl/) | share-alike | © OpenStreetMap contributors; Overture Maps Foundation (buildings: OpenStreetMap, Esri Community Maps contributors, Microsoft Global ML Building Footprints, Google Open Buildings and others) | 2026-09-23.1 / 2026-10-16 | Southern Vietnam | Raw Overture schema retained; no geometry repair. |
| `overture_landuse_hcm` | Overture Maps Foundation | [ODbL](https://opendatacommons.org/licenses/odbl/) | share-alike | © OpenStreetMap contributors. Available under the Open Database License. Data from the Daylight Map Distribution. Overture Maps Foundation | 2026-09-23.1 / 2026-10-16 | Ho Chi Minh City | Flat columns; first source record kept; five-degree extent guard; no geometry repair. |
| `overture_places_hcm` | Overture Maps Foundation | [Per-source licences](https://docs.overturemaps.org/attribution/) | with-attribution | Overture Maps Foundation. Places data from Meta, Microsoft, Foursquare (Apache-2.0, Copyright 2024 Foursquare Labs, Inc. All rights reserved), AllThePlaces and other providers; see sources[].license | 2026-09-23.1 / 2026-10-16 | Ho Chi Minh City | Flat columns; first address and source record kept; no geometry repair. |
| `overture_transport_hcm` | Overture Maps Foundation | [ODbL](https://opendatacommons.org/licenses/odbl/) | share-alike | © OpenStreetMap contributors. Available under the Open Database License. Data from TomTom. Overture Maps Foundation | 2026-09-23.1 / 2026-10-16 | Ho Chi Minh City | Flat columns; first road-surface, speed-limit, and source records kept; no geometry repair. |

## Practical consequences

ODbL share-alike applies to derived **databases**. The normalized Overture GeoParquet files are derived databases: they must carry the required attribution and, if redistributed, be offered under ODbL. GeoFabric does not redistribute those files.

Places use licences per source record: CDLA-Permissive-2.0, Apache-2.0 (including the Foursquare notice), and CC0-1.0. Attribution therefore remains per `sources[].license`.

Copernicus DEM is free of charge for the general public. Its manifest attribution text is mandatory.

The basemap uses OpenFreeMap, OpenMapTiles, and OpenStreetMap. Attribution is shown in the map. The basemap server sees only the viewed map area.

## How attribution reaches the screen

The data catalog supplies dataset attribution records to `attributions`. The map renders them in its corner, and Settings exposes the same attribution information for review.

## Not covered / questions for the data owner

This document does not determine whether a particular downstream product is a produced work or a derived database, whether a planned distribution satisfies ODbL offer requirements, or whether upstream provider notices have changed. The data owner should obtain legal review before redistribution, commercial use, or combining sources beyond the documented normalization.

## PostGIS copies (2026-10-20)

Rows imported into a PostGIS database from Overture-derived GeoParquet files remain ODbL data: the attribution and share-alike obligations above apply to the database copy and anything derived from it (`scripts/postgis/load_data.py` keeps the logical source uri in the source catalog so the origin stays traceable). No data was imported in the phase that added the engine.
