use super::{Bfres, Reader};
use crate::formats::binio::{ByteOrder, align_up};
use crate::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrimitiveType {
    Points,
    Lines,
    LineStrip,
    Triangles,
    TriangleStrip,
    Other(u32),
}

impl PrimitiveType {
    fn from_raw(v: u32) -> Self {
        match v {
            0 => Self::Points,
            1 => Self::Lines,
            2 => Self::LineStrip,
            3 => Self::Triangles,
            4 => Self::TriangleStrip,
            other => Self::Other(other),
        }
    }

    #[must_use]
    pub fn gltf_mode(self) -> u32 {
        match self {
            Self::Points => 0,
            Self::Lines => 1,
            Self::LineStrip => 3,
            Self::TriangleStrip => 5,
            Self::Triangles | Self::Other(_) => 4,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexFormat {
    U16,
    U32,
    U8,
}

impl IndexFormat {
    fn from_raw(v: u32) -> Self {
        match v {
            2 => Self::U32,
            0 => Self::U8,
            _ => Self::U16,
        }
    }

    fn size(self) -> usize {
        match self {
            Self::U8 => 1,
            Self::U16 => 2,
            Self::U32 => 4,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttributeFormat(pub u16);

impl AttributeFormat {
    #[must_use]
    pub fn component_count(self) -> usize {
        if self.0 == 0x0809 {
            return 3;
        }
        match self.0 & 0xFF {
            0x02 | 0x03 | 0x0A | 0x14 | 0x16 => 1,
            0x01 | 0x04 | 0x07 | 0x09 | 0x12 | 0x17 => 2,
            0x18 => 3,
            _ => 4,
        }
    }

    #[must_use]
    pub fn name(self) -> String {
        format!("{:#06x}", self.0)
    }

    fn decode(self, data: &[u8]) -> Option<Vec<f32>> {
        let f16 = |b: &[u8]| half::f16::from_le_bytes([b[0], b[1]]).to_f32();
        let u16n = |b: &[u8]| f32::from(u16::from_le_bytes([b[0], b[1]])) / 65535.0;
        let s16n = |b: &[u8]| (f32::from(i16::from_le_bytes([b[0], b[1]])) / 32767.0).max(-1.0);
        let f32v = |b: &[u8]| f32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        match self.0 {
            0x0518 => take(data, 12, 3, f32v),
            0x0517 => take(data, 8, 2, f32v),
            0x0519 => take(data, 16, 4, f32v),
            0x0516 | 0x0416 | 0x0314 => take(data, 4, 1, f32v),
            0x0515 => take(data, 8, 4, f16),
            0x0115 => take(data, 8, 4, u16n),
            0x0215 => take(data, 8, 4, s16n),
            0x0512 => take(data, 4, 2, f16),
            0x0112 => take(data, 4, 2, u16n),
            0x0212 => take(data, 4, 2, s16n),
            0x050A => take(data, 2, 1, f16),
            0x010A => take(data, 2, 1, u16n),
            0x030A => take(data, 2, 1, s16n),
            0x010B | 0x030B => take(data, 4, 4, |b| f32::from(b[0]) / 255.0),
            0x020B | 0x040B => take(data, 4, 4, |b| {
                (f32::from(b[0].cast_signed()) / 127.0).max(-1.0)
            }),
            0x0109 | 0x0309 => take(data, 2, 2, |b| f32::from(b[0]) / 255.0),
            0x0102 | 0x0302 => take(data, 1, 1, |b| f32::from(b[0]) / 255.0),
            0x000B | 0x090B => decode_10_10_10_2(data, false),
            0x020E | 0x099B => decode_10_10_10_2(data, true),
            _ => None,
        }
    }
}

fn take(data: &[u8], need: usize, n: usize, f: impl Fn(&[u8]) -> f32) -> Option<Vec<f32>> {
    if data.len() < need {
        return None;
    }
    let step = need / n;
    Some((0..n).map(|i| f(&data[i * step..])).collect())
}

fn decode_10_10_10_2(data: &[u8], signed: bool) -> Option<Vec<f32>> {
    if data.len() < 4 {
        return None;
    }
    let v = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
    let comp = |shift: u32| -> f32 {
        let raw = u16::try_from((v >> shift) & 0x3FF).unwrap_or(0);
        if signed {
            let s = if raw & 0x200 != 0 {
                raw.cast_signed() - 1024
            } else {
                raw.cast_signed()
            };
            (f32::from(s) / 511.0).max(-1.0)
        } else {
            f32::from(raw) / 1023.0
        }
    };
    let w = f32::from(u8::try_from((v >> 30) & 0x3).unwrap_or(0)) / 3.0;
    Some(vec![comp(0), comp(10), comp(20), w])
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attribute {
    pub name: String,
    pub format: AttributeFormat,
    pub buffer_index: usize,
    pub offset: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VertexBuffer {
    pub attributes: Vec<Attribute>,
    pub buffers: Vec<Vec<u8>>,
    pub strides: Vec<usize>,
    pub vertex_count: u32,
}

impl VertexBuffer {
    #[must_use]
    pub fn attribute(&self, semantic: &str) -> Option<&Attribute> {
        self.attributes.iter().find(|a| a.name == semantic)
    }

    #[must_use]
    pub fn decode_attribute(&self, attr: &Attribute) -> Option<(usize, Vec<f32>)> {
        let buffer = self.buffers.get(attr.buffer_index)?;
        let stride = *self.strides.get(attr.buffer_index)?;
        if stride == 0 {
            return None;
        }
        let mut out = Vec::new();
        let mut components = 0usize;
        for v in 0..self.vertex_count as usize {
            let base = v * stride + attr.offset;
            let slice = buffer.get(base..)?;
            let decoded = attr.format.decode(slice)?;
            components = decoded.len();
            out.extend_from_slice(&decoded);
        }
        Some((components, out))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubMesh {
    pub offset: u32,
    pub count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mesh {
    pub primitive_type: PrimitiveType,
    pub index_format: IndexFormat,
    pub first_vertex: u32,
    pub indices: Vec<u32>,
    pub submeshes: Vec<SubMesh>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shape {
    pub name: String,
    pub material_index: usize,
    pub vertex_buffer_index: usize,
    pub bone_index: usize,
    pub meshes: Vec<Mesh>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Model {
    pub name: String,
    pub vertex_buffers: Vec<VertexBuffer>,
    pub shapes: Vec<Shape>,
    pub materials: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelInfo {
    pub name: String,
    pub vertex_buffers: usize,
    pub shapes: usize,
    pub materials: usize,
}

pub fn parse_models(bfres: &Bfres) -> Result<Vec<Model>> {
    if bfres.byte_order == ByteOrder::Big {
        return Err(Error::malformed(
            "FRES: big-endian model decoding is not supported",
        ));
    }
    let raw = bfres.raw();
    let mut r = Reader::new(raw);
    r.order = bfres.byte_order;
    let major = bfres.version_major();
    let buffer_base = usize::try_from(bfres.buffer_block_offset()?).unwrap_or(0);

    let mut models = Vec::with_capacity(bfres.models.names.len());
    let header_size = model_header_size(major);
    for (i, name) in bfres.models.names.iter().enumerate() {
        let base = bfres.models.values_offset as usize + i * header_size;
        models.push(parse_model(&r, base, name, major, buffer_base)?);
    }
    Ok(models)
}

#[must_use]
pub fn model_infos(bfres: &Bfres) -> Vec<ModelInfo> {
    let raw = bfres.raw();
    let mut r = Reader::new(raw);
    r.order = bfres.byte_order;
    let major = bfres.version_major();
    let header_size = model_header_size(major);
    bfres
        .models
        .names
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let base = bfres.models.values_offset as usize + i * header_size;
            let counts = model_counts(&r, base, major).unwrap_or((0, 0, 0));
            ModelInfo {
                name: name.clone(),
                vertex_buffers: counts.0,
                shapes: counts.1,
                materials: counts.2,
            }
        })
        .collect()
}

fn model_header_size(major: u32) -> usize {
    if major >= 9 { 0x78 } else { 0x70 }
}

fn count_field_offset(major: u32) -> usize {
    if major >= 9 { 0x68 } else { 0x60 }
}

fn model_counts(r: &Reader, base: usize, major: u32) -> Result<(usize, usize, usize)> {
    let off = base + count_field_offset(major);
    let nv = r.u16_at(off)? as usize;
    let ns = r.u16_at(off + 2)? as usize;
    let nm = r.u16_at(off + 4)? as usize;
    Ok((nv, ns, nm))
}

fn parse_model(
    r: &Reader,
    base: usize,
    name: &str,
    major: u32,
    buffer_base: usize,
) -> Result<Model> {
    let mut pos = base + 8;
    let _name = r.off(pos)?;
    pos += 8;
    let _path = r.off(pos)?;
    pos += 8;
    let _skeleton = r.off(pos)?;
    pos += 8;
    let vertex_array = r.off(pos)?;
    pos += 8;
    let shape_values = r.off(pos)?;
    pos += 8;
    let shape_dict = r.off(pos)?;
    pos += 8;
    let _material_values = r.off(pos)?;
    pos += 8;
    let material_dict = r.off(pos)?;

    let (nv, ns, _nm) = model_counts(r, base, major)?;

    let shape_names = super::read_dict_keys(r, shape_dict)?;
    let material_names = super::read_dict_keys(r, material_dict)?;

    let mut vertex_buffers = Vec::with_capacity(nv);
    for i in 0..nv {
        vertex_buffers.push(parse_vertex_buffer(
            r,
            vertex_array + i * vertex_buffer_size(major),
            major,
            buffer_base,
        )?);
    }

    let mut shapes = Vec::with_capacity(ns);
    for i in 0..ns {
        let base = shape_values + i * shape_size(major);
        let sname = shape_names.get(i).map_or("", String::as_str);
        shapes.push(parse_shape(r, base, sname, major, buffer_base)?);
    }

    Ok(Model {
        name: name.to_string(),
        vertex_buffers,
        shapes,
        materials: material_names,
    })
}

fn vertex_buffer_size(major: u32) -> usize {
    if major >= 9 { 0x58 } else { 0x60 }
}

fn shape_size(major: u32) -> usize {
    if major >= 9 { 0x60 } else { 0x68 }
}

fn parse_vertex_buffer(
    r: &Reader,
    base: usize,
    major: u32,
    buffer_base: usize,
) -> Result<VertexBuffer> {
    let mut pos = base + 4;
    if major >= 9 {
        pos += 4;
    } else {
        pos += 12;
    }
    let attr_values = r.off(pos)?;
    pos += 8;
    let attr_dict = r.off(pos)?;
    pos += 8;
    pos += 8;
    pos += 8;
    if major > 2 {
        pos += 8;
    }
    let size_array_off = r.off(pos)?;
    pos += 8;
    let stride_array_off = r.off(pos)?;
    pos += 8;
    pos += 8;
    let local_offset = r.u32_at(pos)? as usize;
    pos += 4;
    let num_attr = r.byte(pos)? as usize;
    pos += 1;
    let num_buffer = r.byte(pos)? as usize;
    pos += 1;
    pos += 2;
    let vertex_count = r.u32_at(pos)?;
    pos += 4;
    pos += 2;
    let gpu_align = if major >= 10 {
        r.u16_at(pos)? as usize
    } else {
        0
    };

    let attr_names = super::read_dict_keys(r, attr_dict)?;
    let mut attributes = Vec::with_capacity(num_attr);
    for i in 0..num_attr {
        let a = attr_values + i * 16;
        let nameoff = r.off(a)?;
        let name = if let Some(n) = attr_names.get(i) {
            n.clone()
        } else {
            r.string_at(nameoff)?
        };
        let format = AttributeFormat(r.u16_be_at(a + 8)?);
        let offset = r.u16_at(a + 12)? as usize;
        let buffer_index = r.u16_at(a + 14)? as usize;
        attributes.push(Attribute {
            name,
            format,
            buffer_index,
            offset,
        });
    }

    let mut strides = Vec::with_capacity(num_buffer);
    let mut sizes = Vec::with_capacity(num_buffer);
    for i in 0..num_buffer {
        strides.push(r.u32_at(stride_array_off + i * 16)? as usize);
        sizes.push(r.u32_at(size_array_off + i * 16)? as usize);
    }

    let mut buffers = Vec::with_capacity(num_buffer);
    let mut cursor = buffer_base + local_offset;
    let align = if gpu_align.is_power_of_two() {
        gpu_align
    } else {
        1
    };
    for &size in &sizes {
        cursor = align_up(cursor, align);
        let data = r.slice(cursor, size, "FVTX buffer").map(<[u8]>::to_vec);
        match data {
            Ok(d) => {
                buffers.push(d);
                cursor += size;
            }
            Err(_) => buffers.push(Vec::new()),
        }
    }

    Ok(VertexBuffer {
        attributes,
        buffers,
        strides,
        vertex_count,
    })
}

fn parse_shape(
    r: &Reader,
    base: usize,
    name: &str,
    major: u32,
    buffer_base: usize,
) -> Result<Shape> {
    let mut pos = base + 4;
    if major >= 9 {
        pos += 4;
    } else {
        pos += 12;
    }
    let _name = r.off(pos)?;
    pos += 8;
    let _vertex_buffer = r.off(pos)?;
    pos += 8;
    let mesh_array = r.off(pos)?;
    pos += 8;
    let _skin_bone = r.off(pos)?;
    pos += 8;
    let _keyshape_values = r.off(pos)?;
    pos += 8;
    let _keyshape_dict = r.off(pos)?;
    pos += 8;
    let _bbox = r.off(pos)?;
    pos += 8;
    if major > 2 {
        let _radius = r.off(pos)?;
        pos += 8;
        pos += 8;
    } else {
        pos += 8;
        pos += 4;
    }
    let _idx = r.u16_at(pos)?;
    pos += 2;
    let material_index = r.u16_at(pos)? as usize;
    pos += 2;
    let bone_index = r.u16_at(pos)? as usize;
    pos += 2;
    let vertex_buffer_index = r.u16_at(pos)? as usize;
    pos += 2;
    let _num_skin = r.u16_at(pos)?;
    pos += 2;
    let _vertex_skin_count = r.byte(pos)?;
    pos += 1;
    let num_mesh = r.byte(pos)? as usize;

    let mut meshes = Vec::with_capacity(num_mesh);
    for i in 0..num_mesh {
        meshes.push(parse_mesh(r, mesh_array + i * 0x38, buffer_base)?);
    }

    Ok(Shape {
        name: name.to_string(),
        material_index,
        vertex_buffer_index,
        bone_index,
        meshes,
    })
}

fn parse_mesh(r: &Reader, base: usize, buffer_base: usize) -> Result<Mesh> {
    let submesh_array = r.off(base)?;
    let _memory_pool = r.off(base + 8)?;
    let _buffer = r.off(base + 16)?;
    let buffer_size_off = r.off(base + 24)?;
    let face_buffer_offset = r.u32_at(base + 32)? as usize;
    let primitive_type = PrimitiveType::from_raw(r.u32_be_at(base + 36)?);
    let index_format = IndexFormat::from_raw(r.u32_be_at(base + 40)?);
    let index_count = r.u32_at(base + 44)? as usize;
    let first_vertex = r.u32_at(base + 48)?;
    let num_submesh = r.u16_at(base + 52)? as usize;

    let size = r.u32_at(buffer_size_off)? as usize;
    let data_off = buffer_base + face_buffer_offset;
    let raw_indices = r.slice(data_off, size, "FSHP index buffer")?;
    let mut indices = decode_indices(raw_indices, index_format);
    indices.truncate(index_count);

    let mut submeshes = Vec::with_capacity(num_submesh);
    for i in 0..num_submesh {
        let s = submesh_array + i * 8;
        submeshes.push(SubMesh {
            offset: r.u32_at(s)?,
            count: r.u32_at(s + 4)?,
        });
    }

    Ok(Mesh {
        primitive_type,
        index_format,
        first_vertex,
        indices,
        submeshes,
    })
}

fn decode_indices(data: &[u8], format: IndexFormat) -> Vec<u32> {
    let step = format.size();
    data.chunks_exact(step)
        .map(|c| match format {
            IndexFormat::U8 => u32::from(c[0]),
            IndexFormat::U16 => u32::from(u16::from_le_bytes([c[0], c[1]])),
            IndexFormat::U32 => u32::from_le_bytes([c[0], c[1], c[2], c[3]]),
        })
        .collect()
}
