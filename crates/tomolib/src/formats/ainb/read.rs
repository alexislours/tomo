use std::collections::HashMap;

use crate::formats::ainb::model::{
    Action, Attachment, BB_PARAM_TYPES, BbParam, BbParamType, Blackboard, Command, InputParam,
    Module, Node, NodeType, OutputParam, PARAM_TYPES, PLUG_TYPE_COUNT, ParamSet, ParamSource,
    ParamType, Plug, Property, PropertySet, ReplacementEntry, ReplacementType, Source, StateInfo,
    Transition, UnknownSection0x58, Value,
};
use crate::formats::ainb::{Ainb, Exb, check_version};
use crate::formats::binio::ByteOrder;
use crate::{Error, Result};

const LE: ByteOrder = ByteOrder::Little;
const CTX: &str = "AINB";

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
    pool: usize,
    version: u32,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: 0,
            pool: 0,
            version: 0,
        }
    }

    fn tell(&self) -> usize {
        self.pos
    }
    fn seek(&mut self, off: usize) {
        self.pos = off;
    }

    fn u8(&mut self) -> Result<u8> {
        let v = *self
            .data
            .get(self.pos)
            .ok_or_else(|| Error::truncated(CTX, self.pos, 1, 0))?;
        self.pos += 1;
        Ok(v)
    }
    fn u16(&mut self) -> Result<u16> {
        let v = LE.read_u16(self.data, self.pos, CTX)?;
        self.pos += 2;
        Ok(v)
    }
    fn s16(&mut self) -> Result<i16> {
        Ok(self.u16()?.cast_signed())
    }
    fn u32(&mut self) -> Result<u32> {
        let v = LE.read_u32(self.data, self.pos, CTX)?;
        self.pos += 4;
        Ok(v)
    }
    fn s32(&mut self) -> Result<i32> {
        Ok(self.u32()?.cast_signed())
    }
    fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_bits(self.u32()?))
    }
    fn vec3(&mut self) -> Result<[f32; 3]> {
        Ok([self.f32()?, self.f32()?, self.f32()?])
    }

    fn get_string(&self, off: u32) -> Result<String> {
        let start = self
            .pool
            .checked_add(off as usize)
            .ok_or_else(|| Error::malformed("AINB string offset overflow"))?;
        let slice = self
            .data
            .get(start..)
            .ok_or_else(|| Error::truncated(CTX, start, 1, 0))?;
        let end = slice.iter().position(|&b| b == 0).unwrap_or(slice.len());
        std::str::from_utf8(&slice[..end])
            .map(str::to_owned)
            .map_err(|_| Error::invalid_utf8("AINB string"))
    }

    fn string_off(&mut self) -> Result<String> {
        let off = self.u32()?;
        self.get_string(off)
    }

    fn guid(&mut self) -> Result<String> {
        let time_low = self.u32()?;
        let time_mid = self.u16()?;
        let time_hi = self.u16()?;
        let clock_seq = [self.u8()?, self.u8()?];
        let node = [
            self.u8()?,
            self.u8()?,
            self.u8()?,
            self.u8()?,
            self.u8()?,
            self.u8()?,
        ];
        Ok(format!(
            "{time_low:08x}-{time_mid:04x}-{time_hi:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
            clock_seq[0], clock_seq[1], node[0], node[1], node[2], node[3], node[4], node[5]
        ))
    }
}

fn span(end: usize, start: usize) -> Result<usize> {
    end.checked_sub(start)
        .ok_or_else(|| Error::malformed("AINB section offsets out of order"))
}

fn sub_slice<T: Clone>(s: &[T], start: usize, count: usize) -> Result<Vec<T>> {
    let end = start
        .checked_add(count)
        .filter(|&e| e <= s.len())
        .ok_or_else(|| {
            Error::out_of_range("AINB table index", start.saturating_add(count), s.len())
        })?;
    Ok(s[start..end].to_vec())
}

struct Header {
    filename_offset: u32,
    category_name_offset: u32,
    command_count: usize,
    node_count: usize,
    attachment_count: usize,
    blackboard_offset: usize,
    enum_resolve_offset: usize,
    property_offset: usize,
    transition_offset: usize,
    io_param_offset: usize,
    multi_param_offset: usize,
    attachment_offset: usize,
    attachment_index_offset: usize,
    expression_offset: usize,
    replacement_offset: usize,
    query_offset: usize,
    x58: usize,
    module_offset: usize,
    action_offset: usize,
    x6c: usize,
    blackboard_id_offset: usize,
}

