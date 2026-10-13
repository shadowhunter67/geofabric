//! Minimal 2D WKB -> GeoJSON geometry. Used only at the HTTP boundary for
//! map display; the runtime itself never touches GeoJSON.

use serde_json::{json, Value};

struct Rd<'a> {
    b: &'a [u8],
    i: usize,
}

impl Rd<'_> {
    fn u8(&mut self) -> Option<u8> {
        let v = *self.b.get(self.i)?;
        self.i += 1;
        Some(v)
    }
    fn u32(&mut self, le: bool) -> Option<u32> {
        let s: [u8; 4] = self.b.get(self.i..self.i + 4)?.try_into().ok()?;
        self.i += 4;
        Some(if le { u32::from_le_bytes(s) } else { u32::from_be_bytes(s) })
    }
    fn f64(&mut self, le: bool) -> Option<f64> {
        let s: [u8; 8] = self.b.get(self.i..self.i + 8)?.try_into().ok()?;
        self.i += 8;
        Some(if le { f64::from_le_bytes(s) } else { f64::from_be_bytes(s) })
    }
    fn pt(&mut self, le: bool) -> Option<Value> {
        let x = self.f64(le)?;
        let y = self.f64(le)?;
        Some(json!([x, y]))
    }
    fn pts(&mut self, le: bool) -> Option<Vec<Value>> {
        let n = self.u32(le)? as usize;
        if n > (self.b.len() - self.i) / 16 {
            return None;
        }
        (0..n).map(|_| self.pt(le)).collect()
    }
    fn rings(&mut self, le: bool) -> Option<Vec<Value>> {
        let n = self.u32(le)? as usize;
        if n > self.b.len() {
            return None;
        }
        (0..n).map(|_| self.pts(le).map(Value::from)).collect()
    }
    fn geom(&mut self) -> Option<Value> {
        let le = self.u8()? == 1;
        let t = self.u32(le)?;
        match t {
            1 => Some(json!({"type":"Point","coordinates": self.pt(le)?})),
            2 => Some(json!({"type":"LineString","coordinates": self.pts(le)?})),
            3 => Some(json!({"type":"Polygon","coordinates": self.rings(le)?})),
            4 | 5 | 6 | 7 => {
                let n = self.u32(le)? as usize;
                if n > self.b.len() {
                    return None;
                }
                let parts: Option<Vec<Value>> = (0..n).map(|_| self.geom()).collect();
                let parts = parts?;
                if t == 7 {
                    return Some(json!({"type":"GeometryCollection","geometries": parts}));
                }
                let name = match t {
                    4 => "MultiPoint",
                    5 => "MultiLineString",
                    _ => "MultiPolygon",
                };
                let coords: Vec<Value> = parts.into_iter().map(|p| p["coordinates"].clone()).collect();
                Some(json!({"type": name, "coordinates": coords}))
            }
            _ => None, // Z/M/EWKB not supported in v0
        }
    }
}

pub fn wkb_to_geojson(b: &[u8]) -> Option<Value> {
    Rd { b, i: 0 }.geom()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn polygon_wkb() -> Vec<u8> {
        let mut v = vec![1u8];
        v.extend(3u32.to_le_bytes());
        v.extend(1u32.to_le_bytes());
        v.extend(5u32.to_le_bytes());
        for (x, y) in [(0.0f64, 0.0f64), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0), (0.0, 0.0)] {
            v.extend(x.to_le_bytes());
            v.extend(y.to_le_bytes());
        }
        v
    }

    #[test]
    fn polygon_roundtrip() {
        let g = wkb_to_geojson(&polygon_wkb()).unwrap();
        assert_eq!(g["type"], "Polygon");
        assert_eq!(g["coordinates"][0].as_array().unwrap().len(), 5);
    }

    #[test]
    fn rejects_truncated_and_unsupported() {
        let mut w = polygon_wkb();
        w.truncate(20);
        assert!(wkb_to_geojson(&w).is_none());
        assert!(wkb_to_geojson(&[1, 0xE9, 0x03, 0, 0]).is_none(), "Z types unsupported");
        assert!(wkb_to_geojson(&[]).is_none());
    }

    #[test]
    fn hostile_counts_do_not_allocate_or_panic() {
        let mut v = vec![1u8];
        v.extend(3u32.to_le_bytes());
        v.extend(0xFFFF_FFFFu32.to_le_bytes());
        assert!(wkb_to_geojson(&v).is_none());
    }
}
