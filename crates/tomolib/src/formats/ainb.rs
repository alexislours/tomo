pub mod exb;
mod model;
mod read;
mod write;
mod yaml;

pub use exb::Exb;
pub use model::{
    Action, Attachment, BbParam, BbParamType, Blackboard, Command, InputParam, Module, Node,
    NodeType, OutputParam, ParamSet, ParamSource, ParamType, Plug, Property, PropertySet,
    ReplacementEntry, ReplacementType, Source, StateInfo, Transition, UnknownSection0x58, Value,
};

use crate::{Error, Result};

pub const AINB_MAGIC: [u8; 4] = *b"AIB ";
pub const SUPPORTED_VERSIONS: [u32; 3] = [0x404, 0x407, 0x408];

/// A parsed AINB (AI node binary) graph: its commands, nodes, blackboard, and
/// optional expression (EXB) section.
#[derive(Debug, Clone)]
pub struct Ainb {
    pub version: u32,
    pub filename: String,
    pub category: String,
    pub blackboard_id: u32,
    pub parent_blackboard_id: u32,
    pub commands: Vec<Command>,
    pub nodes: Vec<Node>,
    pub blackboard: Option<Blackboard>,
    pub expressions: Option<Exb>,
    pub replacement_table: Vec<ReplacementEntry>,
    pub modules: Vec<Module>,
    pub unk_section0x58: Option<UnknownSection0x58>,
    pub exists_section_0x6c: bool,
}

impl Ainb {
    /// Parses an AINB file. Supported versions are listed in
    /// [`SUPPORTED_VERSIONS`].
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        read::read(bytes)
    }

    /// Serializes the graph back to the binary AINB format.
    #[must_use]
    pub fn to_binary(&self) -> Vec<u8> {
        write::write(self)
    }

    /// Renders the graph as the editable YAML representation accepted by
    /// [`Ainb::from_yaml`].
    pub fn to_yaml(&self) -> Result<String> {
        Ok(yaml::emit(self))
    }

    /// Parses the YAML representation produced by [`Ainb::to_yaml`].
    pub fn from_yaml(text: &str) -> Result<Self> {
        yaml::parse(text)
    }

    #[must_use]
    pub(crate) fn category_id(&self) -> Option<u32> {
        match self.category.as_str() {
            "AI" => Some(0),
            "Logic" => Some(1),
            "Sequence" => Some(2),
            _ => None,
        }
    }
}

pub(crate) fn check_version(version: u32) -> Result<()> {
    if SUPPORTED_VERSIONS.contains(&version) {
        Ok(())
    } else {
        Err(Error::unsupported(format!(
            "unsupported AINB version {version:#x}"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use model::{NodeType, ParamType, Property, Value};

    const GUID: &str = "01234567-89ab-cdef-0123-456789abcdef";

    fn sample() -> Ainb {
        let mut node = Node {
            name: "TestNode".into(),
            ntype: NodeType::UserDefined,
            index: 0,
            flags: 0,
            queries: vec![],
            attachments: vec![],
            properties: model::PropertySet::default(),
            params: model::ParamSet::default(),
            actions: vec![],
            guid: GUID.into(),
            state_info: None,
            plugs: Default::default(),
            expr_count: 0,
            expr_io_size: 0,
        };
        node.properties.props[ParamType::Int.index()].push(Property {
            name: "Threshold".into(),
            classname: String::new(),
            ptype: ParamType::Int,
            flags: 0,
            value: Value::Int(42),
        });
        node.properties.props[ParamType::Float.index()].push(Property {
            name: "Speed".into(),
            classname: String::new(),
            ptype: ParamType::Float,
            flags: 0,
            value: Value::Float(1.5),
        });

        Ainb {
            version: 0x408,
            filename: "sample".into(),
            category: "AI".into(),
            blackboard_id: 7,
            parent_blackboard_id: 0,
            commands: vec![Command {
                name: "Root".into(),
                guid: GUID.into(),
                root_node_index: 0,
                secondary_root_node_index: -1,
            }],
            nodes: vec![node],
            blackboard: Some(Blackboard::default()),
            expressions: None,
            replacement_table: vec![],
            modules: vec![],
            unk_section0x58: None,
            exists_section_0x6c: true,
        }
    }

    #[test]
    fn rejects_unknown_version() {
        assert!(check_version(0x999).is_err());
        assert!(check_version(0x408).is_ok());
    }

    #[test]
    fn binary_round_trip_is_stable() {
        let a = sample();
        let bin = a.to_binary();
        assert_eq!(&bin[0..4], b"AIB ");
        let b = Ainb::parse(&bin).expect("parse own output");
        assert_eq!(b.filename, "sample");
        assert_eq!(b.category, "AI");
        assert_eq!(b.blackboard_id, 7);
        assert_eq!(b.commands.len(), 1);
        assert_eq!(b.nodes.len(), 1);
        assert_eq!(b.nodes[0].name, "TestNode");
        assert_eq!(
            b.nodes[0].properties.get(ParamType::Int)[0].value,
            Value::Int(42)
        );
        assert_eq!(b.to_binary(), bin);
    }

    #[test]
    fn yaml_round_trip_matches_binary() {
        let a = sample();
        let yaml = a.to_yaml().expect("emit yaml");
        let b = Ainb::from_yaml(&yaml).expect("parse yaml");
        assert_eq!(a.to_binary(), b.to_binary());
    }
}
