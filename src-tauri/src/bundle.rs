//! Rust side of the mesh bundle that carries meshes between Rust and the frontend in both
//! directions. The layout must match src/io/bundle.ts exactly:
//!
//!   u32 little-endian   H, the header length in bytes
//!   H bytes             UTF-8 JSON header
//!   zero bytes          padding up to a multiple of 8
//!   blobs               little-endian f32 / u32 arrays, each starting at a multiple of 8
//!
//! The header holds the command's own fields plus `meshes`, an array whose entries name each blob
//! as [byteOffset from the start of the buffer, element count]. A mesh entry may also carry
//! `extra`, named arrays of either element type as {"name": [byteOffset, count, "f32" | "u32"]}
//! (repair mode's selection, highlights and shell ids); an entry without it has none.

use serde_json::{Map, Value, json};

use crate::mesh::Mesh;

const ALIGN: usize = 8;

fn align(n: usize) -> usize {
    n.div_ceil(ALIGN) * ALIGN
}

/// A named extra array read from a bundle.
#[derive(Clone, Debug, PartialEq)]
pub enum Blob {
    F32(Vec<f32>),
    U32(Vec<u32>),
}

/// A named extra array going to the frontend.
#[derive(Clone, Copy, Debug)]
pub enum BlobRef<'a> {
    F32(&'a [f32]),
    U32(&'a [u32]),
}

impl BlobRef<'_> {
    fn len(&self) -> usize {
        match self {
            BlobRef::F32(v) => v.len(),
            BlobRef::U32(v) => v.len(),
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            BlobRef::F32(_) => "f32",
            BlobRef::U32(_) => "u32",
        }
    }
}

/// One mesh read from a bundle: its JSON fields (without the blob references) and its arrays.
#[derive(Debug)]
pub struct InMesh {
    pub info: Map<String, Value>,
    pub mesh: Mesh,
    /// The named extra arrays, by name; empty when the entry has none.
    pub extra: Vec<(String, Blob)>,
}

impl InMesh {
    /// The extra array called `name`, when the frontend sent one.
    pub fn extra(&self, name: &str) -> Option<&Blob> {
        self.extra.iter().find(|(n, _)| n == name).map(|(_, b)| b)
    }
}

#[derive(Debug)]
pub struct Bundle {
    pub meta: Map<String, Value>,
    pub meshes: Vec<InMesh>,
}

const DAMAGED: &str = "the mesh data sent from the window is damaged";

/// Reads a bundle the frontend sent. Every offset and count is checked against the buffer; mesh
/// indices are not (callers run `Mesh::check`).
pub fn decode(bytes: &[u8]) -> Result<Bundle, String> {
    let damaged = || DAMAGED.to_string();
    let head = bytes.get(0..4).ok_or_else(damaged)?;
    let header_len = u32::from_le_bytes([head[0], head[1], head[2], head[3]]) as usize;
    let header = bytes
        .get(4..4usize.checked_add(header_len).ok_or_else(damaged)?)
        .ok_or_else(damaged)?;
    let Ok(Value::Object(mut meta)) = serde_json::from_slice::<Value>(header) else {
        return Err(damaged());
    };
    let raw = match meta.remove("meshes") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(list)) => list,
        Some(_) => return Err(damaged()),
    };
    let mut meshes = Vec::with_capacity(raw.len());
    for entry in raw {
        let Value::Object(mut info) = entry else {
            return Err(damaged());
        };
        let positions = blob(bytes, info.remove("positions"))?;
        let indices = blob(bytes, info.remove("indices"))?;
        info.remove("edges");
        let extra = match info.remove("extra") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Object(refs)) => refs
                .into_iter()
                .map(|(name, reference)| Ok((name, extra_blob(bytes, reference)?)))
                .collect::<Result<_, String>>()?,
            Some(_) => return Err(damaged()),
        };
        meshes.push(InMesh {
            info,
            mesh: Mesh {
                positions: f32s(positions),
                indices: u32s(indices),
            },
            extra,
        });
    }
    Ok(Bundle { meta, meshes })
}

