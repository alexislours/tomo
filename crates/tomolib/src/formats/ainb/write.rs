use std::collections::HashMap;

use crate::formats::ainb::Ainb;
use crate::formats::ainb::model::{
    Attachment, BB_PARAM_TYPES, BbParamType, Blackboard, InputParam, Node, OutputParam,
    PARAM_TYPES, PLUG_TYPE_COUNT, ParamSet, ParamSource, ParamType, Plug, Property, PropertySet,
    ReplacementType, Source, Transition, Value,
};
use crate::murmur3_x86_32_seed0;

struct Writer {
    buf: Vec<u8>,
    pool: Vec<String>,
    map: HashMap<String, u32>,
    pool_len: u32,
}

impl Writer {
    fn new() -> Self {
        Self {
            buf: Vec::new(),
            pool: Vec::new(),
            map: HashMap::new(),
            pool_len: 0,
        }
    }

    fn tell(&self) -> usize {
        self.buf.len()
    }
    fn bytes(&mut self, b: &[u8]) {
        self.buf.extend_from_slice(b);
    }
    fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }
    fn u16(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    fn s16(&mut self, v: i16) {
        self.u16(v.cast_unsigned());
    }
    fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    fn s32(&mut self, v: i32) {
        self.u32(v.cast_unsigned());
    }
    fn f32(&mut self, v: f32) {
        self.u32(v.to_bits());
    }
    fn vec3(&mut self, v: [f32; 3]) {
        for c in v {
            self.f32(c);
        }
    }

    fn add_string(&mut self, s: &str) -> u32 {
        if let Some(&off) = self.map.get(s) {
            return off;
        }
        let off = self.pool_len;
        self.map.insert(s.to_owned(), off);
        self.pool.push(s.to_owned());
        self.pool_len += u32::try_from(s.len()).unwrap_or(0) + 1;
        off
    }
    fn string_off(&mut self, s: &str) {
        let off = self.add_string(s);
        self.u32(off);
    }
    fn write_string_pool(&mut self) {
        let pool = std::mem::take(&mut self.pool);
        for s in pool {
            self.buf.extend_from_slice(s.as_bytes());
            self.buf.push(0);
        }
    }
    fn guid(&mut self, g: &str) {
        let parts: Vec<&str> = g.split('-').collect();
        let time_low = u32::from_str_radix(parts.first().copied().unwrap_or("0"), 16).unwrap_or(0);
        let time_mid = u16::from_str_radix(parts.get(1).copied().unwrap_or("0"), 16).unwrap_or(0);
        let time_hi = u16::from_str_radix(parts.get(2).copied().unwrap_or("0"), 16).unwrap_or(0);
        let clock_seq = parts.get(3).copied().unwrap_or("0000");
        let node = parts.get(4).copied().unwrap_or("000000000000");
        self.u32(time_low);
        self.u16(time_mid);
        self.u16(time_hi);
        self.u8(u8::from_str_radix(clock_seq.get(0..2).unwrap_or("0"), 16).unwrap_or(0));
        self.u8(u8::from_str_radix(clock_seq.get(2..4).unwrap_or("0"), 16).unwrap_or(0));
        for i in 0..6 {
            self.u8(u8::from_str_radix(node.get(i * 2..i * 2 + 2).unwrap_or("0"), 16).unwrap_or(0));
        }
    }
}

fn calc_hash(s: &str) -> u32 {
    murmur3_x86_32_seed0(s.as_bytes())
}

#[derive(Default)]
struct Ctx {
    version: u32,
    command_count: u32,
    node_count: u32,
    query_count: u32,
    attachment_count: u32,
    output_count: u32,
    blackboard_offset: usize,
    string_pool_offset: usize,
    enum_resolve_offset: usize,
    property_offset: usize,
    transition_offset: usize,
    io_param_offset: usize,
    multi_param_offset: usize,
    attachment_offset: usize,
    attachment_prop_offset_start: usize,
    attachment_index_offset: usize,
    expression_offset: usize,
    replacement_table_offset: usize,
    query_offset: usize,
    x50_offset: usize,
    x58_offset: usize,
    module_offset: usize,
    action_offset: usize,
    x6c_offset: usize,
    bb_id_offset: usize,

    attachments: Vec<Attachment>,
    attachment_indices: Vec<usize>,
    multi_params: Vec<ParamSource>,
    actions: Vec<(i32, String, String)>,
    node_param_offsets: Vec<usize>,
    node_state_offsets: Vec<usize>,
    transitions: Vec<Transition>,
    node_expression_counts: Vec<u16>,
    node_expression_sizes: Vec<u16>,
    multi_param_counts: Vec<u16>,
    query_base_indices: Vec<u16>,
    base_attachment_indices: Vec<u32>,
    attachment_expression_counts: Vec<u16>,
    attachment_expression_sizes: Vec<u16>,
    queries: Vec<u16>,
    props: PropertySet,
    params: ParamSet,
}