fn read_header(r: &mut Reader) -> Result<Header> {
    let filename_offset = r.u32()?;
    let command_count = r.u32()? as usize;
    let node_count = r.u32()? as usize;
    let _query_count = r.u32()?;
    let attachment_count = r.u32()? as usize;
    let _output_count = r.u32()?;
    let blackboard_offset = r.u32()? as usize;
    let string_pool_offset = r.u32()? as usize;
    r.pool = string_pool_offset;
    let enum_resolve_offset = r.u32()? as usize;
    let property_offset = r.u32()? as usize;
    let transition_offset = r.u32()? as usize;
    let io_param_offset = r.u32()? as usize;
    let multi_param_offset = r.u32()? as usize;
    let attachment_offset = r.u32()? as usize;
    let attachment_index_offset = r.u32()? as usize;
    let expression_offset = r.u32()? as usize;
    let replacement_offset = r.u32()? as usize;
    let query_offset = r.u32()? as usize;
    let _x50 = r.u32()?;
    let _x54 = r.u32()?;
    let x58 = r.u32()? as usize;
    let module_offset = r.u32()? as usize;
    let category_name_offset = r.u32()?;
    let _category = r.u32()?;
    let action_offset = r.u32()? as usize;
    let x6c = r.u32()? as usize;
    let blackboard_id_offset = r.u32()? as usize;
    Ok(Header {
        filename_offset,
        category_name_offset,
        command_count,
        node_count,
        attachment_count,
        blackboard_offset,
        enum_resolve_offset,
        property_offset,
        transition_offset,
        io_param_offset,
        multi_param_offset,
        attachment_offset,
        attachment_index_offset,
        expression_offset,
        replacement_offset,
        query_offset,
        x58,
        module_offset,
        action_offset,
        x6c,
        blackboard_id_offset,
    })
}

fn read_queries(r: &mut Reader, query_offset: usize, end: usize) -> Result<Vec<i32>> {
    let mut queries_raw: Vec<i32> = Vec::new();
    if query_offset < end {
        r.seek(query_offset);
        for _ in 0..(end - query_offset) / 4 {
            let idx = i32::from(r.u16()?);
            let _unk = r.u16()?;
            queries_raw.push(idx);
        }
    }
    Ok(queries_raw)
}

fn read_actions(r: &mut Reader, action_offset: usize) -> Result<HashMap<i32, Vec<Action>>> {
    let mut actions: HashMap<i32, Vec<Action>> = HashMap::new();
    r.seek(action_offset);
    let action_count = r.u32()?;
    for _ in 0..action_count {
        let index = r.s32()?;
        let slot = r.string_off()?;
        let action = r.string_off()?;
        actions.entry(index).or_default().push(Action {
            action_slot: slot,
            action,
        });
    }
    Ok(actions)
}

fn read_modules(r: &mut Reader, module_offset: usize) -> Result<Vec<Module>> {
    r.seek(module_offset);
    let module_count = r.u32()?;
    (0..module_count)
        .map(|_| {
            Ok(Module {
                path: r.string_off()?,
                category: r.string_off()?,
                instance_count: r.u32()?,
            })
        })
        .collect()
}

fn read_replacement_table(
    r: &mut Reader,
    version: u32,
    replacement_offset: usize,
) -> Result<Vec<ReplacementEntry>> {
    let mut replacement_table: Vec<ReplacementEntry> = Vec::new();
    if version >= 0x407 {
        r.seek(replacement_offset);
        let _replaced = r.u8()?;
        let _pad = r.u8()?;
        let replace_count = r.u16()?;
        let _updated_node = r.s16()?;
        let _updated_attach = r.s16()?;
        for _ in 0..replace_count {
            let rtype = ReplacementType::from_value(i32::from(r.u8()?));
            let _pad = r.u8()?;
            replacement_table.push(ReplacementEntry {
                rtype,
                node_index: i32::from(r.s16()?),
                replace_index: i32::from(r.s16()?),
                new_index: i32::from(r.s16()?),
            });
        }
    }
    Ok(replacement_table)
}

fn resolve_query_indices(nodes: &mut [Node]) -> Result<()> {
    let query_indices: Vec<i32> = nodes
        .iter()
        .enumerate()
        .filter(|(_, node)| node.is_query())
        .map(|(i, _)| i32::try_from(i).unwrap_or(-1))
        .collect();
    for node in nodes.iter_mut() {
        for q in &mut node.queries {
            let qi = usize::try_from(*q).map_err(|_| Error::malformed("AINB query index"))?;
            *q = *query_indices
                .get(qi)
                .ok_or_else(|| Error::out_of_range("AINB query", qi, query_indices.len()))?;
        }
    }
    Ok(())
}

