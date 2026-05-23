use std::fmt::Write as _;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use saphyr::{LoadableYamlNode, Scalar, Yaml};

use crate::formats::ainb::model::{
    Action, Attachment, BB_PARAM_TYPES, BbParam, BbParamType, Blackboard, Command, InputParam,
    Module, Node, NodeType, OutputParam, PARAM_TYPES, PLUG_TYPE_NAMES, ParamSet, ParamSource,
    ParamType, Plug, Property, PropertySet, ReplacementEntry, ReplacementType, Source, StateInfo,
    Transition, UnknownSection0x58, Value, flag_index, flag_is_output, flag_uses_default,
    flag_vector_component, is_blackboard_flag, is_expression_flag,
};
use crate::formats::ainb::{Ainb, Exb};
use crate::{Error, Result};

enum Y {
    Map(Vec<(String, Y)>),
    Seq(Vec<Y>),
    Str(String),
    Int(i64),
    Float(f32),
    Bool(bool),
    Null,
}

fn vc_name(c: u32) -> &'static str {
    match c {
        1 => "X",
        2 => "Y",
        3 => "Z",
        _ => "NONE",
    }
}

fn vc_value(name: &str) -> u32 {
    match name {
        "X" => 1,
        "Y" => 2,
        "Z" => 3,
        _ => 0,
    }
}

fn value_to_y(v: &Value) -> Y {
    match v {
        Value::Int(i) => Y::Int(i64::from(*i)),
        Value::UInt(u) => Y::Int(i64::from(*u)),
        Value::Bool(b) => Y::Bool(*b),
        Value::Float(f) => Y::Float(*f),
        Value::Str(s) => Y::Str(s.clone()),
        Value::Vec3(v) => Y::Seq(vec![Y::Float(v[0]), Y::Float(v[1]), Y::Float(v[2])]),
        Value::Null => Y::Null,
    }
}

fn flags_into(map: &mut Vec<(String, Y)>, flags: u32) {
    let mut list = Vec::new();
    if flag_uses_default(flags) {
        list.push(Y::Str("uses_default".into()));
    }
    if flag_is_output(flags) {
        list.push(Y::Str("is_output".into()));
    }
    map.push(("flags".into(), Y::Seq(list)));
    if is_expression_flag(flags) {
        map.push((
            "expression_index".into(),
            Y::Int(i64::from(flag_index(flags))),
        ));
    } else if is_blackboard_flag(flags) {
        map.push((
            "blackboard_index".into(),
            Y::Int(i64::from(flag_index(flags))),
        ));
        let comp = flag_vector_component(flags);
        if comp != 0 {
            map.push(("vector_component".into(), Y::Str(vc_name(comp).into())));
        }
    }
}

fn property_to_y(p: &Property) -> Y {
    let mut m = vec![("name".into(), Y::Str(p.name.clone()))];
    if p.ptype == ParamType::Pointer {
        m.push(("classname".into(), Y::Str(p.classname.clone())));
    }
    m.push(("default_value".into(), value_to_y(&p.value)));
    flags_into(&mut m, p.flags);
    Y::Map(m)
}

fn property_set_to_y(set: &PropertySet) -> Y {
    let mut m = Vec::new();
    for pt in PARAM_TYPES {
        let v = set.get(pt);
        if !v.is_empty() {
            m.push((
                pt.name().into(),
                Y::Seq(v.iter().map(property_to_y).collect()),
            ));
        }
    }
    Y::Map(m)
}

fn source_to_y(map: &mut Vec<(String, Y)>, src: &ParamSource) {
    map.push(("node_index".into(), Y::Int(i64::from(src.src_node_index))));
    map.push((
        "output_index".into(),
        Y::Int(i64::from(src.src_output_index)),
    ));
    flags_into(map, src.flags);
}

fn input_to_y(p: &InputParam) -> Y {
    let mut m = vec![("name".into(), Y::Str(p.name.clone()))];
    if p.ptype == ParamType::Pointer {
        m.push(("classname".into(), Y::Str(p.classname.clone())));
    }
    m.push(("default_value".into(), value_to_y(&p.value)));
    match &p.source {
        Source::Single(src) => source_to_y(&mut m, src),
        Source::Multi(list) => {
            let mut sources = Vec::new();
            for src in list {
                let mut sm = Vec::new();
                source_to_y(&mut sm, src);
                sources.push(Y::Map(sm));
            }
            m.push(("sources".into(), Y::Seq(sources)));
        }
    }
    if p.is_blackboard_input {
        m.push(("is_set_blackboard".into(), Y::Bool(true)));
    }
    Y::Map(m)
}

fn output_to_y(p: &OutputParam) -> Y {
    let mut m = vec![("name".into(), Y::Str(p.name.clone()))];
    if p.ptype == ParamType::Pointer {
        m.push(("classname".into(), Y::Str(p.classname.clone())));
    }
    m.push(("is_output".into(), Y::Bool(p.is_output)));
    Y::Map(m)
}

fn param_set_to_y(set: &ParamSet) -> Y {
    let mut inputs = Vec::new();
    let mut outputs = Vec::new();
    for pt in PARAM_TYPES {
        if !set.inputs(pt).is_empty() {
            inputs.push((
                pt.name().into(),
                Y::Seq(set.inputs(pt).iter().map(input_to_y).collect()),
            ));
        }
        if !set.outputs(pt).is_empty() {
            outputs.push((
                pt.name().into(),
                Y::Seq(set.outputs(pt).iter().map(output_to_y).collect()),
            ));
        }
    }
    Y::Map(vec![
        ("inputs".into(), Y::Map(inputs)),
        ("outputs".into(), Y::Map(outputs)),
    ])
}