fn build_context(ainb: &Ainb) -> Ctx {
    let mut ctx = Ctx {
        version: ainb.version,
        ..Ctx::default()
    };
    let node_size = if ainb.version > 0x404 { 0x3c } else { 0x38 };
    ctx.command_count = u32::try_from(ainb.commands.len()).unwrap_or(0);
    ctx.node_count = u32::try_from(ainb.nodes.len()).unwrap_or(0);
    ctx.output_count =
        u32::try_from(ainb.nodes.iter().filter(|n| n.ntype.is_output()).count()).unwrap_or(0);
    ctx.query_count =
        u32::try_from(ainb.nodes.iter().filter(|n| n.is_query()).count()).unwrap_or(0);
    ctx.blackboard_offset = 0x74 + 0x18 * ainb.commands.len() + node_size * ainb.nodes.len();

    let query_map = build_query_map(ainb);

    let bb_header_size = if ainb.version >= 0x408 { 0x38 } else { 0x30 };
    let bb_size = ainb
        .blackboard
        .as_ref()
        .map_or(0, |bb| blackboard_size(bb, ainb.version));
    let start = ctx.blackboard_offset + bb_header_size + bb_size;

    let curr_node_param_offset = accumulate_nodes(&mut ctx, ainb, &query_map, start);
    compute_offsets(&mut ctx, ainb, curr_node_param_offset);

    ctx
}

fn build_query_map(ainb: &Ainb) -> HashMap<usize, u16> {
    let mut query_map: HashMap<usize, u16> = HashMap::new();
    let mut curr_query = 0u16;
    for (i, node) in ainb.nodes.iter().enumerate() {
        if node.is_query() {
            query_map.insert(i, curr_query);
            curr_query += 1;
        }
    }
    query_map
}

fn accumulate_nodes(
    ctx: &mut Ctx,
    ainb: &Ainb,
    query_map: &HashMap<usize, u16>,
    start: usize,
) -> usize {
    let mut curr_node_param_offset = start;
    let mut curr_attachment_index = 0u32;
    let mut curr_query_index = 0u16;
    for node in &ainb.nodes {
        ctx.node_param_offsets.push(curr_node_param_offset);
        let plug_bytes: usize = (0..PLUG_TYPE_COUNT)
            .flat_map(|pt| node.plugs[pt].iter())
            .map(|p| p.size() + 4)
            .sum();
        curr_node_param_offset += 0xa4 + plug_bytes;

        for p in &node.plugs[3] {
            if let Plug::Transition { transition, .. } = p {
                ctx.transitions.push(transition.clone());
            }
        }

        let mut multi_count = 0u16;
        for ptype in PARAM_TYPES {
            for prop in node.properties.get(ptype) {
                ctx.props.props[ptype.index()].push(prop.clone());
            }
            for param in node.params.inputs(ptype) {
                ctx.params.inputs[ptype.index()].push(param.clone());
                if let Source::Multi(list) = &param.source {
                    for src in list {
                        ctx.multi_params.push(*src);
                        multi_count += 1;
                    }
                }
            }
            for out in node.params.outputs(ptype) {
                ctx.params.outputs[ptype.index()].push(out.clone());
            }
        }
        ctx.node_expression_counts.push(node.expr_count);
        ctx.node_expression_sizes.push(node.expr_io_size);
        ctx.multi_param_counts.push(multi_count);

        if node.queries.is_empty() {
            ctx.query_base_indices.push(0);
        } else {
            ctx.query_base_indices.push(curr_query_index);
            curr_query_index += u16::try_from(node.queries.len()).unwrap_or(0);
            for &q in &node.queries {
                let qi = usize::try_from(q).unwrap_or(0);
                ctx.queries.push(query_map.get(&qi).copied().unwrap_or(0));
            }
        }

        for action in &node.actions {
            ctx.actions.push((
                node.index,
                action.action_slot.clone(),
                action.action.clone(),
            ));
        }

        ctx.base_attachment_indices.push(curr_attachment_index);
        for attachment in &node.attachments {
            curr_attachment_index += 1;
            if let Some(pos) = ctx.attachments.iter().position(|a| a == attachment) {
                ctx.attachment_indices.push(pos);
            } else {
                ctx.attachment_indices.push(ctx.attachments.len());
                ctx.attachments.push(attachment.clone());
            }
        }
    }

    for attachment in &ctx.attachments {
        ctx.attachment_expression_counts.push(attachment.expr_count);
        ctx.attachment_expression_sizes
            .push(attachment.expr_io_size);
        for ptype in PARAM_TYPES {
            for prop in attachment.properties.get(ptype) {
                ctx.props.props[ptype.index()].push(prop.clone());
            }
        }
    }

    curr_node_param_offset
}

