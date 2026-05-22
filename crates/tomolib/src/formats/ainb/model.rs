/// Data type of a node property or parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamType {
    Int,
    Bool,
    Float,
    String,
    Vector3F,
    Pointer,
}

pub(super) const PARAM_TYPES: [ParamType; 6] = [
    ParamType::Int,
    ParamType::Bool,
    ParamType::Float,
    ParamType::String,
    ParamType::Vector3F,
    ParamType::Pointer,
];

impl ParamType {
    #[must_use]
    pub(crate) fn index(self) -> usize {
        match self {
            ParamType::Int => 0,
            ParamType::Bool => 1,
            ParamType::Float => 2,
            ParamType::String => 3,
            ParamType::Vector3F => 4,
            ParamType::Pointer => 5,
        }
    }

    #[must_use]
    pub(crate) fn name(self) -> &'static str {
        match self {
            ParamType::Int => "Int",
            ParamType::Bool => "Bool",
            ParamType::Float => "Float",
            ParamType::String => "String",
            ParamType::Vector3F => "Vector3F",
            ParamType::Pointer => "Pointer",
        }
    }

    #[must_use]
    pub(crate) fn property_size(self) -> usize {
        match self {
            ParamType::Vector3F => 0x14,
            _ => 0xc,
        }
    }

    #[must_use]
    pub(crate) fn input_size(self) -> usize {
        match self {
            ParamType::Vector3F => 0x18,
            ParamType::Pointer => 0x14,
            _ => 0x10,
        }
    }

    #[must_use]
    pub(crate) fn output_size(self) -> usize {
        if self == ParamType::Pointer { 8 } else { 4 }
    }
}

/// A concrete value held by a property, parameter, or blackboard entry.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Int(i32),
    UInt(u32),
    Bool(bool),
    Float(f32),
    Str(String),
    Vec3([f32; 3]),
    Null,
}

/// Data type of a blackboard parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BbParamType {
    String,
    S32,
    U32,
    F32,
    Bool,
    Vec3f,
    VoidPtr,
}

pub(super) const BB_PARAM_TYPES: [BbParamType; 7] = [
    BbParamType::String,
    BbParamType::S32,
    BbParamType::U32,
    BbParamType::F32,
    BbParamType::Bool,
    BbParamType::Vec3f,
    BbParamType::VoidPtr,
];

impl BbParamType {
    #[must_use]
    pub(crate) fn index(self) -> usize {
        match self {
            BbParamType::String => 0,
            BbParamType::S32 => 1,
            BbParamType::U32 => 2,
            BbParamType::F32 => 3,
            BbParamType::Bool => 4,
            BbParamType::Vec3f => 5,
            BbParamType::VoidPtr => 6,
        }
    }

    #[must_use]
    pub(crate) fn name(self) -> &'static str {
        match self {
            BbParamType::String => "String",
            BbParamType::S32 => "S32",
            BbParamType::U32 => "U32",
            BbParamType::F32 => "F32",
            BbParamType::Bool => "Bool",
            BbParamType::Vec3f => "Vec3f",
            BbParamType::VoidPtr => "VoidPtr",
        }
    }

    #[must_use]
    pub(crate) fn is_supported(self, version: u32) -> bool {
        !(version < 0x408 && self == BbParamType::U32)
    }

    #[must_use]
    pub(crate) fn value_size(self) -> usize {
        match self {
            BbParamType::Vec3f => 0xc,
            BbParamType::VoidPtr => 0,
            _ => 4,
        }
    }
}

/// A named entry point into the graph, identifying its root node(s).
#[derive(Debug, Clone)]
pub struct Command {
    pub name: String,
    pub guid: String,
    pub root_node_index: i32,
    pub secondary_root_node_index: i32,
}

/// A constant value attached directly to a node.
#[derive(Debug, Clone, PartialEq)]
pub struct Property {
    pub name: String,
    pub classname: String,
    pub ptype: ParamType,
    pub flags: u32,
    pub value: Value,
}

/// A node's properties, grouped by [`ParamType`] (one bucket per type, in the
/// order of [`ParamType`]'s variants).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PropertySet {
    pub props: [Vec<Property>; 6],
}