fn read_unk_section0x58(r: &mut Reader, x58: usize) -> Result<Option<UnknownSection0x58>> {
    if x58 == 0 {
        return Ok(None);
    }
    r.seek(x58);
    Ok(Some(UnknownSection0x58 {
        description: r.string_off()?,
        unk04: r.u32()?,
        unk08: r.u32()?,
        unk0c: r.u32()?,
    }))
}

fn read_expressions(
    data: &[u8],
    expression_offset: usize,
    module_offset: usize,
) -> Result<Option<Exb>> {
    if expression_offset == 0 {
        return Ok(None);
    }
    let blob = data
        .get(expression_offset..module_offset)
        .ok_or_else(|| Error::truncated(CTX, expression_offset, 0, 0))?
        .to_vec();
    Ok(Some(Exb::parse(blob)?))
}

struct Sections {
    attachments: Vec<Attachment>,
    attachment_indices: Vec<usize>,
    io_params: ParamSet,
    transitions: Vec<Transition>,
}

fn read_sections(r: &mut Reader, header: &Header, properties: &PropertySet) -> Result<Sections> {
    r.seek(header.attachment_offset);
    let attachments = (0..header.attachment_count)
        .map(|_| read_attachment(r, properties))
        .collect::<Result<Vec<_>>>()?;

    r.seek(header.attachment_index_offset);
    let attachment_indices: Vec<usize> =
        (0..span(header.attachment_offset, header.attachment_index_offset)? / 4)
            .map(|_| r.u32().map(|v| v as usize))
            .collect::<Result<Vec<_>>>()?;

    r.seek(header.multi_param_offset);
    let multi_sources: Vec<ParamSource> =
        (0..span(header.transition_offset, header.multi_param_offset)? / 8)
            .map(|_| read_param_source(r))
            .collect::<Result<Vec<_>>>()?;

    r.seek(header.io_param_offset);
    let io_params = read_param_set(r, header.multi_param_offset, &multi_sources)?;

    let mut transitions: Vec<Transition> = Vec::new();
    if header.transition_offset < header.query_offset {
        r.seek(header.transition_offset);
        transitions = read_transitions(r)?;
    }

    Ok(Sections {
        attachments,
        attachment_indices,
        io_params,
        transitions,
    })
}

struct NodeCtx<'a> {
    attachments: &'a [Attachment],
    attachment_indices: &'a [usize],
    properties: &'a PropertySet,
    io_params: &'a ParamSet,
    transitions: &'a [Transition],
    queries_raw: &'a [i32],
    actions: &'a HashMap<i32, Vec<Action>>,
}

pub(super) fn read(data: &[u8]) -> Result<Ainb> {
    let mut r = Reader::new(data);
    if data.len() < 0x74 || &data[0..4] != b"AIB " {
        return Err(Error::bad_magic("AINB"));
    }
    r.seek(4);
    let version = r.u32()?;
    check_version(version)?;
    r.version = version;

    let header = read_header(&mut r)?;

    let filename = r.get_string(header.filename_offset)?;
    let category = r.get_string(header.category_name_offset)?;

    let commands = (0..header.command_count)
        .map(|_| read_command(&mut r))
        .collect::<Result<Vec<_>>>()?;

    let node_offset = r.tell();

    r.seek(header.enum_resolve_offset);
    let num_enums = r.u32()?;
    if num_enums != 0 {
        return Err(Error::unsupported(
            "AINB enum resolutions are not supported".to_string(),
        ));
    }

    r.seek(header.blackboard_offset);
    let blackboard = Some(read_blackboard(&mut r)?);

    let expressions = read_expressions(data, header.expression_offset, header.module_offset)?;

    r.seek(header.property_offset);
    let properties = read_property_set(&mut r, header.io_param_offset)?;

    let sections = read_sections(&mut r, &header, &properties)?;

    let end = if header.expression_offset != 0 {
        header.expression_offset
    } else {
        header.module_offset
    };
    let queries_raw = read_queries(&mut r, header.query_offset, end)?;

    let actions = read_actions(&mut r, header.action_offset)?;
    let modules = read_modules(&mut r, header.module_offset)?;

    r.seek(header.blackboard_id_offset);
    let blackboard_id = r.u32()?;
    let parent_blackboard_id = r.u32()?;

    let replacement_table = read_replacement_table(&mut r, version, header.replacement_offset)?;

    r.seek(node_offset);
    let node_ctx = NodeCtx {
        attachments: &sections.attachments,
        attachment_indices: &sections.attachment_indices,
        properties: &properties,
        io_params: &sections.io_params,
        transitions: &sections.transitions,
        queries_raw: &queries_raw,
        actions: &actions,
    };
    let mut nodes = (0..header.node_count)
        .map(|_| read_node(&mut r, &node_ctx))
        .collect::<Result<Vec<_>>>()?;

    resolve_query_indices(&mut nodes)?;

    let unk_section0x58 = read_unk_section0x58(&mut r, header.x58)?;
    let exists_section_0x6c = header.x6c != 0;

    Ok(Ainb {
        version,
        filename,
        category,
        blackboard_id,
        parent_blackboard_id,
        commands,
        nodes,
        blackboard,
        expressions,
        replacement_table,
        modules,
        unk_section0x58,
        exists_section_0x6c,
    })
}