fn compute_offsets(ctx: &mut Ctx, ainb: &Ainb, curr_node_param_offset: usize) {
    let attachment_size = if ainb.version > 0x404 { 0x10 } else { 0xc };
    ctx.attachment_count = u32::try_from(ctx.attachments.len()).unwrap_or(0);
    ctx.attachment_index_offset = curr_node_param_offset;
    ctx.attachment_offset = ctx.attachment_index_offset + 4 * ctx.attachment_indices.len();
    ctx.attachment_prop_offset_start =
        ctx.attachment_offset + attachment_size * ctx.attachments.len();
    ctx.property_offset = ctx.attachment_prop_offset_start + 0x64 * ctx.attachments.len();

    let prop_bytes: usize = PARAM_TYPES
        .into_iter()
        .map(|pt| ctx.props.get(pt).len() * pt.property_size())
        .sum();
    ctx.io_param_offset = ctx.property_offset + 0x18 + prop_bytes;

    let input_bytes: usize = PARAM_TYPES
        .into_iter()
        .map(|pt| ctx.params.inputs(pt).len() * pt.input_size())
        .sum();
    let output_bytes: usize = PARAM_TYPES
        .into_iter()
        .map(|pt| ctx.params.outputs(pt).len() * pt.output_size())
        .sum();
    ctx.multi_param_offset = ctx.io_param_offset + 0x30 + input_bytes + output_bytes;

    ctx.x50_offset = ctx.multi_param_offset + 8 * ctx.multi_params.len();
    ctx.transition_offset = ctx.x50_offset;
    let trans_bytes: usize = ctx
        .transitions
        .iter()
        .map(|t| 4 + if t.transition_type == 0 { 8 } else { 4 })
        .sum();
    ctx.query_offset = ctx.transition_offset + trans_bytes;

    if let Some(exb) = &ainb.expressions {
        ctx.expression_offset = ctx.query_offset + 4 * ctx.queries.len();
        ctx.module_offset = ctx.expression_offset + exb.len();
    } else {
        ctx.expression_offset = 0;
        ctx.module_offset = ctx.query_offset + 4 * ctx.queries.len();
    }
    ctx.action_offset = ctx.module_offset + 4 + 0xc * ainb.modules.len();
    ctx.bb_id_offset = ctx.action_offset + 4 + 0xc * ctx.actions.len();

    let state_offset = if ainb.unk_section0x58.is_some() {
        ctx.x58_offset = ctx.bb_id_offset + 8;
        ctx.x58_offset + 0x10
    } else {
        ctx.x58_offset = 0;
        ctx.bb_id_offset + 8
    };

    if ainb.version < 0x407 {
        ctx.node_state_offsets = (0..ainb.nodes.len())
            .map(|i| state_offset + 0x14 * i)
            .collect();
        let state_end = state_offset + 0x14 * ainb.nodes.len();
        ctx.replacement_table_offset = 0;
        if ainb.exists_section_0x6c {
            ctx.x6c_offset = state_end;
            ctx.enum_resolve_offset = ctx.x6c_offset + 4;
        } else {
            ctx.x6c_offset = 0;
            ctx.enum_resolve_offset = state_end;
        }
    } else {
        ctx.replacement_table_offset = state_offset;
        let rep_end = ctx.replacement_table_offset + 8 + 8 * ainb.replacement_table.len();
        if ainb.exists_section_0x6c {
            ctx.x6c_offset = rep_end;
            ctx.enum_resolve_offset = ctx.x6c_offset + 4;
        } else {
            ctx.x6c_offset = 0;
            ctx.enum_resolve_offset = rep_end;
        }
    }
    ctx.string_pool_offset = ctx.enum_resolve_offset + 4;
}

