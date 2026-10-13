type Position = [number, number];
type Geometry =
  | { type: "Point"; coordinates: Position }
  | { type: "LineString"; coordinates: Position[] }
  | { type: "Polygon"; coordinates: Position[][] }
  | { type: "MultiPoint"; coordinates: Position[] }
  | { type: "MultiLineString"; coordinates: Position[][] }
  | { type: "MultiPolygon"; coordinates: Position[][][] }
  | { type: "GeometryCollection"; geometries: Geometry[] };

class WkbReader {
  private readonly view: DataView;
  private offset = 0;

  constructor(bytes: Uint8Array) {
    this.view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  }

  get done(): boolean {
    return this.offset === this.view.byteLength;
  }

  private remaining(): number {
    return this.view.byteLength - this.offset;
  }

  private u8(): number | null {
    if (this.remaining() < 1) return null;
    const value = this.view.getUint8(this.offset);
    this.offset += 1;
    return value;
  }

  private u32(littleEndian: boolean): number | null {
    if (this.remaining() < 4) return null;
    const value = this.view.getUint32(this.offset, littleEndian);
    this.offset += 4;
    return value;
  }

  private f64(littleEndian: boolean): number | null {
    if (this.remaining() < 8) return null;
    const value = this.view.getFloat64(this.offset, littleEndian);
    this.offset += 8;
    return Number.isFinite(value) ? value : null;
  }

  private point(littleEndian: boolean): Position | null {
    const x = this.f64(littleEndian);
    const y = this.f64(littleEndian);
    return x === null || y === null ? null : [x, y];
  }

  private points(littleEndian: boolean): Position[] | null {
    const count = this.u32(littleEndian);
    if (count === null || count > Math.floor(this.remaining() / 16)) return null;

    const points: Position[] = [];
    for (let i = 0; i < count; i += 1) {
      const point = this.point(littleEndian);
      if (point === null) return null;
      points.push(point);
    }
    return points;
  }

  private rings(littleEndian: boolean): Position[][] | null {
    const count = this.u32(littleEndian);
    if (count === null || count > Math.floor(this.remaining() / 4)) return null;

    const rings: Position[][] = [];
    for (let i = 0; i < count; i += 1) {
      const ring = this.points(littleEndian);
      if (ring === null) return null;
      rings.push(ring);
    }
    return rings;
  }

  geom(): Geometry | null {
    const endian = this.u8();
    if (endian !== 0 && endian !== 1) return null;

    const littleEndian = endian === 1;
    const type = this.u32(littleEndian);
    if (type === null) return null;

    switch (type) {
      case 1: {
        const coordinates = this.point(littleEndian);
        return coordinates === null ? null : { type: "Point", coordinates };
      }
      case 2: {
        const coordinates = this.points(littleEndian);
        return coordinates === null ? null : { type: "LineString", coordinates };
      }
      case 3: {
        const coordinates = this.rings(littleEndian);
        return coordinates === null ? null : { type: "Polygon", coordinates };
      }
      case 4:
      case 5:
      case 6:
      case 7:
        return this.collection(type, littleEndian);
      default:
        return null;
    }
  }

  private collection(type: 4 | 5 | 6 | 7, littleEndian: boolean): Geometry | null {
    const count = this.u32(littleEndian);
    if (count === null || count > Math.floor(this.remaining() / 5)) return null;

    const geometries: Geometry[] = [];
    for (let i = 0; i < count; i += 1) {
      const geometry = this.geom();
      if (geometry === null) return null;
      geometries.push(geometry);
    }

    if (type === 7) return { type: "GeometryCollection", geometries };

    if (type === 4) {
      const coordinates = geometries.map((geometry) => geometry.type === "Point" ? geometry.coordinates : null);
      return coordinates.every((position) => position !== null)
        ? { type: "MultiPoint", coordinates: coordinates as Position[] }
        : null;
    }

    if (type === 5) {
      const coordinates = geometries.map((geometry) => geometry.type === "LineString" ? geometry.coordinates : null);
      return coordinates.every((line) => line !== null)
        ? { type: "MultiLineString", coordinates: coordinates as Position[][] }
        : null;
    }

    const coordinates = geometries.map((geometry) => geometry.type === "Polygon" ? geometry.coordinates : null);
    return coordinates.every((polygon) => polygon !== null)
      ? { type: "MultiPolygon", coordinates: coordinates as Position[][][] }
      : null;
  }
}

export function wkbToGeoJSON(input: ArrayBuffer | Uint8Array): Geometry | null {
  const bytes = input instanceof Uint8Array ? input : new Uint8Array(input);
  const reader = new WkbReader(bytes);
  const geometry = reader.geom();
  return geometry !== null && reader.done ? geometry : null;
}
