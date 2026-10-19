//! Choosing a local metric CRS (ADR-011).
//!
//! A distance in metres on longitude/latitude data is only meaningful after the data is projected into a CRS whose units are
//! metres and whose scale error is small where the data is. This module picks such a CRS (a UTM zone) for an area and REFUSES
//! when no single UTM zone is accurate enough, instead of guessing. It never treats degrees as metres.
//!
//! Accuracy: the transverse-Mercator scale factor at a point is `k = 0.9996 * (1 + (dlon * cos(lat))^2 / 2)` (dlon in radians
//! from the zone's central meridian, spherical approximation). A distance measured in the projected plane is off by about
//! `|k - 1|` relative to the ground distance. [`MAX_SCALE_ERROR`] bounds that over the whole extent.

use crate::{BoundingBox, Crs};

/// Largest accepted relative distance error (0.5 %). The central meridian of a zone has k = 0.9996 (0.04 % error); the edge of a
/// zone at the equator about 0.1 %. Anything beyond 0.5 % means the area is too wide for one zone.
pub const MAX_SCALE_ERROR: f64 = 0.005;
/// UTM is defined from 80 S to 84 N; beyond that a polar projection would be needed (not supported).
pub const MAX_LAT_NORTH: f64 = 84.0;
pub const MIN_LAT_SOUTH: f64 = -80.0;

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum MetricCrsError {
    #[error("the area is given in {0}, which is not a known geographic CRS: cannot choose a metric CRS")]
    NotGeographic(String),
    #[error("latitude {0:.2} is outside the UTM range (80 S to 84 N): polar areas are not supported")]
    Polar(f64),
    #[error("the area is too wide for one metric zone: scale error would reach {error_pct:.2} % (limit {limit_pct:.2} %); zoom in")]
    ExtentTooLarge { error_pct: f64, limit_pct: f64 },
    #[error("coordinates are outside the valid longitude/latitude range")]
    OutOfRange,
}

/// UTM zone (1..=60) of a longitude in degrees.
pub fn utm_zone(lon: f64) -> u32 {
    let l = ((lon + 180.0) % 360.0 + 360.0) % 360.0;
    ((l / 6.0).floor() as u32).min(59) + 1
}

/// Central meridian (degrees) of a UTM zone.
pub fn central_meridian(zone: u32) -> f64 {
    -183.0 + 6.0 * zone as f64
}

/// EPSG code of the UTM zone (WGS 84): 326zz north, 327zz south.
pub fn utm_epsg(zone: u32, north: bool) -> u32 {
    (if north { 32600 } else { 32700 }) + zone
}

/// Zone number and hemisphere of a UTM EPSG code (WGS 84), when it is one.
pub fn utm_zone_of_epsg(code: u32) -> Option<(u32, bool)> {
    match code {
        32601..=32660 => Some((code - 32600, true)),
        32701..=32760 => Some((code - 32700, false)),
        _ => None,
    }
}

/// Worst relative scale error of `zone` over a longitude/latitude extent.
pub fn scale_error(zone: u32, min_lon: f64, max_lon: f64, min_lat: f64, max_lat: f64) -> f64 {
    let cm = central_meridian(zone);
    let dlon = (min_lon - cm).abs().max((max_lon - cm).abs()).to_radians();
    // cos(lat) is largest where |lat| is smallest: the extent's closest latitude to the equator
    let nearest = if min_lat <= 0.0 && max_lat >= 0.0 { 0.0 } else { min_lat.abs().min(max_lat.abs()) };
    let k = 0.9996 * (1.0 + (dlon * nearest.to_radians().cos()).powi(2) / 2.0);
    (k - 1.0).abs().max((0.9996 - 1.0f64).abs())
}