pub(super) fn write(ainb: &Ainb) -> Vec<u8> {
    let ctx = build_context(ainb);
    let mut w = Writer::new();

    write_header(&mut w, ainb, &ctx);

    for cmd in &ainb.commands {
        w.string_off(&cmd.name);
        w.guid(&cmd.guid);
        w.u16(u16::try_from(cmd.root_node_index.max(0)).unwrap_or(0));
        w.u16(u16::try_from((cmd.secondary_root_node_index + 1).max(0)).unwrap_or(0));
    }

    for (i, node) in ainb.nodes.iter().enumerate() {
        write_node(&mut w, node, &ctx, i);
    }

    if let Some(bb) = &ainb.blackboard {
        write_blackboard(&mut w, bb, ainb.version);
    } else {
        let bb_header_size = if ainb.version >= 0x408 { 0x38 } else { 0x30 };
        w.bytes(&vec![0u8; bb_header_size]);
    }

    let mut prop_indices = [0u32; 6];
    let mut input_indices = [0u32; 6];
    let mut output_indices = [0u32; 6];
    for node in &ainb.nodes {
        write_node_params(
            &mut w,
            node,
            &ctx.transitions,
            &mut prop_indices,
            &mut input_indices,
            &mut output_indices,
        );
    }

    for &i in &ctx.attachment_indices {
        w.u32(u32::try_from(i).unwrap_or(0));
    }

    write_attachments(&mut w, ainb, &ctx, &mut prop_indices);

    write_property_set(&mut w, &ctx.props);
    write_param_set(&mut w, &ctx.params, &ctx.multi_params);

    for src in &ctx.multi_params {
        w.s16(i16::try_from(src.src_node_index).unwrap_or(0));
        w.s16(i16::try_from(src.src_output_index).unwrap_or(0));
        w.u32(src.flags);
    }

    write_transitions(&mut w, &ctx);

    for &q in &ctx.queries {
        w.u16(q);
        w.u16(0);
    }

    if let Some(exb) = &ainb.expressions {
        w.bytes(exb.raw());
    }

    w.u32(u32::try_from(ainb.modules.len()).unwrap_or(0));
    for m in &ainb.modules {
        w.string_off(&m.path);
        w.string_off(&m.category);
        w.u32(m.instance_count);
    }

    w.u32(u32::try_from(ctx.actions.len()).unwrap_or(0));
    for (index, slot, action) in &ctx.actions {
        w.s32(*index);
        w.string_off(slot);
        w.string_off(action);
    }

    w.u32(ainb.blackboard_id);
    w.u32(ainb.parent_blackboard_id);

    if let Some(unk) = &ainb.unk_section0x58 {
        w.string_off(&unk.description);
        w.u32(unk.unk04);
        w.u32(unk.unk08);
        w.u32(unk.unk0c);
    }

    write_node_states(&mut w, ainb);
    write_replacement_table(&mut w, ainb, &ctx);

    if ainb.exists_section_0x6c {
        w.u32(0);
    }

    w.u32(0);
    w.write_string_pool();
    w.buf
}

fn write_attachments(w: &mut Writer, ainb: &Ainb, ctx: &Ctx, prop_indices: &mut [u32; 6]) {
    let attach_size = if ainb.version > 0x404 { 0x10 } else { 0xc };
    let mut param_offset = w.tell() + attach_size * ctx.attachments.len();
    for (i, attachment) in ctx.attachments.iter().enumerate() {
        w.string_off(&attachment.name);
        w.u32(u32::try_from(param_offset).unwrap_or(0));
        w.u16(ctx.attachment_expression_counts[i]);
        w.u16(ctx.attachment_expression_sizes[i]);
        if ainb.version > 0x404 {
            w.u32(calc_hash(&attachment.name));
        }
        param_offset += 0x64;
    }
    for attachment in &ctx.attachments {
        write_attachment_params(w, attachment, prop_indices);
    }
}

fn write_transitions(w: &mut Writer, ctx: &Ctx) {
    let mut trans_offset = w.tell() + 4 * ctx.transitions.len();
    for t in &ctx.transitions {
        w.u32(u32::try_from(trans_offset).unwrap_or(0));
        trans_offset += if t.transition_type == 0 { 8 } else { 4 };
    }
    for t in &ctx.transitions {
        if t.update_post_calc {
            w.u32(t.transition_type | 0x8000_0000);
        } else {
            w.u32(t.transition_type);
        }
        if t.transition_type == 0 {
            w.string_off(&t.command_name);
        }
    }
}

fn write_node_states(w: &mut Writer, ainb: &Ainb) {
    if ainb.version >= 0x407 {
        return;
    }
    for node in &ainb.nodes {
        if let Some(s) = &node.state_info {
            w.string_off(&s.desired_state);
            w.u32(s.unk04);
            w.u32(s.unk08);
            w.u32(s.unk0c);
            w.u32(s.unk10);
        } else {
            w.u32(0);
            w.u32(0);
            w.u32(0);
            w.u32(0);
            w.u32(0);
        }
    }
}