fn f32s(bytes: &[u8]) -> Vec<f32> {
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|&c| f32::from_le_bytes(c))
        .collect()
}

fn u32s(bytes: &[u8]) -> Vec<u32> {
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|&c| u32::from_le_bytes(c))
        .collect()
}

/// The bytes a `[byteOffset, count]` reference points at, for 4-byte elements.
fn blob(bytes: &[u8], reference: Option<Value>) -> Result<&[u8], String> {
    let Some(Value::Array(pair)) = reference else {
        return Err(DAMAGED.into());
    };
    let [offset, count] = pair.as_slice() else {
        return Err(DAMAGED.into());
    };
    span(bytes, offset, count)
}

/// An extra array from its `[byteOffset, count, "f32" | "u32"]` reference.
fn extra_blob(bytes: &[u8], reference: Value) -> Result<Blob, String> {
    let Value::Array(triple) = reference else {
        return Err(DAMAGED.into());
    };
    let [offset, count, kind] = triple.as_slice() else {
        return Err(DAMAGED.into());
    };
    let data = span(bytes, offset, count)?;
    match kind.as_str() {
        Some("f32") => Ok(Blob::F32(f32s(data))),
        Some("u32") => Ok(Blob::U32(u32s(data))),
        _ => Err(DAMAGED.into()),
    }
}

/// The bytes of `count` 4-byte elements at `offset`, both checked against the buffer.
fn span<'b>(bytes: &'b [u8], offset: &Value, count: &Value) -> Result<&'b [u8], String> {
    let damaged = || DAMAGED.to_string();
    let offset = offset
        .as_u64()
        .and_then(|v| usize::try_from(v).ok())
        .ok_or_else(damaged)?;
    let count = count
        .as_u64()
        .and_then(|v| usize::try_from(v).ok())
        .ok_or_else(damaged)?;
    if offset % 4 != 0 {
        return Err(damaged());
    }
    let end = count
        .checked_mul(4)
        .and_then(|n| n.checked_add(offset))
        .ok_or_else(damaged)?;
    bytes.get(offset..end).ok_or_else(damaged)
}

/// One mesh going to the frontend: JSON fields plus arrays, and the problem edges when known.
pub struct OutMesh<'a> {
    pub info: Map<String, Value>,
    pub positions: &'a [f32],
    pub indices: &'a [u32],
    pub edges: Option<&'a [f32]>,
}

/// The named extra arrays that go with one outgoing mesh, as (name, values); names are distinct.
pub type Extra<'a> = Vec<(&'a str, BlobRef<'a>)>;

/// Packs `meta` and `meshes` into one buffer, laid out exactly as `encodeBundle` in bundle.ts.
pub fn encode(meta: &Map<String, Value>, meshes: &[OutMesh]) -> Vec<u8> {
    encode_with_extra(meta, meshes, &[])
}