impl PropertySet {
    #[must_use]
    pub(crate) fn get(&self, ptype: ParamType) -> &[Property] {
        &self.props[ptype.index()]
    }
}

/// Reference to the output of another node that feeds an input parameter.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParamSource {
    pub src_node_index: i32,
    pub src_output_index: i32,
    pub flags: u32,
}

impl ParamSource {
    #[must_use]
    pub(crate) fn is_multi(&self) -> bool {
        self.src_node_index <= -100
    }
    #[must_use]
    pub(crate) fn multi_index(&self) -> usize {
        usize::try_from(-100 - self.src_node_index).unwrap_or(0)
    }
    #[must_use]
    pub(crate) fn multi_count(&self) -> usize {
        usize::try_from(self.src_output_index).unwrap_or(0)
    }
}

/// Where an input parameter draws its value from: one source or several.
#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    Single(ParamSource),
    Multi(Vec<ParamSource>),
}

/// An input parameter of a node, with its default value and source.
#[derive(Debug, Clone, PartialEq)]
pub struct InputParam {
    pub name: String,
    pub classname: String,
    pub ptype: ParamType,
    pub value: Value,
    pub source: Source,
    pub is_blackboard_input: bool,
}

/// An output parameter of a node.
#[derive(Debug, Clone, PartialEq)]
pub struct OutputParam {
    pub name: String,
    pub classname: String,
    pub ptype: ParamType,
    pub is_output: bool,
}

/// A node's input and output parameters, grouped by [`ParamType`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParamSet {
    pub inputs: [Vec<InputParam>; 6],
    pub outputs: [Vec<OutputParam>; 6],
}

impl ParamSet {
    #[must_use]
    pub(crate) fn inputs(&self, ptype: ParamType) -> &[InputParam] {
        &self.inputs[ptype.index()]
    }
    #[must_use]
    pub(crate) fn outputs(&self, ptype: ParamType) -> &[OutputParam] {
        &self.outputs[ptype.index()]
    }
}

/// A single blackboard parameter.
#[derive(Debug, Clone, PartialEq)]
pub struct BbParam {
    pub name: String,
    pub notes: String,
    pub file_ref: String,
    pub flags: u8,
    pub value: Value,
}

/// The graph's blackboard: shared parameters grouped by [`BbParamType`].
#[derive(Debug, Clone, Default)]
pub struct Blackboard {
    pub params: [Vec<BbParam>; 7],
}

#[derive(Debug, Clone, PartialEq)]
pub struct Attachment {
    pub name: String,
    pub debug: u32,
    pub properties: PropertySet,
    pub expr_count: u16,
    pub expr_io_size: u16,
}