fn write_replacement_table(w: &mut Writer, ainb: &Ainb, ctx: &Ctx) {
    if ainb.version <= 0x404 {
        return;
    }
    w.u16(0);
    w.u16(u16::try_from(ainb.replacement_table.len()).unwrap_or(0));
    let mut exist_node = false;
    let mut exist_attach = false;
    let mut new_attach = i32::try_from(ctx.attachment_count).unwrap_or(0);
    let mut new_node = i32::try_from(ctx.node_count).unwrap_or(0);
    for rep in &ainb.replacement_table {
        if rep.rtype == ReplacementType::RemoveAttachment {
            exist_attach = true;
            new_attach -= 1;
        } else {
            exist_node = true;
            new_node -= if rep.rtype == ReplacementType::RemoveChild {
                1
            } else {
                2
            };
        }
    }
    w.s16(if exist_node {
        i16::try_from(new_node).unwrap_or(-1)
    } else {
        -1
    });
    w.s16(if exist_attach {
        i16::try_from(new_attach).unwrap_or(-1)
    } else {
        -1
    });
    for rep in &ainb.replacement_table {
        w.u8(u8::try_from(rep.rtype.value() & 0xff).unwrap_or(0));
        w.u8(0);
        w.s16(i16::try_from(rep.node_index).unwrap_or(0));
        w.s16(i16::try_from(rep.replace_index).unwrap_or(0));
        w.s16(i16::try_from(rep.new_index).unwrap_or(0));
    }
}

fn write_header(w: &mut Writer, ainb: &Ainb, ctx: &Ctx) {
    w.bytes(b"AIB ");
    w.u32(ainb.version);
    w.string_off(&ainb.filename);
    w.u32(ctx.command_count);
    w.u32(ctx.node_count);
    w.u32(ctx.query_count);
    w.u32(ctx.attachment_count);
    w.u32(ctx.output_count);
    w.u32(u32::try_from(ctx.blackboard_offset).unwrap_or(0));
    w.u32(u32::try_from(ctx.string_pool_offset).unwrap_or(0));
    w.u32(u32::try_from(ctx.enum_resolve_offset).unwrap_or(0));
    w.u32(u32::try_from(ctx.property_offset).unwrap_or(0));
    w.u32(u32::try_from(ctx.transition_offset).unwrap_or(0));
    w.u32(u32::try_from(ctx.io_param_offset).unwrap_or(0));
    w.u32(u32::try_from(ctx.multi_param_offset).unwrap_or(0));
    w.u32(u32::try_from(ctx.attachment_offset).unwrap_or(0));
    w.u32(u32::try_from(ctx.attachment_index_offset).unwrap_or(0));
    w.u32(u32::try_from(ctx.expression_offset).unwrap_or(0));
    w.u32(u32::try_from(ctx.replacement_table_offset).unwrap_or(0));
    w.u32(u32::try_from(ctx.query_offset).unwrap_or(0));
    w.u32(u32::try_from(ctx.x50_offset).unwrap_or(0));
    w.u32(0);
    w.u32(u32::try_from(ctx.x58_offset).unwrap_or(0));
    w.u32(u32::try_from(ctx.module_offset).unwrap_or(0));
    w.string_off(&ainb.category);
    if ainb.version > 0x404 {
        w.u32(ainb.category_id().unwrap_or(0));
    } else {
        w.u32(0);
    }
    w.u32(u32::try_from(ctx.action_offset).unwrap_or(0));
    w.u32(u32::try_from(ctx.x6c_offset).unwrap_or(0));
    w.u32(u32::try_from(ctx.bb_id_offset).unwrap_or(0));
}

fn write_node(w: &mut Writer, node: &Node, ctx: &Ctx, index: usize) {
    w.u16(node.ntype.value());
    w.s16(i16::try_from(index).unwrap_or(0));
    w.u16(u16::try_from(node.attachments.len()).unwrap_or(0));
    w.u8(node.flags);
    w.u8(0);
    w.string_off(&node.name);
    if ctx.version > 0x404 {
        w.u32(calc_hash(&node.name));
    }
    w.u32(0);
    w.u32(u32::try_from(ctx.node_param_offsets[index]).unwrap_or(0));
    w.u16(ctx.node_expression_counts[index]);
    w.u16(ctx.node_expression_sizes[index]);
    w.u16(ctx.multi_param_counts[index]);
    w.u16(0);
    w.u32(ctx.base_attachment_indices[index]);
    w.u16(ctx.query_base_indices[index]);
    w.u16(u16::try_from(node.queries.len()).unwrap_or(0));
    if ctx.version < 0x407 {
        w.u32(u32::try_from(ctx.node_state_offsets[index]).unwrap_or(0));
    } else {
        w.u32(0);
    }
    w.guid(&node.guid);
}

