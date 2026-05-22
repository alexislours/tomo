use crate::formats::binio::ByteOrder;
use crate::{Error, Result};

const LE: ByteOrder = ByteOrder::Little;
const SUPPORTED_VERSIONS: [u32; 3] = [1, 2, 3];

/// The EXB expression section embedded in an [`Ainb`](crate::formats::ainb::Ainb).
///
/// The raw block is preserved verbatim; the accessors expose its summary
/// counts.
#[derive(Debug, Clone)]
pub struct Exb {
    raw: Vec<u8>,
    version: u32,
    expression_count: u32,
    instruction_count: u32,
}

impl Exb {
    pub(crate) fn parse(raw: Vec<u8>) -> Result<Self> {
        if raw.len() < 0x2c || &raw[0..4] != b"EXB " {
            return Err(Error::bad_magic("EXB"));
        }
        let version = LE.read_u32(&raw, 4, "EXB version")?;
        if !SUPPORTED_VERSIONS.contains(&version) {
            return Err(Error::unsupported(format!(
                "unsupported EXB version {version:#x}"
            )));
        }
        LE.read_u32(&raw, 0x0c, "EXB instance count")?;
        let expression_offset = LE.read_u32(&raw, 0x18, "EXB expression offset")? as usize;
        let instruction_offset = LE.read_u32(&raw, 0x1c, "EXB instruction offset")? as usize;
        let signature_offset = LE.read_u32(&raw, 0x20, "EXB signature offset")? as usize;
        let expression_count = LE.read_u32(&raw, expression_offset, "EXB expression count")?;
        let instruction_count = LE.read_u32(&raw, instruction_offset, "EXB instruction count")?;
        LE.read_u32(&raw, signature_offset, "EXB signature count")?;
        Ok(Self {
            raw,
            version,
            expression_count,
            instruction_count,
        })
    }

    #[must_use]
    pub(crate) fn raw(&self) -> &[u8] {
        &self.raw
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.raw.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.raw.is_empty()
    }

    #[must_use]
    pub fn version(&self) -> u32 {
        self.version
    }

    #[must_use]
    pub fn expression_count(&self) -> u32 {
        self.expression_count
    }

    #[must_use]
    pub fn instruction_count(&self) -> u32 {
        self.instruction_count
    }
}