fn plug_to_y(p: &Plug) -> Y {
    let mut m: Vec<(String, Y)> = Vec::new();
    match p {
        Plug::Generic { node_index, name } | Plug::Child { node_index, name } => {
            m.push(("node_index".into(), Y::Int(i64::from(*node_index))));
            m.push(("name".into(), Y::Str(name.clone())));
        }
        Plug::BoolSelectorInput {
            node_index,
            name,
            unk0,
            unk1,
        } => {
            m.push(("node_index".into(), Y::Int(i64::from(*node_index))));
            m.push(("name".into(), Y::Str(name.clone())));
            m.push(("unknown_1".into(), Y::Int(i64::from(*unk0))));
            m.push(("unknown_2".into(), Y::Int(i64::from(*unk1))));
        }
        Plug::F32SelectorInput {
            node_index,
            name,
            unk0,
            unk1,
        } => {
            m.push(("node_index".into(), Y::Int(i64::from(*node_index))));
            m.push(("name".into(), Y::Str(name.clone())));
            m.push(("unknown_1".into(), Y::Int(i64::from(*unk0))));
            m.push(("unknown_2".into(), Y::Float(*unk1)));
        }
        Plug::Transition {
            node_index,
            transition,
        } => {
            m.push(("node_index".into(), Y::Int(i64::from(*node_index))));
            m.push((
                "transition_type".into(),
                Y::Int(i64::from(transition.transition_type)),
            ));
            m.push((
                "update_post_calc".into(),
                Y::Bool(transition.update_post_calc),
            ));
            if transition.transition_type == 0 {
                m.push((
                    "transition_name".into(),
                    Y::Str(transition.command_name.clone()),
                ));
            }
        }
        Plug::StringSelectorInput {
            node_index,
            name,
            unknown,
            default_value,
            read_extra,
        } => {
            m.push(("node_index".into(), Y::Int(i64::from(*node_index))));
            m.push(("name".into(), Y::Str(name.clone())));
            if *read_extra {
                m.push(("unknown".into(), Y::Int(i64::from(*unknown))));
                m.push(("default_value".into(), Y::Str(default_value.clone())));
            }
        }
        Plug::S32SelectorInput {
            node_index,
            name,
            unknown,
            default_value,
            read_extra,
        } => {
            m.push(("node_index".into(), Y::Int(i64::from(*node_index))));
            m.push(("name".into(), Y::Str(name.clone())));
            if *read_extra {
                m.push(("unknown".into(), Y::Int(i64::from(*unknown))));
                m.push(("default_value".into(), Y::Int(i64::from(*default_value))));
            }
        }
        other => selector_plug_to_y(other, &mut m),
    }
    Y::Map(m)
}

fn push_node_name(m: &mut Vec<(String, Y)>, node_index: i32, name: &str) {
    m.push(("node_index".into(), Y::Int(i64::from(node_index))));
    m.push(("name".into(), Y::Str(name.to_owned())));
}

fn push_int(m: &mut Vec<(String, Y)>, key: &str, v: impl Into<i64>) {
    m.push((key.into(), Y::Int(v.into())));
}

