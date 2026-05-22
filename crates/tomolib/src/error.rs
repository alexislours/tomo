use thiserror::Error;

/// Result alias used throughout the crate, defaulting to [`enum@Error`].
pub type Result<T, E = Error> = core::result::Result<T, E>;

/// An error produced while parsing or writing a file format.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error("not a {format} file (bad magic)")]
    BadMagic { format: &'static str },

    #[error("{context}: needs {needed} bytes at offset {offset}, but only {available} available")]
    Truncated {
        context: &'static str,
        offset: usize,
        needed: usize,
        available: usize,
    },

    #[error("{context}: index {index} out of range (len {len})")]
    OutOfRange {
        context: &'static str,
        index: usize,
        len: usize,
    },

    #[error("{0}")]
    Overflow(String),

    #[error("{context}: invalid UTF-8")]
    InvalidUtf8 { context: &'static str },

    #[error("{0}")]
    Unsupported(String),

    #[error("{0}")]
    Decode(String),

    #[error("{0}")]
    Malformed(String),
}

impl Error {
    pub(crate) fn bad_magic(format: &'static str) -> Self {
        Self::BadMagic { format }
    }

    pub(crate) fn truncated(
        context: &'static str,
        offset: usize,
        needed: usize,
        available: usize,
    ) -> Self {
        Self::Truncated {
            context,
            offset,
            needed,
            available,
        }
    }

    pub(crate) fn out_of_range(context: &'static str, index: usize, len: usize) -> Self {
        Self::OutOfRange {
            context,
            index,
            len,
        }
    }

    pub(crate) fn overflow(msg: impl Into<String>) -> Self {
        Self::Overflow(msg.into())
    }

    pub(crate) fn invalid_utf8(context: &'static str) -> Self {
        Self::InvalidUtf8 { context }
    }

    pub(crate) fn unsupported(msg: impl Into<String>) -> Self {
        Self::Unsupported(msg.into())
    }

    pub(crate) fn decode(msg: impl Into<String>) -> Self {
        Self::Decode(msg.into())
    }

    pub(crate) fn malformed(msg: impl Into<String>) -> Self {
        Self::Malformed(msg.into())
    }
}

impl From<std::num::ParseIntError> for Error {
    fn from(e: std::num::ParseIntError) -> Self {
        Self::Decode(e.to_string())
    }
}

impl From<std::num::ParseFloatError> for Error {
    fn from(e: std::num::ParseFloatError) -> Self {
        Self::Decode(e.to_string())
    }
}