fn read_command(r: &mut Reader) -> Result<Command> {
    let name = r.string_off()?;
    let guid = r.guid()?;
    let root_node_index = i32::from(r.u16()?);
    let secondary_root_node_index = i32::from(r.u16()?) - 1;
    Ok(Command {
        name,
        guid,
        root_node_index,
        secondary_root_node_index,
    })
}

fn read_transitions(r: &mut Reader) -> Result<Vec<Transition>> {
    let first = r.u32()?;
    let mut offsets = vec![first];
    while r.tell() < first as usize {
        offsets.push(r.u32()?);
    }
    offsets
        .into_iter()
        .map(|off| read_transition(r, off as usize))
        .collect()
}

fn read_transition(r: &mut Reader, off: usize) -> Result<Transition> {
    r.seek(off);
    let flags = r.u32()?;
    let transition_type = flags & 0xff;
    let update_post_calc = (flags >> 0x1f) & 1 != 0;
    let command_name = if transition_type == 0 {
        r.string_off()?
    } else {
        String::new()
    };
    Ok(Transition {
        transition_type,
        update_post_calc,
        command_name,
    })
}

fn read_param_source(r: &mut Reader) -> Result<ParamSource> {
    let node = i32::from(r.s16()?);
    let out = i32::from(r.s16()?);
    let flags = r.u32()?;
    Ok(ParamSource {
        src_node_index: node,
        src_output_index: out,
        flags,
    })
}

fn read_value(r: &mut Reader, ptype: ParamType) -> Result<Value> {
    Ok(match ptype {
        ParamType::Int => Value::Int(r.s32()?),
        ParamType::Bool => Value::Bool(r.u32()? != 0),
        ParamType::Float => Value::Float(r.f32()?),
        ParamType::String => Value::Str(r.string_off()?),
        ParamType::Vector3F => Value::Vec3(r.vec3()?),
        ParamType::Pointer => Value::Null,
    })
}

fn read_property_set(r: &mut Reader, end_offset: usize) -> Result<PropertySet> {
    let offsets: Vec<usize> = (0..6)
        .map(|_| r.u32().map(|v| v as usize))
        .collect::<Result<_>>()?;
    let mut set = PropertySet::default();
    for (i, ptype) in PARAM_TYPES.into_iter().enumerate() {
        let start = offsets[i];
        let end = if i < 5 { offsets[i + 1] } else { end_offset };
        let count = span(end, start)? / ptype.property_size();
        if count > r.data.len() {
            return Err(Error::out_of_range(
                "AINB property count",
                count,
                r.data.len(),
            ));
        }
        r.seek(start);
        let mut v = Vec::with_capacity(count);
        for _ in 0..count {
            v.push(read_property(r, ptype)?);
        }
        set.props[ptype.index()] = v;
    }
    Ok(set)
}

fn read_property(r: &mut Reader, ptype: ParamType) -> Result<Property> {
    let name = r.string_off()?;
    let classname = if ptype == ParamType::Pointer {
        r.string_off()?
    } else {
        String::new()
    };
    let flags = r.u32()?;
    let value = read_value(r, ptype)?;
    Ok(Property {
        name,
        classname,
        ptype,
        flags,
        value,
    })
}