fn selector_plug_to_y(p: &Plug, m: &mut Vec<(String, Y)>) {
    match p {
        Plug::S32Selector {
            node_index,
            name,
            condition,
            is_default,
            blackboard_index,
        } => {
            push_node_name(m, *node_index, name);
            if *is_default {
                m.push(("is_default".into(), Y::Bool(true)));
            } else if *blackboard_index == -1 {
                push_int(m, "condition", *condition);
            } else {
                push_int(m, "blackboard_index", *blackboard_index);
                push_int(m, "default_condition", *condition);
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
            push_node_name(m, *node_index, name);
            if *is_default {
                m.push(("is_default".into(), Y::Bool(true)));
            } else {
                if *blackboard_index_min == -1 {
                    m.push(("condition_min".into(), Y::Float(*condition_min)));
                } else {
                    push_int(m, "condition_min_blackboard_index", *blackboard_index_min);
                }
                if *blackboard_index_max == -1 {
                    m.push(("condition_max".into(), Y::Float(*condition_max)));
                } else {
                    push_int(m, "condition_max_blackboard_index", *blackboard_index_max);
                }
            }
        }
        Plug::StringSelector {
            node_index,
            name,
            condition,
            is_default,
            blackboard_index,
        } => {
            push_node_name(m, *node_index, name);
            if *is_default {
                m.push(("is_default".into(), Y::Bool(true)));
                m.push(("condition".into(), Y::Str(condition.clone())));
            } else if *blackboard_index == -1 {
                m.push(("condition".into(), Y::Str(condition.clone())));
            } else {
                push_int(m, "blackboard_index", *blackboard_index);
                m.push(("default_condition".into(), Y::Str(condition.clone())));
            }
        }
        Plug::RandomSelector {
            node_index,
            name,
            blackboard_index,
            weight,
        } => {
            push_node_name(m, *node_index, name);
            if *blackboard_index == -1 {
                m.push(("weight".into(), Y::Float(*weight)));
            } else {
                push_int(m, "blackboard_index", *blackboard_index);
                m.push(("default_weight".into(), Y::Float(*weight)));
            }
        }
        Plug::BsaSelectorUpdater {
            node_index,
            name,
            child_enum_bb_index,
            child_enum_value,
        } => {
            push_node_name(m, *node_index, name);
            if *child_enum_bb_index < 0 {
                push_int(m, "child_enum_value", *child_enum_value);
            } else {
                push_int(m, "child_enum_bb_index", *child_enum_bb_index);
            }
        }
        _ => unreachable!(),
    }
}

fn flag_list(node: &Node) -> Y {
    let mut v = Vec::new();
    if node.is_query() {
        v.push(Y::Str("is_query".into()));
    }
    if node.is_module() {
        v.push(Y::Str("is_module".into()));
    }
    if node.is_root_node() {
        v.push(Y::Str("is_root_node".into()));
    }
    if node.is_multi_param_type2() {
        v.push(Y::Str("use_multiparam_type_2".into()));
    }
    Y::Seq(v)
}

fn attachment_to_y(a: &Attachment) -> Y {
    Y::Map(vec![
        ("name".into(), Y::Str(a.name.clone())),
        ("debug".into(), Y::Int(i64::from(a.debug))),
        (
            "expression_instance_count".into(),
            Y::Int(i64::from(a.expr_count)),
        ),
        (
            "expression_io_size".into(),
            Y::Int(i64::from(a.expr_io_size)),
        ),
        ("properties".into(), property_set_to_y(&a.properties)),
    ])
}

fn node_to_y(n: &Node) -> Y {
    let mut m = vec![
        ("node_type".into(), Y::Str(n.ntype.name().into())),
        ("node_index".into(), Y::Int(i64::from(n.index))),
        ("name".into(), Y::Str(n.name.clone())),
        ("guid".into(), Y::Str(n.guid.clone())),
        ("flags".into(), flag_list(n)),
        (
            "expression_instance_count".into(),
            Y::Int(i64::from(n.expr_count)),
        ),
        (
            "expression_io_size".into(),
            Y::Int(i64::from(n.expr_io_size)),
        ),
        (
            "queries".into(),
            Y::Seq(n.queries.iter().map(|q| Y::Int(i64::from(*q))).collect()),
        ),
        (
            "attachments".into(),
            Y::Seq(n.attachments.iter().map(attachment_to_y).collect()),
        ),
        ("properties".into(), property_set_to_y(&n.properties)),
        ("parameters".into(), param_set_to_y(&n.params)),
        (
            "xlink_actions".into(),
            Y::Seq(
                n.actions
                    .iter()
                    .map(|a| {
                        Y::Map(vec![
                            ("action_slot".into(), Y::Str(a.action_slot.clone())),
                            ("action".into(), Y::Str(a.action.clone())),
                        ])
                    })
                    .collect(),
            ),
        ),
    ];
    if let Some(s) = &n.state_info {
        m.push(("state_info".into(), state_info_to_y(s)));
    }
    let mut plugs = Vec::new();
    for (i, name) in PLUG_TYPE_NAMES.into_iter().enumerate() {
        if !n.plugs[i].is_empty() {
            plugs.push((
                name.into(),
                Y::Seq(n.plugs[i].iter().map(plug_to_y).collect()),
            ));
        }
    }
    m.push(("plugs".into(), Y::Map(plugs)));
    Y::Map(m)
}

fn state_info_to_y(s: &StateInfo) -> Y {
    Y::Map(vec![
        ("desired_state".into(), Y::Str(s.desired_state.clone())),
        ("unknown_04".into(), Y::Int(i64::from(s.unk04))),
        ("unknown_08".into(), Y::Int(i64::from(s.unk08))),
        ("unknown_0c".into(), Y::Int(i64::from(s.unk0c))),
        ("unknown_10".into(), Y::Int(i64::from(s.unk10))),
    ])
}

fn bb_param_to_y(p: &BbParam, index: usize) -> Y {
    let mut m = vec![
        (
            "blackboard_index".into(),
            Y::Int(i64::try_from(index).unwrap_or(0)),
        ),
        ("name".into(), Y::Str(p.name.clone())),
        ("notes".into(), Y::Str(p.notes.clone())),
    ];
    if !p.file_ref.is_empty() {
        m.push(("source_file".into(), Y::Str(p.file_ref.clone())));
    }
    m.push(("flags".into(), Y::Int(i64::from(p.flags))));
    m.push(("default_value".into(), value_to_y(&p.value)));
    Y::Map(m)
}

fn blackboard_to_y(bb: &Blackboard) -> Y {
    let mut m = Vec::new();
    for pt in BB_PARAM_TYPES {
        let v = &bb.params[pt.index()];
        if !v.is_empty() {
            m.push((
                pt.name().into(),
                Y::Seq(
                    v.iter()
                        .enumerate()
                        .map(|(i, p)| bb_param_to_y(p, i))
                        .collect(),
                ),
            ));
        }
    }
    Y::Map(m)
}

fn command_to_y(c: &Command) -> Y {
    let mut m = vec![
        ("name".into(), Y::Str(c.name.clone())),
        ("guid".into(), Y::Str(c.guid.clone())),
        (
            "root_node_index".into(),
            Y::Int(i64::from(c.root_node_index)),
        ),
    ];
    if c.secondary_root_node_index >= 0 {
        m.push((
            "secondary_root_node_index".into(),
            Y::Int(i64::from(c.secondary_root_node_index)),
        ));
    }
    Y::Map(m)
}

fn replacement_to_y(r: &ReplacementEntry) -> Y {
    let mut m = vec![
        ("type".into(), Y::Str(r.rtype.name().into())),
        ("node_index".into(), Y::Int(i64::from(r.node_index))),
    ];
    if r.rtype == ReplacementType::RemoveAttachment {
        m.push((
            "attachment_index".into(),
            Y::Int(i64::from(r.replace_index)),
        ));
    } else {
        m.push((
            "child_plug_index".into(),
            Y::Int(i64::from(r.replace_index)),
        ));
        if r.rtype == ReplacementType::ReplaceChild {
            m.push((
                "replacement_node_index".into(),
                Y::Int(i64::from(r.new_index)),
            ));
        }
    }
    Y::Map(m)
}

pub(super) fn emit(ainb: &Ainb) -> String {
    let mut top = vec![
        ("version".into(), Y::Int(i64::from(ainb.version))),
        ("filename".into(), Y::Str(ainb.filename.clone())),
        ("category".into(), Y::Str(ainb.category.clone())),
        (
            "blackboard_id".into(),
            Y::Int(i64::from(ainb.blackboard_id)),
        ),
        (
            "parent_blackboard_id".into(),
            Y::Int(i64::from(ainb.parent_blackboard_id)),
        ),
        (
            "commands".into(),
            Y::Seq(ainb.commands.iter().map(command_to_y).collect()),
        ),
        (
            "nodes".into(),
            Y::Seq(ainb.nodes.iter().map(node_to_y).collect()),
        ),
        (
            "blackboard".into(),
            ainb.blackboard
                .as_ref()
                .map_or(Y::Map(vec![]), blackboard_to_y),
        ),
    ];
    match &ainb.expressions {
        Some(exb) => top.push((
            "expressions".into(),
            Y::Map(vec![
                ("version".into(), Y::Int(i64::from(exb.version()))),
                ("exb".into(), Y::Str(B64.encode(exb.raw()))),
            ]),
        )),
        None => top.push(("expressions".into(), Y::Null)),
    }
    if ainb.version >= 0x407 {
        top.push((
            "replacement_table".into(),
            Y::Seq(
                ainb.replacement_table
                    .iter()
                    .map(replacement_to_y)
                    .collect(),
            ),
        ));
    }
    top.push((
        "modules".into(),
        Y::Seq(
            ainb.modules
                .iter()
                .map(|m| {
                    Y::Map(vec![
                        ("path".into(), Y::Str(m.path.clone())),
                        ("category".into(), Y::Str(m.category.clone())),
                        ("instance_count".into(), Y::Int(i64::from(m.instance_count))),
                    ])
                })
                .collect(),
        ),
    ));
    if let Some(unk) = &ainb.unk_section0x58 {
        top.push((
            "unknown_section_0x58".into(),
            Y::Map(vec![
                ("description".into(), Y::Str(unk.description.clone())),
                ("unknown_04".into(), Y::Int(i64::from(unk.unk04))),
                ("unknown_08".into(), Y::Int(i64::from(unk.unk08))),
                ("unknown_0c".into(), Y::Int(i64::from(unk.unk0c))),
            ]),
        ));
    }
    top.push(("has_section_0x6c".into(), Y::Bool(ainb.exists_section_0x6c)));

    let mut out = String::new();
    emit_map(&mut out, &top, 0);
    out
}

fn emit_scalar(out: &mut String, y: &Y) {
    match y {
        Y::Int(i) => {
            let _ = write!(out, "{i}");
        }
        Y::Float(f) => {
            if f.is_finite() {
                let _ = write!(out, "{f:?}");
            } else if f.is_nan() {
                out.push_str(".nan");
            } else if *f > 0.0 {
                out.push_str(".inf");
            } else {
                out.push_str("-.inf");
            }
        }
        Y::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Y::Null => out.push_str("null"),
        Y::Str(s) => emit_quoted(out, s),
        _ => {}
    }
}

fn emit_quoted(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out.push('"');
}

fn emit_map(out: &mut String, map: &[(String, Y)], indent: usize) {
    let pad = " ".repeat(indent);
    for (k, v) in map {
        match v {
            Y::Map(m) if m.is_empty() => {
                let _ = writeln!(out, "{pad}{k}: {{}}");
            }
            Y::Seq(s) if s.is_empty() => {
                let _ = writeln!(out, "{pad}{k}: []");
            }
            Y::Map(m) => {
                let _ = writeln!(out, "{pad}{k}:");
                emit_map(out, m, indent + 2);
            }
            Y::Seq(s) => {
                let _ = writeln!(out, "{pad}{k}:");
                emit_seq(out, s, indent);
            }
            scalar => {
                let _ = write!(out, "{pad}{k}: ");
                emit_scalar(out, scalar);
                out.push('\n');
            }
        }
    }
}

fn emit_seq(out: &mut String, seq: &[Y], indent: usize) {
    let pad = " ".repeat(indent);
    for item in seq {
        match item {
            Y::Map(m) if m.is_empty() => {
                let _ = writeln!(out, "{pad}- {{}}");
            }
            Y::Seq(s) if s.is_empty() => {
                let _ = writeln!(out, "{pad}- []");
            }
            Y::Map(m) => {
                let mut buf = String::new();
                emit_map(&mut buf, m, indent + 2);
                splice_dash(out, &buf, indent);
            }
            Y::Seq(s) => {
                let mut buf = String::new();
                emit_seq(&mut buf, s, indent + 2);
                splice_dash(out, &buf, indent);
            }
            scalar => {
                out.push_str(&pad);
                out.push_str("- ");
                emit_scalar(out, scalar);
                out.push('\n');
            }
        }
    }
}

fn splice_dash(out: &mut String, buf: &str, indent: usize) {
    let mut lines = buf.lines();
    if let Some(first) = lines.next() {
        let trimmed = first.trim_start();
        out.push_str(&" ".repeat(indent));
        out.push_str("- ");
        out.push_str(trimmed);
        out.push('\n');
        for line in lines {
            out.push_str(line);
            out.push('\n');
        }
    }
}

type Map<'a> = saphyr::AnnotatedMapping<'a, Yaml<'a>>;

fn get<'a>(m: &'a Map<'a>, key: &str) -> Option<&'a Yaml<'a>> {
    m.get(&Yaml::Value(Scalar::String(std::borrow::Cow::Owned(
        key.to_owned(),
    ))))
}