fn write_node_params(
    w: &mut Writer,
    node: &Node,
    transitions: &[Transition],
    prop_indices: &mut [u32; 6],
    input_indices: &mut [u32; 6],
    output_indices: &mut [u32; 6],
) {
    for ptype in PARAM_TYPES {
        let idx = ptype.index();
        let count = u32::try_from(node.properties.get(ptype).len()).unwrap_or(0);
        w.u32(prop_indices[idx]);
        prop_indices[idx] += count;
        w.u32(count);
    }
    for ptype in PARAM_TYPES {
        let idx = ptype.index();
        let ic = u32::try_from(node.params.inputs(ptype).len()).unwrap_or(0);
        w.u32(input_indices[idx]);
        input_indices[idx] += ic;
        w.u32(ic);
        w.u32(output_indices[idx]);
        let oc = u32::try_from(node.params.outputs(ptype).len()).unwrap_or(0);
        output_indices[idx] += oc;
        w.u32(oc);
    }
    let mut curr_index = 0u8;
    for pt in 0..PLUG_TYPE_COUNT {
        let count = u8::try_from(node.plugs[pt].len()).unwrap_or(0);
        w.u8(count);
        w.u8(curr_index);
        curr_index = curr_index.wrapping_add(count);
    }
    let mut curr_offset = w.tell() + usize::from(curr_index) * 4;
    for pt in 0..PLUG_TYPE_COUNT {
        for plug in &node.plugs[pt] {
            w.u32(u32::try_from(curr_offset).unwrap_or(0));
            curr_offset += plug.size();
        }
    }
    for pt in 0..PLUG_TYPE_COUNT {
        for plug in &node.plugs[pt] {
            write_plug(w, plug, transitions);
        }
    }
}

fn write_bb_index(w: &mut Writer, blackboard_index: i32) {
    if blackboard_index == -1 {
        w.u32(0);
    } else {
        w.s16(i16::try_from(blackboard_index).unwrap_or(0));
        w.u16(0x8000);
    }
}

fn write_bb_cond_f32(w: &mut Writer, blackboard_index: i32, condition: f32) {
    if blackboard_index == -1 {
        w.u32(0);
        w.f32(condition);
    } else {
        w.s16(i16::try_from(blackboard_index).unwrap_or(0));
        w.u16(0x8000);
        w.u32(0);
    }
}

fn write_plug(w: &mut Writer, plug: &Plug, transitions: &[Transition]) {
    match plug {
        Plug::Generic { node_index, name } | Plug::Child { node_index, name } => {
            w.s32(*node_index);
            w.string_off(name);
        }
        Plug::BoolSelectorInput {
            node_index,
            name,
            unk0,
            unk1,
        } => {
            w.s32(*node_index);
            w.string_off(name);
            w.u32(*unk0);
            w.u32(*unk1);
        }
        Plug::F32SelectorInput {
            node_index,
            name,
            unk0,
            unk1,
        } => {
            w.s32(*node_index);
            w.string_off(name);
            w.u32(*unk0);
            w.f32(*unk1);
        }
        Plug::Transition {
            node_index,
            transition,
        } => {
            w.s32(*node_index);
            let idx = transitions
                .iter()
                .position(|t| t == transition)
                .unwrap_or(0);
            w.u32(u32::try_from(idx).unwrap_or(0));
        }
        Plug::StringSelectorInput {
            node_index,
            name,
            unknown,
            default_value,
            read_extra,
        } => {
            w.s32(*node_index);
            w.string_off(name);
            if *read_extra {
                w.u32(*unknown);
                w.string_off(default_value);
            }
        }
        Plug::S32SelectorInput {
            node_index,
            name,
            unknown,
            default_value,
            read_extra,
        } => {
            w.s32(*node_index);
            w.string_off(name);
            if *read_extra {
                w.u32(*unknown);
                w.s32(*default_value);
            }
        }
        other => write_selector_plug(w, other),
    }
}

fn write_selector_plug(w: &mut Writer, plug: &Plug) {
    match plug {
        Plug::S32Selector {
            node_index,
            name,
            condition,
            is_default,
            blackboard_index,
        } => {
            w.s32(*node_index);
            w.string_off(name);
            write_bb_index(w, *blackboard_index);
            if *is_default {
                w.u32(0);
            } else {
                w.s32(*condition);
            }
        }
        Plug::F32Selector {
            node_index,
            name,
            condition_min,
            blackboard_index_min,
            condition_max,
            blackboard_index_max,
            is_default,
        } => {
            w.s32(*node_index);
            w.string_off(name);
            if *is_default {
                w.bytes(&[0u8; 0x20]);
            } else {
                write_bb_cond_f32(w, *blackboard_index_min, *condition_min);
                write_bb_cond_f32(w, *blackboard_index_max, *condition_max);
                w.bytes(&[0u8; 0x10]);
            }
        }
        Plug::StringSelector {
            node_index,
            name,
            condition,
            is_default: _,
            blackboard_index,
        } => {
            w.s32(*node_index);
            w.string_off(name);
            write_bb_index(w, *blackboard_index);
            w.string_off(condition);
        }
        Plug::RandomSelector {
            node_index,
            name,
            blackboard_index,
            weight,
        } => {
            w.s32(*node_index);
            w.string_off(name);
            write_bb_index(w, *blackboard_index);
            w.f32(*weight);
        }
        Plug::BsaSelectorUpdater {
            node_index,
            name,
            child_enum_bb_index,
            child_enum_value,
        } => {
            w.s32(*node_index);
            w.string_off(name);
            if *child_enum_bb_index < 0 {
                w.u32(0);
                w.u32(*child_enum_value);
            } else {
                w.u32(u32::try_from(*child_enum_bb_index).unwrap_or(0) | 0x8000_0000);
                w.u32(0);
            }
        }
        _ => unreachable!(),
    }
}