fn read_param_set(r: &mut Reader, end_offset: usize, multi: &[ParamSource]) -> Result<ParamSet> {
    let mut input_off = [0usize; 6];
    let mut output_off = [0usize; 6];
    for i in 0..6 {
        input_off[i] = r.u32()? as usize;
        output_off[i] = r.u32()? as usize;
    }
    let mut set = ParamSet::default();
    for (i, ptype) in PARAM_TYPES.into_iter().enumerate() {
        let in_end = output_off[i];
        let out_end = if i < 5 { input_off[i + 1] } else { end_offset };
        let in_count = span(in_end, input_off[i])? / ptype.input_size();
        if in_count > r.data.len() {
            return Err(Error::out_of_range(
                "AINB input count",
                in_count,
                r.data.len(),
            ));
        }
        r.seek(input_off[i]);
        let mut inputs = Vec::with_capacity(in_count);
        for _ in 0..in_count {
            inputs.push(read_input(r, ptype, multi)?);
        }
        set.inputs[ptype.index()] = inputs;

        let out_count = span(out_end, output_off[i])? / ptype.output_size();
        if out_count > r.data.len() {
            return Err(Error::out_of_range(
                "AINB output count",
                out_count,
                r.data.len(),
            ));
        }
        r.seek(output_off[i]);
        let mut outputs = Vec::with_capacity(out_count);
        for _ in 0..out_count {
            outputs.push(read_output(r, ptype)?);
        }
        set.outputs[ptype.index()] = outputs;
    }
    Ok(set)
}

fn read_input(r: &mut Reader, ptype: ParamType, multi: &[ParamSource]) -> Result<InputParam> {
    let name = r.string_off()?;
    let classname = if ptype == ParamType::Pointer {
        r.string_off()?
    } else {
        String::new()
    };
    let mut src = read_param_source(r)?;
    let value = if ptype == ParamType::Pointer {
        let _ = r.u32()?;
        Value::Null
    } else {
        read_value(r, ptype)?
    };
    let mut is_blackboard_input = false;
    let source = if src.is_multi() {
        let start = src.multi_index();
        let count = src.multi_count();
        Source::Multi(sub_slice(multi, start, count)?)
    } else {
        if src.src_output_index < 0 {
            src.src_output_index &= 0x7fff;
            is_blackboard_input = true;
        }
        Source::Single(src)
    };
    Ok(InputParam {
        name,
        classname,
        ptype,
        value,
        source,
        is_blackboard_input,
    })
}

fn read_output(r: &mut Reader, ptype: ParamType) -> Result<OutputParam> {
    let flags = r.u32()?;
    let name = r.get_string(flags & 0x3fff_ffff)?;
    let classname = if ptype == ParamType::Pointer {
        r.string_off()?
    } else {
        String::new()
    };
    let is_output = (flags >> 0x1f) & 1 != 0;
    Ok(OutputParam {
        name,
        classname,
        ptype,
        is_output,
    })
}

struct BbHdr {
    count: usize,
    offset: usize,
}

struct BbInfo {
    file_ref_index: i32,
    name: String,
    notes: String,
    flags: u8,
}

fn read_blackboard(r: &mut Reader) -> Result<Blackboard> {
    let mut headers: Vec<BbHdr> = Vec::with_capacity(7);
    for ptype in BB_PARAM_TYPES {
        if ptype.is_supported(r.version) {
            let count = r.u16()? as usize;
            let _base = r.u16()?;
            let offset = r.u16()? as usize;
            let _pad = r.u16()?;
            headers.push(BbHdr { count, offset });
        } else {
            headers.push(BbHdr {
                count: 0,
                offset: 0,
            });
        }
    }

    let mut infos: Vec<Vec<BbInfo>> = Vec::with_capacity(7);
    for (i, _ptype) in BB_PARAM_TYPES.into_iter().enumerate() {
        let mut v = Vec::with_capacity(headers[i].count);
        for _ in 0..headers[i].count {
            let flags = r.u32()?;
            let file_ref_index = if flags >> 0x1f != 0 {
                i32::try_from((flags >> 0x18) & 0x7f).unwrap_or(-1)
            } else {
                -1
            };
            let name = r.get_string(flags & 0x3f_ffff)?;
            let notes = r.string_off()?;
            let pflags = ((flags >> 0x16) & 3) as u8;
            v.push(BbInfo {
                file_ref_index,
                name,
                notes,
                flags: pflags,
            });
        }
        infos.push(v);
    }

    let base_offset = r.tell();
    let vec3_idx = BbParamType::Vec3f.index();
    let file_ref_offset = base_offset + headers[vec3_idx].offset + headers[vec3_idx].count * 0xc;

    let mut bb = Blackboard::default();
    for (i, ptype) in BB_PARAM_TYPES.into_iter().enumerate() {
        r.seek(base_offset + headers[i].offset);
        let mut params = Vec::with_capacity(headers[i].count);
        for info in &infos[i] {
            let value = read_bb_value(r, ptype)?;
            let file_ref = if info.file_ref_index == -1 {
                String::new()
            } else {
                let save = r.tell();
                let fri = usize::try_from(info.file_ref_index).unwrap_or(0);
                r.seek(file_ref_offset + 0x10 * fri);
                let fr = r.string_off()?;
                let _ph = r.u32()?;
                let _fh = r.u32()?;
                let _eh = r.u32()?;
                r.seek(save);
                fr
            };
            params.push(BbParam {
                name: info.name.clone(),
                notes: info.notes.clone(),
                file_ref,
                flags: info.flags,
                value,
            });
        }
        bb.params[ptype.index()] = params;
    }
    Ok(bb)
}