fn as_map<'a>(y: &'a Yaml<'a>) -> Result<&'a Map<'a>> {
    match y {
        Yaml::Mapping(m) => Ok(m),
        _ => Err(Error::malformed("AINB YAML: expected mapping")),
    }
}

fn as_seq<'a>(y: &'a Yaml<'a>) -> &'a [Yaml<'a>] {
    y.as_sequence().map_or(&[], |s| s)
}

fn req<'a>(m: &'a Map<'a>, key: &str) -> Result<&'a Yaml<'a>> {
    get(m, key).ok_or_else(|| Error::malformed(format!("AINB YAML: missing key `{key}`")))
}

fn p_str(y: &Yaml) -> Result<String> {
    y.as_str()
        .map(str::to_owned)
        .ok_or_else(|| Error::malformed("AINB YAML: expected string"))
}

fn p_str_at(m: &Map, key: &str) -> Result<String> {
    p_str(req(m, key)?)
}

fn p_i64(y: &Yaml) -> Result<i64> {
    match y {
        Yaml::Value(Scalar::Integer(i)) => Ok(*i),
        Yaml::Value(Scalar::String(s)) => s
            .parse::<i64>()
            .map_err(|_| Error::malformed("AINB YAML: bad integer")),
        _ => Err(Error::malformed("AINB YAML: expected integer")),
    }
}