fn write_property_set(w: &mut Writer, set: &PropertySet) {
    let mut base = w.tell() + 0x18;
    for ptype in PARAM_TYPES {
        w.u32(u32::try_from(base).unwrap_or(0));
        base += ptype.property_size() * set.get(ptype).len();
    }
    for ptype in PARAM_TYPES {
        for prop in set.get(ptype) {
            write_property(w, prop, ptype);
        }
    }
}

fn write_property(w: &mut Writer, prop: &Property, ptype: ParamType) {
    w.string_off(&prop.name);
    if ptype == ParamType::Pointer {
        w.string_off(&prop.classname);
    }
    w.u32(prop.flags);
    write_value(w, &prop.value, ptype);
}

fn write_value(w: &mut Writer, value: &Value, ptype: ParamType) {
    match ptype {
        ParamType::Int => {
            if let Value::Int(v) = value {
                w.s32(*v);
            } else {
                w.s32(0);
            }
        }
        ParamType::Bool => w.u32(u32::from(matches!(value, Value::Bool(true)))),
        ParamType::Float => {
            if let Value::Float(v) = value {
                w.f32(*v);
            } else {
                w.f32(0.0);
            }
        }
        ParamType::String => {
            if let Value::Str(s) = value {
                w.string_off(s);
            } else {
                w.string_off("");
            }
        }
        ParamType::Vector3F => {
            if let Value::Vec3(v) = value {
                w.vec3(*v);
            } else {
                w.vec3([0.0; 3]);
            }
        }
        ParamType::Pointer => {}
    }
}

fn write_param_set(w: &mut Writer, set: &ParamSet, multi: &[ParamSource]) {
    let mut offset = w.tell() + 0x30;
    for ptype in PARAM_TYPES {
        w.u32(u32::try_from(offset).unwrap_or(0));
        offset += set.inputs(ptype).len() * ptype.input_size();
        w.u32(u32::try_from(offset).unwrap_or(0));
        offset += set.outputs(ptype).len() * ptype.output_size();
    }
    for ptype in PARAM_TYPES {
        for input in set.inputs(ptype) {
            write_input(w, input, ptype, multi);
        }
        for out in set.outputs(ptype) {
            write_output(w, out, ptype);
        }
    }
}

fn write_input(w: &mut Writer, input: &InputParam, ptype: ParamType, multi: &[ParamSource]) {
    w.string_off(&input.name);
    if ptype == ParamType::Pointer {
        w.string_off(&input.classname);
    }
    match &input.source {
        Source::Multi(list) => {
            let count = list.len();
            let mut node_index = -1i32;
            if count <= multi.len() {
                for i in 0..=multi.len() - count {
                    if &multi[i..i + count] == list.as_slice() {
                        node_index = -100 - i32::try_from(i).unwrap_or(0);
                        break;
                    }
                }
            }
            w.s16(i16::try_from(node_index).unwrap_or(-1));
            w.s16(i16::try_from(count).unwrap_or(0));
            w.u32(0);
        }
        Source::Single(src) => {
            w.s16(i16::try_from(src.src_node_index).unwrap_or(0));
            if input.is_blackboard_input {
                w.u16(u16::try_from(src.src_output_index).unwrap_or(0) | 0x8000);
            } else {
                w.s16(i16::try_from(src.src_output_index).unwrap_or(0));
            }
            w.u32(src.flags);
        }
    }
    if ptype == ParamType::Pointer {
        w.u32(0);
    } else {
        write_value(w, &input.value, ptype);
    }
}

fn write_output(w: &mut Writer, out: &OutputParam, ptype: ParamType) {
    let off = w.add_string(&out.name);
    if out.is_output {
        w.u32(off | 0x8000_0000);
    } else {
        w.u32(off);
    }
    if ptype == ParamType::Pointer {
        w.string_off(&out.classname);
    }
}