fn read_bb_value(r: &mut Reader, ptype: BbParamType) -> Result<Value> {
    Ok(match ptype {
        BbParamType::String => Value::Str(r.string_off()?),
        BbParamType::S32 => Value::Int(r.s32()?),
        BbParamType::U32 => Value::UInt(r.u32()?),
        BbParamType::F32 => Value::Float(r.f32()?),
        BbParamType::Bool => Value::Bool(r.u32()? != 0),
        BbParamType::Vec3f => Value::Vec3(r.vec3()?),
        BbParamType::VoidPtr => Value::Null,
    })
}

fn read_attachment(r: &mut Reader, properties: &PropertySet) -> Result<Attachment> {
    let name = r.string_off()?;
    let offset = r.u32()? as usize;
    let expr_count = r.u16()?;
    let expr_io_size = r.u16()?;
    if r.version >= 0x407 {
        let _hash = r.u32()?;
    }
    let save = r.tell();
    r.seek(offset);
    let debug = r.u32()?;
    let mut props = PropertySet::default();
    for ptype in PARAM_TYPES {
        let base = r.u32()? as usize;
        let count = r.u32()? as usize;
        props.props[ptype.index()] = sub_slice(properties.get(ptype), base, count)?;
    }
    r.seek(save);
    Ok(Attachment {
        name,
        debug,
        properties: props,
        expr_count,
        expr_io_size,
    })
}

fn read_node(r: &mut Reader, ctx: &NodeCtx) -> Result<Node> {
    let ntype =
        NodeType::from_value(r.u16()?).ok_or_else(|| Error::malformed("AINB unknown node type"))?;
    let index = i32::from(r.s16()?);
    let attachment_count = r.u16()? as usize;
    let flags = r.u8()?;
    let _pad = r.u8()?;
    let name = r.string_off()?;
    if r.version >= 0x407 {
        let _name_hash = r.u32()?;
    }
    let _unk1 = r.u32()?;
    let node_param_offset = r.u32()? as usize;
    let expr_count = r.u16()?;
    let expr_io_size = r.u16()?;
    let _multi_param_count = r.u16()?;
    let _pad2 = r.u16()?;
    let base_attachment_index = r.u32()? as usize;
    let base_query_index = r.u16()? as usize;
    let query_count = r.u16()? as usize;
    let state_info_offset = r.u32()? as usize;
    let state_info = read_state_info(r, state_info_offset)?;
    let guid = r.guid()?;
    let node_end = r.tell();

    let queries = sub_slice(ctx.queries_raw, base_query_index, query_count)?;
    let node_attachments: Vec<Attachment> = sub_slice(
        ctx.attachment_indices,
        base_attachment_index,
        attachment_count,
    )?
    .into_iter()
    .map(|i| {
        ctx.attachments
            .get(i)
            .cloned()
            .ok_or_else(|| Error::out_of_range("AINB attachment", i, ctx.attachments.len()))
    })
    .collect::<Result<_>>()?;

    r.seek(node_param_offset);
    let (node_props, node_params) = read_node_params(r, ctx.properties, ctx.io_params)?;
    let plugs = read_node_plugs(r, ntype, &name, ctx.transitions)?;

    r.seek(node_end);
    let node_actions = ctx.actions.get(&index).cloned().unwrap_or_default();

    Ok(Node {
        name,
        ntype,
        index,
        flags,
        queries,
        attachments: node_attachments,
        properties: node_props,
        params: node_params,
        actions: node_actions,
        guid,
        state_info,
        plugs,
        expr_count,
        expr_io_size,
    })
}

fn read_state_info(r: &mut Reader, state_info_offset: usize) -> Result<Option<StateInfo>> {
    if r.version >= 0x407 {
        return Ok(None);
    }
    let save = r.tell();
    r.seek(state_info_offset);
    let state_info = StateInfo {
        desired_state: r.string_off()?,
        unk04: r.u32()?,
        unk08: r.u32()?,
        unk0c: r.u32()?,
        unk10: r.u32()?,
    };
    r.seek(save);
    Ok(Some(state_info))
}