fn p_i64_at(m: &Map, key: &str) -> Result<i64> {
    p_i64(req(m, key)?)
}

fn p_i32(y: &Yaml) -> Result<i32> {
    i32::try_from(p_i64(y)?).map_err(|_| Error::malformed("AINB YAML: i32 out of range"))
}

fn p_u32(y: &Yaml) -> Result<u32> {
    u32::try_from(p_i64(y)?).map_err(|_| Error::malformed("AINB YAML: u32 out of range"))
}

fn p_i32_at(m: &Map, key: &str) -> Result<i32> {
    p_i32(req(m, key)?)
}

fn p_u32_at(m: &Map, key: &str) -> Result<u32> {
    p_u32(req(m, key)?)
}

fn p_u16_at(m: &Map, key: &str) -> Result<u16> {
    u16::try_from(p_i64_at(m, key)?).map_err(|_| Error::malformed("AINB YAML: u16 out of range"))
}

fn p_u8_at(m: &Map, key: &str) -> Result<u8> {
    u8::try_from(p_i64_at(m, key)?).map_err(|_| Error::malformed("AINB YAML: u8 out of range"))
}

#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
fn p_f32(y: &Yaml) -> Result<f32> {
    match y {
        Yaml::Value(Scalar::FloatingPoint(f)) => Ok(f.into_inner() as f32),
        Yaml::Value(Scalar::Integer(i)) => Ok(*i as f32),
        Yaml::Value(Scalar::String(s)) => {
            let t = s.trim();
            match t {
                ".nan" | ".NaN" | "NaN" => Ok(f32::NAN),
                ".inf" | "inf" => Ok(f32::INFINITY),
                "-.inf" | "-inf" => Ok(f32::NEG_INFINITY),
                _ => t
                    .parse::<f32>()
                    .map_err(|_| Error::malformed("AINB YAML: bad float")),
            }
        }
        _ => Err(Error::malformed("AINB YAML: expected float")),
    }
}

fn p_bool(y: &Yaml) -> Result<bool> {
    match y {
        Yaml::Value(Scalar::Boolean(b)) => Ok(*b),
        Yaml::Value(Scalar::String(s)) if s == "true" => Ok(true),
        Yaml::Value(Scalar::String(s)) if s == "false" => Ok(false),
        _ => Err(Error::malformed("AINB YAML: expected bool")),
    }
}

fn p_value(y: &Yaml, ptype: ParamType) -> Result<Value> {
    Ok(match ptype {
        ParamType::Int => Value::Int(p_i32(y)?),
        ParamType::Bool => Value::Bool(p_bool(y)?),
        ParamType::Float => Value::Float(p_f32(y)?),
        ParamType::String => Value::Str(p_str(y)?),
        ParamType::Vector3F => {
            let s = as_seq(y);
            if s.len() != 3 {
                return Err(Error::malformed("AINB YAML: vec3 needs 3 elements"));
            }
            Value::Vec3([p_f32(&s[0])?, p_f32(&s[1])?, p_f32(&s[2])?])
        }
        ParamType::Pointer => Value::Null,
    })
}

fn p_bb_value(y: &Yaml, ptype: BbParamType) -> Result<Value> {
    Ok(match ptype {
        BbParamType::String => Value::Str(p_str(y)?),
        BbParamType::S32 => Value::Int(p_i32(y)?),
        BbParamType::U32 => Value::UInt(p_u32(y)?),
        BbParamType::F32 => Value::Float(p_f32(y)?),
        BbParamType::Bool => Value::Bool(p_bool(y)?),
        BbParamType::Vec3f => {
            let s = as_seq(y);
            if s.len() != 3 {
                return Err(Error::malformed("AINB YAML: vec3 needs 3 elements"));
            }
            Value::Vec3([p_f32(&s[0])?, p_f32(&s[1])?, p_f32(&s[2])?])
        }
        BbParamType::VoidPtr => Value::Null,
    })
}

fn p_flags(m: &Map) -> u32 {
    let mut flag = 0u32;
    if let Some(list) = get(m, "flags") {
        for item in as_seq(list) {
            match item.as_str() {
                Some("uses_default") => flag = (flag & 0xff7f_ffff) | (1 << 0x17),
                Some("is_output") => flag = (flag & 0xfeff_ffff) | (1 << 0x18),
                _ => {}
            }
        }
    }
    if let Some(idx) = get(m, "expression_index") {
        flag = (flag & 0x3dff_ffff) | 0xc200_0000;
        flag = (flag & 0xffff_0000) | (p_u32(idx).unwrap_or(0) & 0xffff);
    } else if let Some(idx) = get(m, "blackboard_index") {
        flag = (flag & 0x3dff_ffff) | 0x8000_0000;
        flag = (flag & 0xffff_0000) | (p_u32(idx).unwrap_or(0) & 0xffff);
        if let Some(vc) = get(m, "vector_component") {
            let comp = vc_value(vc.as_str().unwrap_or("NONE"));
            flag = (flag & 0xf3ff_ffff) | (comp << 0x1a);
        }
    }
    flag
}

fn p_property(m: &Map, ptype: ParamType) -> Result<Property> {
    Ok(Property {
        name: p_str_at(m, "name")?,
        classname: if ptype == ParamType::Pointer {
            p_str_at(m, "classname")?
        } else {
            String::new()
        },
        ptype,
        flags: p_flags(m),
        value: p_value(req(m, "default_value")?, ptype)?,
    })
}

fn p_property_set(y: &Yaml) -> Result<PropertySet> {
    let m = as_map(y)?;
    let mut set = PropertySet::default();
    for pt in PARAM_TYPES {
        if let Some(seq) = get(m, pt.name()) {
            let mut v = Vec::new();
            for item in as_seq(seq) {
                v.push(p_property(as_map(item)?, pt)?);
            }
            set.props[pt.index()] = v;
        }
    }
    Ok(set)
}

