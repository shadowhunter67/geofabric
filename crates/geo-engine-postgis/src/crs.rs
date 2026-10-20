//! GeoFabric CRS <-> PostGIS SRID. Explicit, one place.
//!
//! * `EPSG:n` -> SRID n.
//! * `OGC:CRS84` -> SRID 4326, **with the axis order stated**: the external axis order of GeoFabric is always (lon, lat), and a
//!   PostGIS geometry in SRID 4326 stores x = longitude, y = latitude (PostGIS never applies the EPSG authority axis order to
//!   stored coordinates; `ST_Transform` takes them as lon/lat). The two therefore agree on coordinates, which is why the mapping
//!   is safe - but they stay DIFFERENT CRSs at the semantic layer: a relation declared `EPSG:4326` does not satisfy a plan that
//!   says `OGC:CRS84` (and the reverse), exactly as in the DuckDB path (`ST_Transform(..., always_xy := true)`).
//! * `Undefined` / `Unknown` -> an error. Never guessed.

use geo_types::Crs;

/// Stored coordinate order of a PostGIS geometry column for the supported SRIDs.
pub const AXIS_ORDER: &str = "lon,lat (x=lon, y=lat)";

pub fn srid_for(crs: &Crs) -> Result<i32, String> {
    match crs {
        Crs::OgcCrs84 => Ok(4326),
        Crs::Epsg(code) if *code > 0 && *code <= i32::MAX as u32 => Ok(*code as i32),
        other => Err(format!("CRS {other} has no SRID mapping (an undefined or unknown CRS is never guessed)")),
    }
}

/// Parse the CRS string of a catalog entry: `OGC:CRS84` or `EPSG:<code>`.
pub fn parse_crs(s: &str) -> Result<Crs, String> {
    let t = s.trim();
    if t.eq_ignore_ascii_case("OGC:CRS84") {
        return Ok(Crs::OgcCrs84);
    }
    if let Some(code) = t.strip_prefix("EPSG:").or_else(|| t.strip_prefix("epsg:")) {
        return code.parse::<u32>().map(Crs::Epsg).map_err(|_| format!("bad EPSG code in {t:?}"));
    }
    Err(format!("unsupported CRS string {t:?} (use OGC:CRS84 or EPSG:<code>)"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mapping_is_explicit() {
        assert_eq!(srid_for(&Crs::OgcCrs84), Ok(4326));
        assert_eq!(srid_for(&Crs::Epsg(32648)), Ok(32648));
        assert!(srid_for(&Crs::Undefined).is_err());
        assert!(srid_for(&Crs::Unknown("x".into())).is_err());
        assert!(srid_for(&Crs::Epsg(0)).is_err());
    }

    #[test]
    fn crs84_and_epsg4326_share_an_srid_but_stay_different_crss() {
        assert_eq!(srid_for(&Crs::OgcCrs84), srid_for(&Crs::Epsg(4326)));
        assert_ne!(parse_crs("OGC:CRS84").unwrap(), parse_crs("EPSG:4326").unwrap());
        assert!(parse_crs("EPSG:abc").is_err() && parse_crs("WGS84").is_err());
    }
}