fn read_node_params(
    r: &mut Reader,
    properties: &PropertySet,
    io_params: &ParamSet,
) -> Result<(PropertySet, ParamSet)> {
    let mut node_props = PropertySet::default();
    let mut node_params = ParamSet::default();
    for ptype in PARAM_TYPES {
        let base = r.u32()? as usize;
        let count = r.u32()? as usize;
        node_props.props[ptype.index()] = sub_slice(properties.get(ptype), base, count)?;
    }
    for ptype in PARAM_TYPES {
        let bi = r.u32()? as usize;
        let ic = r.u32()? as usize;
        node_params.inputs[ptype.index()] = sub_slice(io_params.inputs(ptype), bi, ic)?;
        let bo = r.u32()? as usize;
        let oc = r.u32()? as usize;
        node_params.outputs[ptype.index()] = sub_slice(io_params.outputs(ptype), bo, oc)?;
    }
    Ok((node_props, node_params))
}

fn read_node_plugs(
    r: &mut Reader,
    ntype: NodeType,
    name: &str,
    transitions: &[Transition],
) -> Result<[Vec<Plug>; PLUG_TYPE_COUNT]> {
    let mut plugs: [Vec<Plug>; PLUG_TYPE_COUNT] = Default::default();
    let mut plug_count = [0u8; PLUG_TYPE_COUNT];
    let mut plug_base = [0u8; PLUG_TYPE_COUNT];
    for i in 0..PLUG_TYPE_COUNT {
        plug_count[i] = r.u8()?;
        plug_base[i] = r.u8()?;
    }
    let base_offset = r.tell();
    for pt in 0..PLUG_TYPE_COUNT {
        let save = r.tell();
        r.seek(base_offset + plug_base[pt] as usize * 4);
        let offsets: Vec<u32> = (0..plug_count[pt])
            .map(|_| r.u32())
            .collect::<Result<_>>()?;
        let n = offsets.len();
        let mut v = Vec::with_capacity(n);
        for (i, off) in offsets.into_iter().enumerate() {
            v.push(read_plug(
                r,
                off as usize,
                pt,
                ntype,
                name,
                i == n - 1,
                transitions,
            )?);
        }
        plugs[pt] = v;
        r.seek(save);
    }
    Ok(plugs)
}

fn read_plug(
    r: &mut Reader,
    offset: usize,
    plug_type: usize,
    ntype: NodeType,
    node_name: &str,
    is_last: bool,
    transitions: &[Transition],
) -> Result<Plug> {
    r.seek(offset);
    match plug_type {
        0 => match ntype {
            NodeType::ElementBoolSelector => {
                let node_index = r.s32()?;
                let name = r.string_off()?;
                Ok(Plug::BoolSelectorInput {
                    node_index,
                    name,
                    unk0: r.u32()?,
                    unk1: r.u32()?,
                })
            }
            NodeType::ElementF32Selector => {
                let node_index = r.s32()?;
                let name = r.string_off()?;
                Ok(Plug::F32SelectorInput {
                    node_index,
                    name,
                    unk0: r.u32()?,
                    unk1: r.f32()?,
                })
            }
            NodeType::ElementExpression => read_s32_selector_input(r),
            _ => read_generic(r),
        },
        2 => match ntype {
            NodeType::ElementS32Selector => read_s32_selector(r, is_last),
            NodeType::ElementF32Selector => read_f32_selector(r, is_last),
            NodeType::ElementStringSelector => read_string_selector(r, is_last),
            NodeType::ElementRandomSelector => read_random_selector(r),
            _ if node_name == "SelectorBSABrainVerbUpdater"
                || node_name == "SelectorBSAFormChangeUpdater" =>
            {
                read_bsa_selector_updater(r)
            }
            _ => read_child(r),
        },
        3 => {
            let node_index = r.s32()?;
            let idx = r.u32()? as usize;
            let transition = transitions.get(idx).cloned().ok_or_else(|| {
                Error::out_of_range("AINB transition plug", idx, transitions.len())
            })?;
            Ok(Plug::Transition {
                node_index,
                transition,
            })
        }
        4 => match ntype {
            NodeType::ElementStringSelector | NodeType::ElementExpression => {
                read_string_selector_input(r, r.version)
            }
            _ => read_generic(r),
        },
        5 => match ntype {
            NodeType::ElementS32Selector | NodeType::ElementExpression => {
                read_s32_selector_input(r)
            }
            _ => read_generic(r),
        },
        _ => Err(Error::malformed(format!(
            "AINB unsupported plug type {plug_type}"
        ))),
    }
}

fn read_generic(r: &mut Reader) -> Result<Plug> {
    let node_index = r.s32()?;
    let name = r.string_off()?;
    Ok(Plug::Generic { node_index, name })
}