fn p_source(m: &Map) -> Result<ParamSource> {
    Ok(ParamSource {
        src_node_index: p_i32_at(m, "node_index")?,
        src_output_index: p_i32_at(m, "output_index")?,
        flags: p_flags(m),
    })
}

fn p_input(m: &Map, ptype: ParamType) -> Result<InputParam> {
    let source = if let Some(sources) = get(m, "sources") {
        let mut list = Vec::new();
        for s in as_seq(sources) {
            list.push(p_source(as_map(s)?)?);
        }
        Source::Multi(list)
    } else {
        Source::Single(p_source(m)?)
    };
    Ok(InputParam {
        name: p_str_at(m, "name")?,
        classname: if ptype == ParamType::Pointer {
            p_str_at(m, "classname")?
        } else {
            String::new()
        },
        ptype,
        value: p_value(req(m, "default_value")?, ptype)?,
        source,
        is_blackboard_input: get(m, "is_set_blackboard")
            .is_some_and(|b| p_bool(b).unwrap_or(false)),
    })
}

fn p_output(m: &Map, ptype: ParamType) -> Result<OutputParam> {
    Ok(OutputParam {
        name: p_str_at(m, "name")?,
        classname: if ptype == ParamType::Pointer {
            p_str_at(m, "classname")?
        } else {
            String::new()
        },
        ptype,
        is_output: p_bool(req(m, "is_output")?)?,
    })
}

fn p_param_set(y: &Yaml) -> Result<ParamSet> {
    let m = as_map(y)?;
    let mut set = ParamSet::default();
    if let Some(inputs) = get(m, "inputs") {
        let im = as_map(inputs)?;
        for pt in PARAM_TYPES {
            if let Some(seq) = get(im, pt.name()) {
                let mut v = Vec::new();
                for item in as_seq(seq) {
                    v.push(p_input(as_map(item)?, pt)?);
                }
                set.inputs[pt.index()] = v;
            }
        }
    }
    if let Some(outputs) = get(m, "outputs") {
        let om = as_map(outputs)?;
        for pt in PARAM_TYPES {
            if let Some(seq) = get(om, pt.name()) {
                let mut v = Vec::new();
                for item in as_seq(seq) {
                    v.push(p_output(as_map(item)?, pt)?);
                }
                set.outputs[pt.index()] = v;
            }
        }
    }
    Ok(set)
}

fn p_plug(m: &Map, ntype: NodeType, bucket: usize, node_name: &str) -> Result<Plug> {
    let node_index = p_i32_at(m, "node_index")?;
    let name = || p_str_at(m, "name");
    match bucket {
        0 => match ntype {
            NodeType::ElementBoolSelector => Ok(Plug::BoolSelectorInput {
                node_index,
                name: name()?,
                unk0: p_u32_at(m, "unknown_1")?,
                unk1: p_u32_at(m, "unknown_2")?,
            }),
            NodeType::ElementF32Selector => Ok(Plug::F32SelectorInput {
                node_index,
                name: name()?,
                unk0: p_u32_at(m, "unknown_1")?,
                unk1: p_f32(req(m, "unknown_2")?)?,
            }),
            NodeType::ElementExpression => p_s32_selector_input(m, node_index),
            _ => Ok(Plug::Generic {
                node_index,
                name: name()?,
            }),
        },
        2 => match ntype {
            NodeType::ElementS32Selector => Ok(p_s32_selector(m, node_index, name()?)),
            NodeType::ElementF32Selector => Ok(p_f32_selector(m, node_index, name()?)?),
            NodeType::ElementStringSelector => Ok(p_string_selector(m, node_index, name()?)?),
            NodeType::ElementRandomSelector => Ok(p_random_selector(m, node_index, name()?)?),
            _ if node_name == "SelectorBSABrainVerbUpdater"
                || node_name == "SelectorBSAFormChangeUpdater" =>
            {
                Ok(p_bsa(m, node_index, name()?))
            }
            _ => Ok(Plug::Child {
                node_index,
                name: name()?,
            }),
        },
        3 => {
            let transition = if let Some(tn) = get(m, "transition_name") {
                Transition {
                    transition_type: p_u32_at(m, "transition_type")?,
                    update_post_calc: p_bool(req(m, "update_post_calc")?)?,
                    command_name: p_str(tn)?,
                }
            } else {
                Transition {
                    transition_type: p_u32_at(m, "transition_type")?,
                    update_post_calc: p_bool(req(m, "update_post_calc")?)?,
                    command_name: String::new(),
                }
            };
            Ok(Plug::Transition {
                node_index,
                transition,
            })
        }
        4 => match ntype {
            NodeType::ElementStringSelector | NodeType::ElementExpression => {
                p_string_selector_input(m, node_index)
            }
            _ => Ok(Plug::Generic {
                node_index,
                name: name()?,
            }),
        },
        5 => match ntype {
            NodeType::ElementS32Selector | NodeType::ElementExpression => {
                p_s32_selector_input(m, node_index)
            }
            _ => Ok(Plug::Generic {
                node_index,
                name: name()?,
            }),
        },
        _ => Err(Error::malformed("AINB YAML: unsupported plug bucket")),
    }
}

fn p_s32_selector(m: &Map, node_index: i32, name: String) -> Plug {
    let (condition, is_default, blackboard_index) = if get(m, "condition").is_some() {
        (p_i32_at(m, "condition").unwrap_or(0), false, -1)
    } else if get(m, "default_condition").is_some() {
        (
            p_i32_at(m, "default_condition").unwrap_or(0),
            false,
            p_i32_at(m, "blackboard_index").unwrap_or(-1),
        )
    } else {
        (0, true, -1)
    };
    Plug::S32Selector {
        node_index,
        name,
        condition,
        is_default,
        blackboard_index,
    }
}

