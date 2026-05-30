use serde_json::{Value, json};
use tomolib::formats::bfres::model::{Model, VertexBuffer};

struct Builder {
    bin: Vec<u8>,
    buffer_views: Vec<Value>,
    accessors: Vec<Value>,
}

impl Builder {
    fn new() -> Self {
        Self {
            bin: Vec::new(),
            buffer_views: Vec::new(),
            accessors: Vec::new(),
        }
    }

    fn align(&mut self) {
        while !self.bin.len().is_multiple_of(4) {
            self.bin.push(0);
        }
    }

    fn push_floats(&mut self, data: &[f32], components: usize, target: u32) -> usize {
        self.align();
        let offset = self.bin.len();
        for v in data {
            self.bin.extend_from_slice(&v.to_le_bytes());
        }
        let view = self.buffer_views.len();
        self.buffer_views.push(json!({
            "buffer": 0,
            "byteOffset": offset,
            "byteLength": data.len() * 4,
            "target": target,
        }));
        let count = data.len() / components;
        let accessor = self.accessors.len();
        let ty = match components {
            2 => "VEC2",
            3 => "VEC3",
            4 => "VEC4",
            _ => "SCALAR",
        };
        let mut acc = json!({
            "bufferView": view,
            "componentType": 5126,
            "count": count,
            "type": ty,
        });
        if components == 3 {
            let (min, max) = bounds3(data);
            acc["min"] = json!(min);
            acc["max"] = json!(max);
        }
        self.accessors.push(acc);
        accessor
    }

    fn push_indices(&mut self, indices: &[u32]) -> usize {
        self.align();
        let offset = self.bin.len();
        for i in indices {
            self.bin.extend_from_slice(&i.to_le_bytes());
        }
        let view = self.buffer_views.len();
        self.buffer_views.push(json!({
            "buffer": 0,
            "byteOffset": offset,
            "byteLength": indices.len() * 4,
            "target": 34963,
        }));
        let accessor = self.accessors.len();
        self.accessors.push(json!({
            "bufferView": view,
            "componentType": 5125,
            "count": indices.len(),
            "type": "SCALAR",
        }));
        accessor
    }
}

fn bounds3(data: &[f32]) -> (Vec<f32>, Vec<f32>) {
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for v in data.chunks_exact(3) {
        for i in 0..3 {
            min[i] = min[i].min(v[i]);
            max[i] = max[i].max(v[i]);
        }
    }
    if !min[0].is_finite() {
        return (vec![0.0; 3], vec![0.0; 3]);
    }
    (min.to_vec(), max.to_vec())
}

fn extract_components(vb: &VertexBuffer, semantic: &str, want: usize) -> Option<Vec<f32>> {
    let attr = vb.attribute(semantic)?;
    let (components, data) = vb.decode_attribute(attr)?;
    if components < want {
        return None;
    }
    if components == want {
        return Some(data);
    }
    let count = vb.vertex_count as usize;
    let mut out = Vec::with_capacity(count * want);
    for v in 0..count {
        let base = v * components;
        for c in 0..want {
            out.push(*data.get(base + c)?);
        }
    }
    Some(out)
}

pub(crate) fn build_glb(model: &Model) -> Option<Vec<u8>> {
    let mut b = Builder::new();
    let mut meshes = Vec::new();
    let mut nodes = Vec::new();
    let mut scene_nodes = Vec::new();

    for shape in &model.shapes {
        let Some(vb) = model.vertex_buffers.get(shape.vertex_buffer_index) else {
            continue;
        };
        let Some(positions) = extract_components(vb, "_p0", 3) else {
            continue;
        };
        let Some(mesh0) = shape.meshes.first() else {
            continue;
        };
        if mesh0.indices.is_empty() {
            continue;
        }

        let baked: Vec<u32> = mesh0
            .indices
            .iter()
            .map(|&i| i.wrapping_add(mesh0.first_vertex))
            .collect();
        if baked.iter().any(|&i| i as usize >= positions.len() / 3) {
            continue;
        }

        let mut attributes = serde_json::Map::new();
        attributes.insert(
            "POSITION".to_string(),
            json!(b.push_floats(&positions, 3, 34962)),
        );
        if let Some(normals) = extract_components(vb, "_n0", 3) {
            attributes.insert(
                "NORMAL".to_string(),
                json!(b.push_floats(&normals, 3, 34962)),
            );
        }
        if let Some(uv) = extract_components(vb, "_u0", 2) {
            attributes.insert(
                "TEXCOORD_0".to_string(),
                json!(b.push_floats(&uv, 2, 34962)),
            );
        }
        let index_accessor = b.push_indices(&baked);

        let mesh_index = meshes.len();
        meshes.push(json!({
            "primitives": [{
                "attributes": Value::Object(attributes),
                "indices": index_accessor,
                "mode": mesh0.primitive_type.gltf_mode(),
            }],
            "name": shape.name,
        }));
        let node_index = nodes.len();
        nodes.push(json!({ "mesh": mesh_index, "name": shape.name }));
        scene_nodes.push(node_index);
    }

    if meshes.is_empty() {
        return None;
    }

    let doc = json!({
        "asset": { "version": "2.0", "generator": "tomo bfres" },
        "scene": 0,
        "scenes": [{ "nodes": scene_nodes, "name": model.name }],
        "nodes": nodes,
        "meshes": meshes,
        "buffers": [{ "byteLength": b.bin.len() }],
        "bufferViews": b.buffer_views,
        "accessors": b.accessors,
    });

    Some(assemble_glb(&doc, &b.bin))
}

fn assemble_glb(doc: &Value, bin: &[u8]) -> Vec<u8> {
    let mut json = serde_json::to_vec(doc).unwrap_or_default();
    while !json.len().is_multiple_of(4) {
        json.push(b' ');
    }
    let mut bin = bin.to_vec();
    while !bin.len().is_multiple_of(4) {
        bin.push(0);
    }

    let total = 12 + 8 + json.len() + 8 + bin.len();
    let u32len = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&u32len(total).to_le_bytes());

    out.extend_from_slice(&u32len(json.len()).to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(&json);

    out.extend_from_slice(&u32len(bin.len()).to_le_bytes());
    out.extend_from_slice(b"BIN\0");
    out.extend_from_slice(&bin);

    out
}