/// `encode` with named extra arrays: `extra[i]` goes with `meshes[i]`, and a mesh without an entry
/// (or with an empty one) gets no `extra` field. Each mesh's extra arrays follow its own arrays,
/// in the order given.
pub fn encode_with_extra(
    meta: &Map<String, Value>,
    meshes: &[OutMesh],
    extra: &[Extra],
) -> Vec<u8> {
    const NONE: &[(&str, BlobRef)] = &[];
    let extra_of = |i: usize| extra.get(i).map_or(NONE, Vec::as_slice);
    // Offsets relative to the data section first; the header that lists them comes before the
    // data, so the base offset is settled once the header length is known.
    struct Rel {
        positions: usize,
        indices: usize,
        edges: Option<usize>,
        extra: Vec<usize>,
    }
    let mut rel = Vec::with_capacity(meshes.len());
    let mut data = 0;
    for (i, m) in meshes.iter().enumerate() {
        let positions = data;
        data = align(data + m.positions.len() * 4);
        let indices = data;
        data = align(data + m.indices.len() * 4);
        let edges = m.edges.map(|e| {
            let at = data;
            data = align(data + e.len() * 4);
            at
        });
        let extra = extra_of(i)
            .iter()
            .map(|(_, b)| {
                let at = data;
                data = align(data + b.len() * 4);
                at
            })
            .collect();
        rel.push(Rel {
            positions,
            indices,
            edges,
            extra,
        });
    }

    let header_for = |base: usize| -> Vec<u8> {
        let mut header = meta.clone();
        let list = meshes
            .iter()
            .zip(&rel)
            .enumerate()
            .map(|(i, (m, r))| {
                let mut info = m.info.clone();
                info.insert(
                    "positions".into(),
                    json!([base + r.positions, m.positions.len()]),
                );
                info.insert("indices".into(), json!([base + r.indices, m.indices.len()]));
                if let (Some(at), Some(edges)) = (r.edges, m.edges) {
                    info.insert("edges".into(), json!([base + at, edges.len()]));
                }
                let extra = extra_of(i);
                if !extra.is_empty() {
                    let refs: Map<String, Value> = extra
                        .iter()
                        .zip(&r.extra)
                        .map(|((name, b), at)| {
                            ((*name).to_string(), json!([base + at, b.len(), b.kind()]))
                        })
                        .collect();
                    info.insert("extra".into(), Value::Object(refs));
                }
                Value::Object(info)
            })
            .collect();
        header.insert("meshes".into(), Value::Array(list));
        // Serialising a map of plain JSON values cannot fail.
        serde_json::to_vec(&Value::Object(header)).unwrap_or_default()
    };
    // The header grows with the base offset's digit count; settle on a base the header fits under.
    let mut base = align(4 + header_for(0).len());
    let mut header = header_for(base);
    while align(4 + header.len()) > base {
        base = align(4 + header.len());
        header = header_for(base);
    }

    let mut out = vec![0u8; base + data];
    out[0..4].copy_from_slice(&(header.len() as u32).to_le_bytes());
    out[4..4 + header.len()].copy_from_slice(&header);
    for (i, (m, r)) in meshes.iter().zip(&rel).enumerate() {
        put(
            &mut out[base + r.positions..],
            m.positions.iter().map(|v| v.to_le_bytes()),
        );
        put(
            &mut out[base + r.indices..],
            m.indices.iter().map(|v| v.to_le_bytes()),
        );
        if let (Some(at), Some(edges)) = (r.edges, m.edges) {
            put(&mut out[base + at..], edges.iter().map(|v| v.to_le_bytes()));
        }
        for ((_, b), at) in extra_of(i).iter().zip(&r.extra) {
            let slot = &mut out[base + at..];
            match b {
                BlobRef::F32(v) => put(slot, v.iter().map(|x| x.to_le_bytes())),
                BlobRef::U32(v) => put(slot, v.iter().map(|x| x.to_le_bytes())),
            }
        }
    }
    out
}