fn p_f32_selector(m: &Map, node_index: i32, name: String) -> Result<Plug> {
    if get(m, "is_default").is_some() {
        return Ok(Plug::F32Selector {
            node_index,
            name,
            condition_min: 0.0,
            blackboard_index_min: -1,
            condition_max: 0.0,
            blackboard_index_max: -1,
            is_default: true,
        });
    }
    let (condition_min, bb_min) = if let Some(c) = get(m, "condition_min") {
        (p_f32(c)?, -1)
    } else {
        (0.0, p_i32_at(m, "condition_min_blackboard_index")?)
    };
    let (condition_max, bb_max) = if let Some(c) = get(m, "condition_max") {
        (p_f32(c)?, -1)
    } else {
        (0.0, p_i32_at(m, "condition_max_blackboard_index")?)
    };
    Ok(Plug::F32Selector {
        node_index,
        name,
        condition_min,
        blackboard_index_min: bb_min,
        condition_max,
        blackboard_index_max: bb_max,
        is_default: false,
    })
}

fn p_string_selector(m: &Map, node_index: i32, name: String) -> Result<Plug> {
    let (condition, is_default, blackboard_index) = if get(m, "is_default").is_some() {
        (
            get(m, "condition")
                .map(p_str)
                .transpose()?
                .unwrap_or_default(),
            true,
            -1,
        )
    } else if let Some(c) = get(m, "condition") {
        (p_str(c)?, false, -1)
    } else if let Some(c) = get(m, "default_condition") {
        (p_str(c)?, false, p_i32_at(m, "blackboard_index")?)
    } else {
        (String::new(), true, -1)
    };
    Ok(Plug::StringSelector {
        node_index,
        name,
        condition,
        is_default,
        blackboard_index,
    })
}

fn p_random_selector(m: &Map, node_index: i32, name: String) -> Result<Plug> {
    let (blackboard_index, weight) = if let Some(w) = get(m, "weight") {
        (-1, p_f32(w)?)
    } else {
        (
            p_i32_at(m, "blackboard_index")?,
            p_f32(req(m, "default_weight")?)?,
        )
    };
    Ok(Plug::RandomSelector {
        node_index,
        name,
        blackboard_index,
        weight,
    })
}

fn p_bsa(m: &Map, node_index: i32, name: String) -> Plug {
    if get(m, "child_enum_value").is_some() {
        Plug::BsaSelectorUpdater {
            node_index,
            name,
            child_enum_bb_index: -1,
            child_enum_value: p_u32_at(m, "child_enum_value").unwrap_or(0),
        }
    } else {
        Plug::BsaSelectorUpdater {
            node_index,
            name,
            child_enum_bb_index: p_i32_at(m, "child_enum_bb_index").unwrap_or(-1),
            child_enum_value: 0,
        }
    }
}

fn p_string_selector_input(m: &Map, node_index: i32) -> Result<Plug> {
    let name = p_str_at(m, "name")?;
    if let Some(u) = get(m, "unknown") {
        Ok(Plug::StringSelectorInput {
            node_index,
            name,
            unknown: p_u32(u)?,
            default_value: p_str_at(m, "default_value")?,
            read_extra: true,
        })
    } else {
        Ok(Plug::StringSelectorInput {
            node_index,
            name,
            unknown: 0,
            default_value: String::new(),
            read_extra: false,
        })
    }
}

fn p_s32_selector_input(m: &Map, node_index: i32) -> Result<Plug> {
    let name = p_str_at(m, "name")?;
    if let Some(u) = get(m, "unknown") {
        Ok(Plug::S32SelectorInput {
            node_index,
            name,
            unknown: p_u32(u)?,
            default_value: p_i32_at(m, "default_value")?,
            read_extra: true,
        })
    } else {
        Ok(Plug::S32SelectorInput {
            node_index,
            name,
            unknown: 0,
            default_value: 0,
            read_extra: false,
        })
    }
}

fn p_attachment(m: &Map) -> Result<Attachment> {
    Ok(Attachment {
        name: p_str_at(m, "name")?,
        debug: p_u32_at(m, "debug")?,
        expr_count: p_u16_at(m, "expression_instance_count")?,
        expr_io_size: p_u16_at(m, "expression_io_size")?,
        properties: p_property_set(req(m, "properties")?)?,
    })
}

fn p_node(m: &Map) -> Result<Node> {
    let ntype = NodeType::from_name(&p_str_at(m, "node_type")?)
        .ok_or_else(|| Error::malformed("AINB YAML: unknown node type"))?;
    let name = p_str_at(m, "name")?;
    let mut flags = 0u8;
    for f in as_seq(req(m, "flags")?) {
        match f.as_str() {
            Some("is_query") => flags |= 1,
            Some("is_module") => flags |= 2,
            Some("is_root_node") => flags |= 4,
            Some("use_multiparam_type_2") => flags |= 8,
            _ => {}
        }
    }
    let queries = as_seq(req(m, "queries")?)
        .iter()
        .map(p_i32)
        .collect::<Result<Vec<_>>>()?;
    let mut attachments = Vec::new();
    for a in as_seq(req(m, "attachments")?) {
        attachments.push(p_attachment(as_map(a)?)?);
    }
    let mut actions = Vec::new();
    for a in as_seq(req(m, "xlink_actions")?) {
        let am = as_map(a)?;
        actions.push(Action {
            action_slot: p_str_at(am, "action_slot")?,
            action: p_str_at(am, "action")?,
        });
    }
    let state_info = if let Some(s) = get(m, "state_info") {
        let sm = as_map(s)?;
        Some(StateInfo {
            desired_state: p_str_at(sm, "desired_state")?,
            unk04: p_u32_at(sm, "unknown_04")?,
            unk08: p_u32_at(sm, "unknown_08")?,
            unk0c: p_u32_at(sm, "unknown_0c")?,
            unk10: p_u32_at(sm, "unknown_10")?,
        })
    } else {
        None
    };
    let mut plugs: [Vec<Plug>; 10] = Default::default();
    let plug_map = as_map(req(m, "plugs")?)?;
    for (i, pname) in PLUG_TYPE_NAMES.into_iter().enumerate() {
        if let Some(seq) = get(plug_map, pname) {
            let mut v = Vec::new();
            for item in as_seq(seq) {
                v.push(p_plug(as_map(item)?, ntype, i, &name)?);
            }
            plugs[i] = v;
        }
    }
    Ok(Node {
        index: p_i32_at(m, "node_index")?,
        guid: p_str_at(m, "guid")?,
        expr_count: p_u16_at(m, "expression_instance_count")?,
        expr_io_size: p_u16_at(m, "expression_io_size")?,
        ntype,
        name,
        flags,
        queries,
        attachments,
        properties: p_property_set(req(m, "properties")?)?,
        params: p_param_set(req(m, "parameters")?)?,
        actions,
        state_info,
        plugs,
    })
}