fn write_attachment_params(w: &mut Writer, attachment: &Attachment, prop_indices: &mut [u32; 6]) {
    w.u32(attachment.debug);
    for ptype in PARAM_TYPES {
        let idx = ptype.index();
        let count = u32::try_from(attachment.properties.get(ptype).len()).unwrap_or(0);
        w.u32(prop_indices[idx]);
        w.u32(count);
        prop_indices[idx] += count;
    }
    let offset = w.tell() + 0x30;
    for _ in 0..6 {
        w.u32(0);
        w.u32(u32::try_from(offset).unwrap_or(0));
    }
}

fn blackboard_size(bb: &Blackboard, version: u32) -> usize {
    let mut file_refs: Vec<&str> = Vec::new();
    let mut size = 0;
    for ptype in BB_PARAM_TYPES {
        if !ptype.is_supported(version) {
            continue;
        }
        for param in &bb.params[ptype.index()] {
            let vsize = ptype.value_size();
            if !param.file_ref.is_empty() && !file_refs.contains(&param.file_ref.as_str()) {
                file_refs.push(&param.file_ref);
                size += 0x18 + vsize;
            } else {
                size += 8 + vsize;
            }
        }
    }
    size
}

fn write_blackboard(w: &mut Writer, bb: &Blackboard, version: u32) {
    let mut index = 0u16;
    let mut pos = 0u16;
    for ptype in BB_PARAM_TYPES {
        if !ptype.is_supported(version) {
            continue;
        }
        let count = u16::try_from(bb.params[ptype.index()].len()).unwrap_or(0);
        w.u16(count);
        w.u16(index);
        index += count;
        w.u16(pos);
        match ptype {
            BbParamType::Vec3f => pos += 0xc * count,
            BbParamType::VoidPtr => {}
            _ => pos += 4 * count,
        }
        w.u16(0);
    }
    let mut file_refs: Vec<String> = Vec::new();
    for ptype in BB_PARAM_TYPES {
        if !ptype.is_supported(version) {
            continue;
        }
        for param in &bb.params[ptype.index()] {
            let mut name_off = w.add_string(&param.name);
            if param.file_ref.is_empty() {
                name_off |= u32::from(param.flags) << 0x16;
            } else {
                if !file_refs.contains(&param.file_ref) {
                    file_refs.push(param.file_ref.clone());
                }
                let fr_idx = u32::try_from(
                    file_refs
                        .iter()
                        .position(|f| f == &param.file_ref)
                        .unwrap_or(0),
                )
                .unwrap_or(0);
                name_off |= (1 << 0x1f) | (fr_idx << 0x18) | (u32::from(param.flags) << 0x16);
            }
            w.u32(name_off);
            w.string_off(&param.notes);
        }
    }
    for ptype in BB_PARAM_TYPES {
        if !ptype.is_supported(version) {
            continue;
        }
        for param in &bb.params[ptype.index()] {
            write_bb_value(w, &param.value, ptype);
        }
    }
    for fr in &file_refs {
        w.string_off(fr);
        w.u32(calc_hash(fr));
        w.u32(calc_hash(file_stem(fr)));
        w.u32(calc_hash(&file_ext(fr)));
    }
}

fn write_bb_value(w: &mut Writer, value: &Value, ptype: BbParamType) {
    match ptype {
        BbParamType::String => {
            if let Value::Str(s) = value {
                w.string_off(s);
            } else {
                w.string_off("");
            }
        }
        BbParamType::S32 => {
            if let Value::Int(v) = value {
                w.s32(*v);
            } else {
                w.s32(0);
            }
        }
        BbParamType::U32 => {
            if let Value::UInt(v) = value {
                w.u32(*v);
            } else {
                w.u32(0);
            }
        }
        BbParamType::F32 => {
            if let Value::Float(v) = value {
                w.f32(*v);
            } else {
                w.f32(0.0);
            }
        }
        BbParamType::Bool => w.u32(u32::from(matches!(value, Value::Bool(true)))),
        BbParamType::Vec3f => {
            if let Value::Vec3(v) = value {
                w.vec3(*v);
            } else {
                w.vec3([0.0; 3]);
            }
        }
        BbParamType::VoidPtr => {}
    }
}

fn file_stem(path: &str) -> &str {
    let base = path.rsplit('/').next().unwrap_or(path);
    match base.rfind('.') {
        Some(0) | None => base,
        Some(i) => &base[..i],
    }
}

fn file_ext(path: &str) -> String {
    let base = path.rsplit('/').next().unwrap_or(path);
    match base.rfind('.') {
        Some(0) | None => String::new(),
        Some(i) => base[i + 1..].to_string(),
    }
}