fn put(out: &mut [u8], values: impl Iterator<Item = [u8; 4]>) {
    for (slot, bytes) in out.as_chunks_mut::<4>().0.iter_mut().zip(values) {
        slot.copy_from_slice(&bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_u32(b: &[u8], at: usize) -> u32 {
        u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
    }

    fn header_of(out: &[u8]) -> Value {
        let h = read_u32(out, 0) as usize;
        serde_json::from_slice(&out[4..4 + h]).unwrap()
    }

    #[test]
    fn layout_matches_the_js_bundle() {
        let positions = [1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0];
        let indices = [0u32, 1, 2];
        let edges = [0.5f32; 6];
        let mut meta = Map::new();
        meta.insert("format".into(), json!("stl"));
        let mut info = Map::new();
        info.insert("name".into(), json!("cube"));
        let out = encode(
            &meta,
            &[OutMesh {
                info,
                positions: &positions,
                indices: &indices,
                edges: Some(&edges),
            }],
        );

        let h = read_u32(&out, 0) as usize;
        let header: Value = serde_json::from_slice(&out[4..4 + h]).unwrap();
        assert!(
            out[4 + h..align(4 + h)].iter().all(|&b| b == 0),
            "padding is zero"
        );
        assert_eq!(header["format"], "stl");
        let m = &header["meshes"][0];
        assert_eq!(m["name"], "cube");
        assert!(m.get("extra").is_none(), "no extra field without extras");
        let p = m["positions"][0].as_u64().unwrap() as usize;
        let i = m["indices"][0].as_u64().unwrap() as usize;
        let e = m["edges"][0].as_u64().unwrap() as usize;
        assert_eq!(
            p,
            align(4 + h),
            "first blob starts right after the padded header"
        );
        assert_eq!((p % 8, i % 8, e % 8), (0, 0, 0));
        assert_eq!(i, align(p + 36));
        assert_eq!(e, align(i + 12));
        assert_eq!(m["positions"][1], 9);
        assert_eq!(
            f32::from_le_bytes(out[p + 4..p + 8].try_into().unwrap()),
            2.0
        );
        assert_eq!(read_u32(&out, i + 8), 2);
        assert_eq!(out.len(), align(e + 24));
    }

    #[test]
    fn extra_arrays_follow_the_mesh_arrays_aligned_and_typed() {
        let positions = [1.0f32, 2.0, 3.0];
        let indices = [0u32, 0, 0];
        let edges = [0.25f32; 6];
        let selection = [4u32, 7, 9];
        let border = [1.5f32, -2.5, 3.5, 4.5, 5.5];
        let out = encode_with_extra(
            &Map::new(),
            &[OutMesh {
                info: Map::new(),
                positions: &positions,
                indices: &indices,
                edges: Some(&edges),
            }],
            &[vec![
                ("selection", BlobRef::U32(&selection)),
                ("border", BlobRef::F32(&border)),
            ]],
        );
        let header = header_of(&out);
        let m = &header["meshes"][0];
        let e = m["edges"][0].as_u64().unwrap() as usize;
        let s = &m["extra"]["selection"];
        let b = &m["extra"]["border"];
        assert_eq!(s[1], 3);
        assert_eq!(s[2], "u32");
        assert_eq!(b[1], 5);
        assert_eq!(b[2], "f32");
        let s = s[0].as_u64().unwrap() as usize;
        let b = b[0].as_u64().unwrap() as usize;
        // In the order given, after the edges, each at a multiple of 8.
        assert_eq!(s, align(e + 24));
        assert_eq!(b, align(s + 12));
        assert_eq!((s % 8, b % 8), (0, 0));
        assert_eq!(read_u32(&out, s + 4), 7);
        assert_eq!(
            f32::from_le_bytes(out[b + 4..b + 8].try_into().unwrap()),
            -2.5
        );
        assert_eq!(out.len(), align(b + 20));
    }

    #[test]
    fn encode_without_extras_is_unchanged() {
        let positions = [1.0f32; 9];
        let indices = [0u32, 1, 2];
        let mesh = || OutMesh {
            info: Map::new(),
            positions: &positions,
            indices: &indices,
            edges: None,
        };
        let plain = encode(&Map::new(), &[mesh(), mesh()]);
        assert_eq!(
            encode_with_extra(&Map::new(), &[mesh(), mesh()], &[]),
            plain
        );
        assert_eq!(
            encode_with_extra(&Map::new(), &[mesh(), mesh()], &[Vec::new()]),
            plain
        );
        // Extras only for the second mesh: the first mesh's entry has none.
        let ids = [3u32];
        let out = encode_with_extra(
            &Map::new(),
            &[mesh(), mesh()],
            &[Vec::new(), vec![("selection", BlobRef::U32(&ids))]],
        );
        let back = decode(&out).unwrap();
        assert!(back.meshes[0].extra.is_empty());
        assert_eq!(back.meshes[1].extra("selection"), Some(&Blob::U32(vec![3])));
    }

    #[test]
    fn round_trips_through_decode() {
        let a = Mesh {
            positions: vec![0.0, 1.5, -2.25, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0],
            indices: vec![0, 1, 2],
        };
        let b = Mesh {
            positions: vec![9.0; 3],
            indices: vec![],
        };
        let mut meta = Map::new();
        meta.insert("path".into(), json!("C:/x.stl"));
        let out: Vec<OutMesh> = [&a, &b]
            .iter()
            .enumerate()
            .map(|(n, m)| {
                let mut info = Map::new();
                info.insert("name".into(), json!(format!("part {n}")));
                OutMesh {
                    info,
                    positions: &m.positions,
                    indices: &m.indices,
                    edges: None,
                }
            })
            .collect();
        let bytes = encode(&meta, &out);
        let back = decode(&bytes).unwrap();
        assert_eq!(back.meta["path"], "C:/x.stl");
        assert_eq!(back.meshes.len(), 2);
        assert_eq!(back.meshes[0].mesh, a);
        assert_eq!(back.meshes[1].mesh, b);
        assert_eq!(back.meshes[1].info["name"], "part 1");
        assert!(!back.meshes[0].info.contains_key("positions"));
        assert!(back.meshes[0].extra.is_empty());
    }

    #[test]
    fn extra_arrays_round_trip_through_decode() {
        let mesh = make_mesh();
        let selection: Vec<u32> = vec![0, 2, 5, u32::MAX];
        let values: Vec<f32> = vec![0.0, -0.0, 1e-30, f32::MAX, -7.25];
        let empty: Vec<u32> = Vec::new();
        let out = encode_with_extra(
            &Map::new(),
            &[OutMesh {
                info: Map::new(),
                positions: &mesh.positions,
                indices: &mesh.indices,
                edges: None,
            }],
            &[vec![
                ("selection", BlobRef::U32(&selection)),
                ("values", BlobRef::F32(&values)),
                ("none", BlobRef::U32(&empty)),
            ]],
        );
        let back = decode(&out).unwrap();
        let m = &back.meshes[0];
        assert_eq!(m.mesh, mesh);
        assert_eq!(m.extra("selection"), Some(&Blob::U32(selection)));
        let Some(Blob::F32(got)) = m.extra("values") else {
            panic!("values should come back as f32")
        };
        assert_eq!(
            got.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
            values.iter().map(|v| v.to_bits()).collect::<Vec<_>>()
        );
        assert_eq!(m.extra("none"), Some(&Blob::U32(Vec::new())));
        assert_eq!(m.extra("missing"), None);
        assert!(!m.info.contains_key("extra"), "extra leaves the info");
    }

    fn make_mesh() -> Mesh {
        Mesh {
            positions: vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
            indices: vec![0, 1, 2],
        }
    }

    #[test]
    fn decode_rejects_bad_offsets() {
        let mut meta = Map::new();
        meta.insert("x".into(), json!(1));
        let good = encode(
            &meta,
            &[OutMesh {
                info: Map::new(),
                positions: &[1.0; 3],
                indices: &[],
                edges: None,
            }],
        );
        assert!(decode(&good).is_ok());
        assert!(decode(&good[..good.len() - 1]).is_err(), "truncated blob");
        assert!(decode(&[1, 0]).is_err());
        assert!(decode(&[200, 0, 0, 0, b'{']).is_err());

        let header = br#"{"meshes":[{"positions":[2,1],"indices":[0,0]}]}"#;
        let mut odd = (header.len() as u32).to_le_bytes().to_vec();
        odd.extend_from_slice(header);
        odd.resize(odd.len() + 16, 0);
        assert!(decode(&odd).is_err(), "unaligned offset");

        let header = br#"{"meshes":[{"positions":[0,18446744073709551615],"indices":[0,0]}]}"#;
        let mut huge = (header.len() as u32).to_le_bytes().to_vec();
        huge.extend_from_slice(header);
        assert!(decode(&huge).is_err(), "overflowing count");
    }

    /// A bundle whose only mesh has the given `extra` header value, with 64 zero bytes of data.
    fn with_extra(extra: &str) -> Vec<u8> {
        let header =
            format!(r#"{{"meshes":[{{"positions":[0,0],"indices":[0,0],"extra":{extra}}}]}}"#);
        let mut out = (header.len() as u32).to_le_bytes().to_vec();
        out.extend_from_slice(header.as_bytes());
        out.resize(out.len() + 64, 0);
        out
    }

    #[test]
    fn decode_rejects_bad_extra_arrays() {
        assert!(decode(&with_extra(r#"{"a":[8,2,"u32"]}"#)).is_ok());
        assert!(decode(&with_extra(r#"{"a":[8,2,"f32"]}"#)).is_ok());
        assert!(decode(&with_extra("null")).is_ok());
        assert!(decode(&with_extra("{}")).is_ok());
        for bad in [
            r#"{"a":[8,2,"f64"]}"#,
            r#"{"a":[8,2]}"#,
            r#"{"a":[8,2,"u32",1]}"#,
            r#"{"a":[6,2,"u32"]}"#,
            r#"{"a":[8,1000,"u32"]}"#,
            r#"{"a":[-8,2,"u32"]}"#,
            r#"{"a":[8,18446744073709551615,"u32"]}"#,
            r#"{"a":"8,2,u32"}"#,
            r#"[[8,2,"u32"]]"#,
            "7",
        ] {
            assert!(decode(&with_extra(bad)).is_err(), "{bad}");
        }
    }

    #[test]
    fn decodes_a_bundle_built_like_encode_bundle_in_js() {
        // Hand-built the way bundle.ts lays it out: header, pad to 8, positions, pad, indices.
        let make = |base: usize| {
            format!(
                r#"{{"path":"a.stl","format":"stl-binary","meshes":[{{"name":"a","matrix":[1,0,0,0,0,1,0,0,0,0,1,0,0,0,0,1],"positions":[{base},9],"indices":[{},3]}}]}}"#,
                base + 40
            )
        };
        let mut base = 0;
        while align(4 + make(base).len()) != base {
            base = align(4 + make(base).len());
        }
        let header = make(base);
        let mut buf = (header.len() as u32).to_le_bytes().to_vec();
        buf.extend_from_slice(header.as_bytes());
        buf.resize(base, 0);
        for v in [0.0f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0] {
            buf.extend_from_slice(&v.to_le_bytes());
        }
        buf.resize(base + 40, 0);
        for v in [0u32, 1, 2] {
            buf.extend_from_slice(&v.to_le_bytes());
        }
        buf.resize(align(buf.len()), 0);
        let b = decode(&buf).unwrap();
        assert_eq!(b.meta["format"], "stl-binary");
        assert_eq!(b.meshes[0].mesh.indices, vec![0, 1, 2]);
        assert_eq!(b.meshes[0].mesh.positions[3], 1.0);
        assert_eq!(b.meshes[0].info["matrix"].as_array().unwrap().len(), 16);
        assert!(b.meshes[0].extra.is_empty());
    }

    #[test]
    fn decodes_extra_arrays_laid_out_like_encode_bundle_in_js() {
        // bundle.ts: positions, indices, then each extra array in insertion order, each padded
        // to 8; the entry names them as [byteOffset, count, kind].
        let make = |base: usize| {
            format!(
                r#"{{"job":3,"meshes":[{{"positions":[{base},9],"indices":[{},3],"extra":{{"selection":[{},1,"u32"]}}}}]}}"#,
                base + 40,
                base + 56
            )
        };
        let mut base = 0;
        while align(4 + make(base).len()) != base {
            base = align(4 + make(base).len());
        }
        let header = make(base);
        let mut buf = (header.len() as u32).to_le_bytes().to_vec();
        buf.extend_from_slice(header.as_bytes());
        buf.resize(base, 0);
        for v in [0.0f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0] {
            buf.extend_from_slice(&v.to_le_bytes());
        }
        buf.resize(base + 40, 0);
        for v in [0u32, 1, 2] {
            buf.extend_from_slice(&v.to_le_bytes());
        }
        buf.resize(base + 56, 0);
        buf.extend_from_slice(&0u32.to_le_bytes());
        buf.resize(align(buf.len()), 0);
        let b = decode(&buf).unwrap();
        assert_eq!(b.meta["job"], 3);
        assert_eq!(b.meshes[0].extra("selection"), Some(&Blob::U32(vec![0])));
    }
}