fn p_blackboard(y: &Yaml) -> Result<Option<Blackboard>> {
    let m = as_map(y)?;
    if m.is_empty() {
        return Ok(Some(Blackboard::default()));
    }
    let mut bb = Blackboard::default();
    for pt in BB_PARAM_TYPES {
        if let Some(seq) = get(m, pt.name()) {
            let mut v = Vec::new();
            for item in as_seq(seq) {
                let pm = as_map(item)?;
                let flags = p_u8_at(pm, "flags")?;
                if flags > 3 {
                    return Err(Error::malformed(
                        "AINB YAML: blackboard flags must be 0..=3",
                    ));
                }
                v.push(BbParam {
                    name: p_str_at(pm, "name")?,
                    notes: p_str_at(pm, "notes")?,
                    file_ref: get(pm, "source_file")
                        .map(|s| p_str(s))
                        .transpose()?
                        .unwrap_or_default(),
                    flags,
                    value: p_bb_value(req(pm, "default_value")?, pt)?,
                });
            }
            bb.params[pt.index()] = v;
        }
    }
    Ok(Some(bb))
}

fn p_replacement_table(seq: &Yaml) -> Result<Vec<ReplacementEntry>> {
    let mut table = Vec::new();
    for r in as_seq(seq) {
        let rm = as_map(r)?;
        let rtype = ReplacementType::from_name(&p_str_at(rm, "type")?)
            .ok_or_else(|| Error::malformed("AINB YAML: unknown replacement type"))?;
        let (replace_index, new_index) = if rtype == ReplacementType::RemoveAttachment {
            (p_i32_at(rm, "attachment_index")?, -1)
        } else if rtype == ReplacementType::ReplaceChild {
            (
                p_i32_at(rm, "child_plug_index")?,
                p_i32_at(rm, "replacement_node_index")?,
            )
        } else {
            (p_i32_at(rm, "child_plug_index")?, -1)
        };
        table.push(ReplacementEntry {
            rtype,
            node_index: p_i32_at(rm, "node_index")?,
            replace_index,
            new_index,
        });
    }
    Ok(table)
}

pub(super) fn parse(text: &str) -> Result<Ainb> {
    let docs =
        Yaml::load_from_str(text).map_err(|e| Error::malformed(format!("AINB YAML parse: {e}")))?;
    let doc = docs
        .first()
        .ok_or_else(|| Error::malformed("AINB YAML: empty document"))?;
    let m = as_map(doc)?;

    let version = p_u32_at(m, "version")?;
    crate::formats::ainb::check_version(version)?;

    let mut commands = Vec::new();
    for c in as_seq(req(m, "commands")?) {
        let cm = as_map(c)?;
        commands.push(Command {
            name: p_str_at(cm, "name")?,
            guid: p_str_at(cm, "guid")?,
            root_node_index: p_i32_at(cm, "root_node_index")?,
            secondary_root_node_index: get(cm, "secondary_root_node_index")
                .map(p_i32)
                .transpose()?
                .unwrap_or(-1),
        });
    }

    let mut nodes = Vec::new();
    for n in as_seq(req(m, "nodes")?) {
        nodes.push(p_node(as_map(n)?)?);
    }

    let blackboard = match get(m, "blackboard") {
        Some(y) => p_blackboard(y)?,
        None => Some(Blackboard::default()),
    };

    let expressions = match get(m, "expressions") {
        Some(Yaml::Mapping(em)) => {
            let data = B64
                .decode(p_str_at(em, "exb")?)
                .map_err(|e| Error::malformed(format!("AINB YAML: bad base64 EXB: {e}")))?;
            Some(Exb::parse(data)?)
        }
        _ => None,
    };

    let replacement_table = match get(m, "replacement_table") {
        Some(seq) => p_replacement_table(seq)?,
        None => Vec::new(),
    };

    let mut modules = Vec::new();
    for mm in as_seq(req(m, "modules")?) {
        let md = as_map(mm)?;
        modules.push(Module {
            path: p_str_at(md, "path")?,
            category: p_str_at(md, "category")?,
            instance_count: p_u32_at(md, "instance_count")?,
        });
    }

    let unk_section0x58 = match get(m, "unknown_section_0x58") {
        Some(y) => {
            let um = as_map(y)?;
            Some(UnknownSection0x58 {
                description: p_str_at(um, "description")?,
                unk04: p_u32_at(um, "unknown_04")?,
                unk08: p_u32_at(um, "unknown_08")?,
                unk0c: p_u32_at(um, "unknown_0c")?,
            })
        }
        None => None,
    };

    Ok(Ainb {
        version,
        filename: p_str_at(m, "filename")?,
        category: p_str_at(m, "category")?,
        blackboard_id: p_u32_at(m, "blackboard_id")?,
        parent_blackboard_id: p_u32_at(m, "parent_blackboard_id")?,
        commands,
        nodes,
        blackboard,
        expressions,
        replacement_table,
        modules,
        unk_section0x58,
        exists_section_0x6c: get(m, "has_section_0x6c")
            .map(p_bool)
            .transpose()?
            .unwrap_or(false),
    })
}