/// The local metric CRS for a longitude/latitude area (its centroid decides the zone). Errors instead of guessing.
pub fn local_metric_crs(area: &BoundingBox) -> Result<Crs, MetricCrsError> {
    match area.crs().is_geographic() {
        Some(true) => {}
        _ => return Err(MetricCrsError::NotGeographic(area.crs().to_string())),
    }
    let (x0, y0, x1, y1) = (area.min_x(), area.min_y(), area.max_x(), area.max_y());
    if x0 < -180.0 || x1 > 180.0 || y0 < -90.0 || y1 > 90.0 {
        return Err(MetricCrsError::OutOfRange);
    }
    for lat in [y0, y1] {
        if !(MIN_LAT_SOUTH..=MAX_LAT_NORTH).contains(&lat) {
            return Err(MetricCrsError::Polar(lat));
        }
    }
    let lon_c = (x0 + x1) / 2.0;
    let lat_c = (y0 + y1) / 2.0;
    let zone = utm_zone(lon_c);
    let err = scale_error(zone, x0, x1, y0, y1);
    if err > MAX_SCALE_ERROR {
        return Err(MetricCrsError::ExtentTooLarge { error_pct: err * 100.0, limit_pct: MAX_SCALE_ERROR * 100.0 });
    }
    Ok(Crs::Epsg(utm_epsg(zone, lat_c >= 0.0)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bb(x0: f64, y0: f64, x1: f64, y1: f64) -> BoundingBox {
        BoundingBox::new(x0, y0, x1, y1, Crs::OgcCrs84).unwrap()
    }

    #[test]
    fn zones_and_codes() {
        assert_eq!(utm_zone(106.7), 48);
        assert_eq!(utm_zone(-0.1), 30);
        assert_eq!(utm_zone(-180.0), 1);
        assert_eq!(utm_zone(179.9), 60);
        assert_eq!(central_meridian(48), 105.0);
        assert_eq!(utm_epsg(48, true), 32648);
        assert_eq!(utm_epsg(18, false), 32718);
        assert_eq!(utm_zone_of_epsg(32648), Some((48, true)));
        assert_eq!(utm_zone_of_epsg(32718), Some((18, false)));
        assert_eq!(utm_zone_of_epsg(3857), None);
        assert_eq!(utm_zone_of_epsg(4326), None);
    }

    #[test]
    fn hcmc_viewport_gets_utm_48n_with_tiny_error() {
        let crs = local_metric_crs(&bb(106.6, 10.7, 106.8, 10.85)).unwrap();
        assert_eq!(crs, Crs::Epsg(32648));
        let e = scale_error(48, 106.6, 106.8, 10.7, 10.85);
        assert!(e < 0.001, "{e}");
    }

    #[test]
    fn southern_hemisphere_uses_327xx() {
        // Sao Paulo, zone 23 S
        assert_eq!(local_metric_crs(&bb(-46.7, -23.6, -46.5, -23.5)).unwrap(), Crs::Epsg(32723));
    }

    #[test]
    fn a_wide_area_is_refused_not_guessed() {
        // 20 degrees of longitude at the equator: scale error about 1.5 %, far outside one zone (10 degrees would still be 0.34 %)
        let e = local_metric_crs(&bb(95.0, -1.0, 115.0, 1.0)).unwrap_err();
        assert!(matches!(e, MetricCrsError::ExtentTooLarge { .. }), "{e}");
        // the whole of southern Vietnam (3 degrees, one zone's width) is still accepted
        assert!(local_metric_crs(&bb(105.0, 8.5, 108.0, 12.0)).is_ok());
    }

    #[test]
    fn polar_and_non_geographic_inputs_are_refused() {
        assert!(matches!(local_metric_crs(&bb(0.0, 85.0, 1.0, 86.0)), Err(MetricCrsError::Polar(_))));
        assert!(matches!(local_metric_crs(&bb(0.0, -85.0, 1.0, -84.0)), Err(MetricCrsError::Polar(_))));
        let projected = BoundingBox::new(0.0, 0.0, 1.0, 1.0, Crs::Epsg(32648)).unwrap();
        assert!(matches!(local_metric_crs(&projected), Err(MetricCrsError::NotGeographic(_))));
        let unknown = BoundingBox::new(0.0, 0.0, 1.0, 1.0, Crs::Unknown("custom".into())).unwrap();
        assert!(matches!(local_metric_crs(&unknown), Err(MetricCrsError::NotGeographic(_))));
        let undefined = BoundingBox::new(0.0, 0.0, 1.0, 1.0, Crs::Undefined).unwrap();
        assert!(matches!(local_metric_crs(&undefined), Err(MetricCrsError::NotGeographic(_))));
    }

    #[test]
    fn zone_boundary_areas_use_the_centroid_zone_and_stay_accurate() {
        // straddles 108 E (zones 48/49): the centroid picks one, the other half is 3 degrees from the central meridian
        let crs = local_metric_crs(&bb(107.8, 10.0, 108.2, 10.2)).unwrap();
        assert!(matches!(crs, Crs::Epsg(32648) | Crs::Epsg(32649)));
    }
}
