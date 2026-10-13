use geo_source_geoparquet::wkt_envelope;
use geo_types::Crs;

#[test]
fn wkt_envelope_basic() {
    let b = wkt_envelope("POLYGON((104 12, 105 12, 105 13, 104 13, 104 12))", &Crs::Epsg(4326)).unwrap();
    assert_eq!((b.min_x(), b.min_y(), b.max_x(), b.max_y()), (104.0, 12.0, 105.0, 13.0));
    let p = wkt_envelope("POINT(1.5 -2.5e0)", &Crs::Epsg(4326)).unwrap();
    assert_eq!((p.min_x(), p.min_y()), (1.5, -2.5));
}

#[test]
fn wkt_envelope_refuses_what_it_cannot_parse_safely() {
    assert!(wkt_envelope("POINT EMPTY", &Crs::Epsg(4326)).is_none());
    assert!(wkt_envelope("POINT Z (1 2 3)", &Crs::Epsg(4326)).is_none());
    assert!(wkt_envelope("garbage", &Crs::Epsg(4326)).is_none());
    assert!(wkt_envelope("POLYGON((1 2 3))", &Crs::Epsg(4326)).is_none(), "odd coordinate count");
}