fn read_child(r: &mut Reader) -> Result<Plug> {
    let node_index = r.s32()?;
    let name = r.string_off()?;
    Ok(Plug::Child { node_index, name })
}

fn read_s32_selector(r: &mut Reader, is_last: bool) -> Result<Plug> {
    let node_index = r.s32()?;
    let name = r.string_off()?;
    let index = i32::from(r.s16()?);
    let flag = r.u16()?;
    let mut blackboard_index = -1;
    if flag >> 0xf != 0 {
        blackboard_index = index;
    }
    let mut condition = 0;
    let mut is_default = false;
    if is_last {
        is_default = true;
        let _ = r.s32()?;
    } else {
        condition = r.s32()?;
    }
    Ok(Plug::S32Selector {
        node_index,
        name,
        condition,
        is_default,
        blackboard_index,
    })
}

fn read_f32_selector(r: &mut Reader, is_last: bool) -> Result<Plug> {
    let node_index = r.s32()?;
    let name = r.string_off()?;
    let mut condition_min = 0.0;
    let mut blackboard_index_min = -1;
    let mut condition_max = 0.0;
    let mut blackboard_index_max = -1;
    let mut is_default = false;
    if is_last {
        is_default = true;
    } else {
        let index = i32::from(r.s16()?);
        let flag = r.u16()?;
        if flag >> 0xf != 0 {
            blackboard_index_min = index;
            let _ = r.f32()?;
        } else {
            condition_min = r.f32()?;
        }
        let index = i32::from(r.s16()?);
        let flag = r.u16()?;
        if flag >> 0xf != 0 {
            blackboard_index_max = index;
            let _ = r.f32()?;
        } else {
            condition_max = r.f32()?;
        }
    }
    Ok(Plug::F32Selector {
        node_index,
        name,
        condition_min,
        blackboard_index_min,
        condition_max,
        blackboard_index_max,
        is_default,
    })
}

fn read_string_selector(r: &mut Reader, is_last: bool) -> Result<Plug> {
    let node_index = r.s32()?;
    let name = r.string_off()?;
    let index = i32::from(r.s16()?);
    let flag = r.u16()?;
    let mut blackboard_index = -1;
    if flag >> 0xf != 0 {
        blackboard_index = index;
    }
    let condition = r.string_off()?;
    let is_default = is_last;
    Ok(Plug::StringSelector {
        node_index,
        name,
        condition,
        is_default,
        blackboard_index,
    })
}

fn read_random_selector(r: &mut Reader) -> Result<Plug> {
    let node_index = r.s32()?;
    let name = r.string_off()?;
    let index = i32::from(r.s16()?);
    let flag = r.u16()?;
    let mut blackboard_index = -1;
    if flag >> 0xf != 0 {
        blackboard_index = index;
    }
    let weight = r.f32()?;
    Ok(Plug::RandomSelector {
        node_index,
        name,
        blackboard_index,
        weight,
    })
}

fn read_bsa_selector_updater(r: &mut Reader) -> Result<Plug> {
    let node_index = r.s32()?;
    let name = r.string_off()?;
    let bb_flag = r.u32()?;
    let mut child_enum_bb_index = -1;
    let mut child_enum_value = 0;
    if (bb_flag >> 0x1f) & 1 != 0 {
        child_enum_bb_index = i32::try_from(bb_flag & 0xffff).unwrap_or(-1);
        let _ = r.u32()?;
    } else {
        child_enum_value = r.u32()?;
    }
    Ok(Plug::BsaSelectorUpdater {
        node_index,
        name,
        child_enum_bb_index,
        child_enum_value,
    })
}

fn read_string_selector_input(r: &mut Reader, version: u32) -> Result<Plug> {
    let node_index = r.s32()?;
    let name = r.string_off()?;
    if version < 0x407 {
        return Ok(Plug::StringSelectorInput {
            node_index,
            name,
            unknown: 0,
            default_value: String::new(),
            read_extra: false,
        });
    }
    Ok(Plug::StringSelectorInput {
        node_index,
        name,
        unknown: r.u32()?,
        default_value: r.string_off()?,
        read_extra: true,
    })
}

fn read_s32_selector_input(r: &mut Reader) -> Result<Plug> {
    let node_index = r.s32()?;
    let name = r.string_off()?;
    if r.version < 0x407 {
        return Ok(Plug::S32SelectorInput {
            node_index,
            name,
            unknown: 0,
            default_value: 0,
            read_extra: false,
        });
    }
    Ok(Plug::S32SelectorInput {
        node_index,
        name,
        unknown: r.u32()?,
        default_value: r.s32()?,
        read_extra: true,
    })
}