/// A reference to another AINB file used as a child module.
#[derive(Debug, Clone, PartialEq)]
pub struct Module {
    pub path: String,
    pub category: String,
    pub instance_count: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Action {
    pub action_slot: String,
    pub action: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Transition {
    pub transition_type: u32,
    pub update_post_calc: bool,
    pub command_name: String,
}

/// Kind of edit recorded in a node-replacement table entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplacementType {
    Invalid,
    RemoveChild,
    ReplaceChild,
    RemoveAttachment,
}

impl ReplacementType {
    #[must_use]
    pub(crate) fn value(self) -> i32 {
        match self {
            ReplacementType::Invalid => -1,
            ReplacementType::RemoveChild => 0,
            ReplacementType::ReplaceChild => 1,
            ReplacementType::RemoveAttachment => 2,
        }
    }
    #[must_use]
    pub(crate) fn from_value(v: i32) -> Self {
        match v {
            0 => ReplacementType::RemoveChild,
            1 => ReplacementType::ReplaceChild,
            2 => ReplacementType::RemoveAttachment,
            _ => ReplacementType::Invalid,
        }
    }
    #[must_use]
    pub(crate) fn name(self) -> &'static str {
        match self {
            ReplacementType::Invalid => "Invalid",
            ReplacementType::RemoveChild => "RemoveChild",
            ReplacementType::ReplaceChild => "ReplaceChild",
            ReplacementType::RemoveAttachment => "RemoveAttachment",
        }
    }
    #[must_use]
    pub(crate) fn from_name(name: &str) -> Option<Self> {
        match name {
            "Invalid" => Some(ReplacementType::Invalid),
            "RemoveChild" => Some(ReplacementType::RemoveChild),
            "ReplaceChild" => Some(ReplacementType::ReplaceChild),
            "RemoveAttachment" => Some(ReplacementType::RemoveAttachment),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ReplacementEntry {
    pub rtype: ReplacementType,
    pub node_index: i32,
    pub replace_index: i32,
    pub new_index: i32,
}

#[derive(Debug, Clone)]
pub struct StateInfo {
    pub desired_state: String,
    pub unk04: u32,
    pub unk08: u32,
    pub unk0c: u32,
    pub unk10: u32,
}

#[derive(Debug, Clone)]
pub struct UnknownSection0x58 {
    pub description: String,
    pub unk04: u32,
    pub unk08: u32,
    pub unk0c: u32,
}

/// The kind of a node: a user-defined node or one of the built-in elements.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeType {
    UserDefined,
    ElementS32Selector,
    ElementSequential,
    ElementSimultaneous,
    ElementF32Selector,
    ElementStringSelector,
    ElementRandomSelector,
    ElementBoolSelector,
    ElementFork,
    ElementJoin,
    ElementAlert,
    ElementExpression,
    ElementModuleIfInputS32,
    ElementModuleIfInputF32,
    ElementModuleIfInputVec3f,
    ElementModuleIfInputString,
    ElementModuleIfInputBool,
    ElementModuleIfInputPtr,
    ElementModuleIfOutputS32,
    ElementModuleIfOutputF32,
    ElementModuleIfOutputVec3f,
    ElementModuleIfOutputString,
    ElementModuleIfOutputBool,
    ElementModuleIfOutputPtr,
    ElementModuleIfChild,
    ElementStateEnd,
    ElementSplitTiming,
}

impl NodeType {
    #[must_use]
    pub(crate) fn value(self) -> u16 {
        match self {
            NodeType::UserDefined => 0,
            NodeType::ElementS32Selector => 1,
            NodeType::ElementSequential => 2,
            NodeType::ElementSimultaneous => 3,
            NodeType::ElementF32Selector => 4,
            NodeType::ElementStringSelector => 5,
            NodeType::ElementRandomSelector => 6,
            NodeType::ElementBoolSelector => 7,
            NodeType::ElementFork => 8,
            NodeType::ElementJoin => 9,
            NodeType::ElementAlert => 10,
            NodeType::ElementExpression => 20,
            NodeType::ElementModuleIfInputS32 => 100,
            NodeType::ElementModuleIfInputF32 => 101,
            NodeType::ElementModuleIfInputVec3f => 102,
            NodeType::ElementModuleIfInputString => 103,
            NodeType::ElementModuleIfInputBool => 104,
            NodeType::ElementModuleIfInputPtr => 105,
            NodeType::ElementModuleIfOutputS32 => 200,
            NodeType::ElementModuleIfOutputF32 => 201,
            NodeType::ElementModuleIfOutputVec3f => 202,
            NodeType::ElementModuleIfOutputString => 203,
            NodeType::ElementModuleIfOutputBool => 204,
            NodeType::ElementModuleIfOutputPtr => 205,
            NodeType::ElementModuleIfChild => 300,
            NodeType::ElementStateEnd => 400,
            NodeType::ElementSplitTiming => 500,
        }
    }

    #[must_use]
    pub(crate) fn from_value(v: u16) -> Option<Self> {
        Some(match v {
            0 => NodeType::UserDefined,
            1 => NodeType::ElementS32Selector,
            2 => NodeType::ElementSequential,
            3 => NodeType::ElementSimultaneous,
            4 => NodeType::ElementF32Selector,
            5 => NodeType::ElementStringSelector,
            6 => NodeType::ElementRandomSelector,
            7 => NodeType::ElementBoolSelector,
            8 => NodeType::ElementFork,
            9 => NodeType::ElementJoin,
            10 => NodeType::ElementAlert,
            20 => NodeType::ElementExpression,
            100 => NodeType::ElementModuleIfInputS32,
            101 => NodeType::ElementModuleIfInputF32,
            102 => NodeType::ElementModuleIfInputVec3f,
            103 => NodeType::ElementModuleIfInputString,
            104 => NodeType::ElementModuleIfInputBool,
            105 => NodeType::ElementModuleIfInputPtr,
            200 => NodeType::ElementModuleIfOutputS32,
            201 => NodeType::ElementModuleIfOutputF32,
            202 => NodeType::ElementModuleIfOutputVec3f,
            203 => NodeType::ElementModuleIfOutputString,
            204 => NodeType::ElementModuleIfOutputBool,
            205 => NodeType::ElementModuleIfOutputPtr,
            300 => NodeType::ElementModuleIfChild,
            400 => NodeType::ElementStateEnd,
            500 => NodeType::ElementSplitTiming,
            _ => return None,
        })
    }

    #[must_use]
    pub(crate) fn name(self) -> &'static str {
        match self {
            NodeType::UserDefined => "UserDefined",
            NodeType::ElementS32Selector => "Element_S32Selector",
            NodeType::ElementSequential => "Element_Sequential",
            NodeType::ElementSimultaneous => "Element_Simultaneous",
            NodeType::ElementF32Selector => "Element_F32Selector",
            NodeType::ElementStringSelector => "Element_StringSelector",
            NodeType::ElementRandomSelector => "Element_RandomSelector",
            NodeType::ElementBoolSelector => "Element_BoolSelector",
            NodeType::ElementFork => "Element_Fork",
            NodeType::ElementJoin => "Element_Join",
            NodeType::ElementAlert => "Element_Alert",
            NodeType::ElementExpression => "Element_Expression",
            NodeType::ElementModuleIfInputS32 => "Element_ModuleIF_Input_S32",
            NodeType::ElementModuleIfInputF32 => "Element_ModuleIF_Input_F32",
            NodeType::ElementModuleIfInputVec3f => "Element_ModuleIF_Input_Vec3f",
            NodeType::ElementModuleIfInputString => "Element_ModuleIF_Input_String",
            NodeType::ElementModuleIfInputBool => "Element_ModuleIF_Input_Bool",
            NodeType::ElementModuleIfInputPtr => "Element_ModuleIF_Input_Ptr",
            NodeType::ElementModuleIfOutputS32 => "Element_ModuleIF_Output_S32",
            NodeType::ElementModuleIfOutputF32 => "Element_ModuleIF_Output_F32",
            NodeType::ElementModuleIfOutputVec3f => "Element_ModuleIF_Output_Vec3f",
            NodeType::ElementModuleIfOutputString => "Element_ModuleIF_Output_String",
            NodeType::ElementModuleIfOutputBool => "Element_ModuleIF_Output_Bool",
            NodeType::ElementModuleIfOutputPtr => "Element_ModuleIF_Output_Ptr",
            NodeType::ElementModuleIfChild => "Element_ModuleIF_Child",
            NodeType::ElementStateEnd => "Element_StateEnd",
            NodeType::ElementSplitTiming => "Element_SplitTiming",
        }
    }

    #[must_use]
    pub(crate) fn from_name(name: &str) -> Option<Self> {
        (0..=500u16)
            .filter_map(NodeType::from_value)
            .find(|t| t.name() == name)
    }

    #[must_use]
    pub(crate) fn is_output(self) -> bool {
        let v = self.value();
        (200..300).contains(&v)
    }
}

pub(super) const PLUG_TYPE_COUNT: usize = 10;

pub(super) const PLUG_TYPE_NAMES: [&str; PLUG_TYPE_COUNT] = [
    "Generic",
    "_01",
    "Child",
    "Transition",
    "String",
    "Int",
    "_06",
    "_07",
    "_08",
    "_09",
];

/// A connection from a node to another node, with type-specific payload such
/// as selector conditions or transitions.
#[derive(Debug, Clone, PartialEq)]
pub enum Plug {
    Generic {
        node_index: i32,
        name: String,
    },
    BoolSelectorInput {
        node_index: i32,
        name: String,
        unk0: u32,
        unk1: u32,
    },
    F32SelectorInput {
        node_index: i32,
        name: String,
        unk0: u32,
        unk1: f32,
    },
    Child {
        node_index: i32,
        name: String,
    },
    S32Selector {
        node_index: i32,
        name: String,
        condition: i32,
        is_default: bool,
        blackboard_index: i32,
    },
    F32Selector {
        node_index: i32,
        name: String,
        condition_min: f32,
        blackboard_index_min: i32,
        condition_max: f32,
        blackboard_index_max: i32,
        is_default: bool,
    },
    StringSelector {
        node_index: i32,
        name: String,
        condition: String,
        is_default: bool,
        blackboard_index: i32,
    },
    RandomSelector {
        node_index: i32,
        name: String,
        blackboard_index: i32,
        weight: f32,
    },
    BsaSelectorUpdater {
        node_index: i32,
        name: String,
        child_enum_bb_index: i32,
        child_enum_value: u32,
    },
    Transition {
        node_index: i32,
        transition: Transition,
    },
    StringSelectorInput {
        node_index: i32,
        name: String,
        unknown: u32,
        default_value: String,
        read_extra: bool,
    },
    S32SelectorInput {
        node_index: i32,
        name: String,
        unknown: u32,
        default_value: i32,
        read_extra: bool,
    },
}

impl Plug {
    #[must_use]
    pub(crate) fn size(&self) -> usize {
        match self {
            Plug::Generic { .. } | Plug::Child { .. } | Plug::Transition { .. } => 0x8,
            Plug::StringSelectorInput { read_extra, .. }
            | Plug::S32SelectorInput { read_extra, .. } => {
                if *read_extra {
                    0x10
                } else {
                    0x8
                }
            }
            Plug::F32Selector { .. } => 0x28,
            _ => 0x10,
        }
    }
}

/// A single node in the graph, with its properties, parameters, and outgoing
/// plugs.
#[derive(Debug, Clone)]
pub struct Node {
    pub name: String,
    pub ntype: NodeType,
    pub index: i32,
    pub flags: u8,
    pub queries: Vec<i32>,
    pub attachments: Vec<Attachment>,
    pub properties: PropertySet,
    pub params: ParamSet,
    pub actions: Vec<Action>,
    pub guid: String,
    pub state_info: Option<StateInfo>,
    pub plugs: [Vec<Plug>; PLUG_TYPE_COUNT],
    pub expr_count: u16,
    pub expr_io_size: u16,
}

impl Node {
    /// Whether this node is a query node (set in [`Node::flags`]).
    #[must_use]
    pub fn is_query(&self) -> bool {
        self.flags & 1 != 0
    }
    #[must_use]
    pub(crate) fn is_module(&self) -> bool {
        self.flags & 2 != 0
    }
    #[must_use]
    pub(crate) fn is_root_node(&self) -> bool {
        self.flags & 4 != 0
    }
    #[must_use]
    pub(crate) fn is_multi_param_type2(&self) -> bool {
        self.flags & 8 != 0
    }
}

#[must_use]
pub(crate) fn is_expression_flag(flags: u32) -> bool {
    flags & 0xc200_0000 == 0xc200_0000
}

#[must_use]
pub(crate) fn is_blackboard_flag(flags: u32) -> bool {
    flags & 0xc200_0000 != 0xc200_0000 && flags & 0xc200_0000 != 0
}

#[must_use]
pub(crate) fn flag_index(flags: u32) -> u32 {
    flags & 0xffff
}

#[must_use]
pub(crate) fn flag_uses_default(flags: u32) -> bool {
    flags & 0x80_0000 != 0
}

#[must_use]
pub(crate) fn flag_is_output(flags: u32) -> bool {
    flags & 0x100_0000 != 0
}

#[must_use]
pub(crate) fn flag_vector_component(flags: u32) -> u32 {
    (flags >> 0x1a) & 3
}
