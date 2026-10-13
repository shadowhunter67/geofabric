import assert from "node:assert/strict";
import { test } from "node:test";
import { wkbToGeoJSON } from "../src/engine/wkb.ts";

function bytes(...values: number[]): Uint8Array {
  return Uint8Array.from(values);
}

function writer(littleEndian = true) {
  const chunks: number[] = [];
  const view = new DataView(new ArrayBuffer(8));

  return {
    endian() {
      chunks.push(littleEndian ? 1 : 0);
    },
    u32(value: number) {
      view.setUint32(0, value, littleEndian);
      chunks.push(...new Uint8Array(view.buffer, 0, 4));
    },
    f64(value: number) {
      view.setFloat64(0, value, littleEndian);
      chunks.push(...new Uint8Array(view.buffer, 0, 8));
    },
    point(x: number, y: number) {
      this.f64(x);
      this.f64(y);
    },
    bytes() {
      return Uint8Array.from(chunks);
    },
  };
}

function pointWkb(x: number, y: number, littleEndian = true): Uint8Array {
  const w = writer(littleEndian);
  w.endian();
  w.u32(1);
  w.point(x, y);
  return w.bytes();
}

function polygonWkb(): Uint8Array {
  const w = writer();
  w.endian();
  w.u32(3);
  w.u32(1);
  w.u32(5);
  for (const [x, y] of [[0, 0], [1, 0], [1, 1], [0, 1], [0, 0]]) {
    w.point(x, y);
  }
  return w.bytes();
}

test("decodes little-endian point", () => {
  assert.deepEqual(wkbToGeoJSON(pointWkb(12.5, -3.25)), {
    type: "Point",
    coordinates: [12.5, -3.25],
  });
});

test("decodes big-endian point", () => {
  assert.deepEqual(wkbToGeoJSON(pointWkb(1.5, 2.75, false)), {
    type: "Point",
    coordinates: [1.5, 2.75],
  });
});

test("decodes polygon rings", () => {
  assert.deepEqual(wkbToGeoJSON(polygonWkb()), {
    type: "Polygon",
    coordinates: [[
      [0, 0],
      [1, 0],
      [1, 1],
      [0, 1],
      [0, 0],
    ]],
  });
});

test("decodes multi-geometries and geometry collections", () => {
  const a = pointWkb(1, 2);
  const b = pointWkb(3, 4);
  const multi = writer();
  multi.endian();
  multi.u32(4);
  multi.u32(2);
  const multiPoint = bytes(...multi.bytes(), ...a, ...b);

  assert.deepEqual(wkbToGeoJSON(multiPoint), {
    type: "MultiPoint",
    coordinates: [[1, 2], [3, 4]],
  });

  const collection = writer();
  collection.endian();
  collection.u32(7);
  collection.u32(2);
  assert.deepEqual(wkbToGeoJSON(bytes(...collection.bytes(), ...a, ...polygonWkb())), {
    type: "GeometryCollection",
    geometries: [
      { type: "Point", coordinates: [1, 2] },
      {
        type: "Polygon",
        coordinates: [[[0, 0], [1, 0], [1, 1], [0, 1], [0, 0]]],
      },
    ],
  });
});

test("rejects truncated, unsupported, mismatched, and trailing data", () => {
  const truncated = polygonWkb().slice(0, 20);
  assert.equal(wkbToGeoJSON(truncated), null);
  assert.equal(wkbToGeoJSON(bytes(1, 0xe9, 0x03, 0, 0)), null);
  assert.equal(wkbToGeoJSON(bytes(...pointWkb(1, 2), 0)), null);

  const wrongChild = writer();
  wrongChild.endian();
  wrongChild.u32(4);
  wrongChild.u32(1);
  assert.equal(wkbToGeoJSON(bytes(...wrongChild.bytes(), ...polygonWkb())), null);
});

test("rejects hostile counts and non-finite coordinates", () => {
  const hostile = writer();
  hostile.endian();
  hostile.u32(3);
  hostile.u32(0xffff_ffff);
  assert.equal(wkbToGeoJSON(hostile.bytes()), null);

  assert.equal(wkbToGeoJSON(pointWkb(Number.NaN, 1)), null);
});
