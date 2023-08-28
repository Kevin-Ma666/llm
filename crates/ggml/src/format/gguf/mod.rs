#![allow(missing_docs)]

use std::{
    collections::HashMap,
    io::{BufRead, Seek},
};

use crate::{util, ElementType};


mod metadata;
pub use metadata::*;

pub const DEFAULT_ALIGNMENT: u32 = 32;

#[derive(Debug, thiserror::Error)]
/// Errors that can occur while loading a model.
pub enum GgufLoadError {
    #[error("invalid GGUF file magic value: {0}")]
    /// The file magic number is invalid.
    InvalidMagic(util::FileMagic),
    #[error("invalid ggml format: format={0:?}")]
    /// An unsupported format version was found.
    InvalidFormatVersion(ContainerType),
    #[error("non-specific I/O error")]
    /// A non-specific IO error.
    Io(#[from] std::io::Error),
    #[error("could not convert bytes to a UTF-8 string")]
    /// One of the strings encountered was not valid UTF-8.
    InvalidUtf8(#[from] std::string::FromUtf8Error),
    #[error("invalid integer conversion")]
    /// One of the integers encountered could not be converted to a more appropriate type.
    InvalidIntegerConversion(#[from] std::num::TryFromIntError),
    #[error("unsupported tensor type {ftype} for tensor {tensor_name}")]
    /// One of the tensors encountered had an unsupported data type.
    UnsupportedElementType {
        /// The name of the tensor.
        tensor_name: String,
        /// The format type that was encountered.
        ftype: u32,
    },
    #[error("invariant broken: {0}")]
    /// An invariant was broken.
    InvariantBroken(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Gguf {
    pub metadata: HashMap<String, MetadataValue>,
    pub tensor_infos: HashMap<String, TensorInfo>,
    pub tensor_data_position: u64,
}
impl Gguf {
    pub fn load<R: BufRead + Seek>(reader: &mut R) -> Result<Self, GgufLoadError> {
        let container = ContainerType::read(reader).map_err(|e| match e {
            ContainerTypeReadError::InvalidMagic(magic) => GgufLoadError::InvalidMagic(magic),
            ContainerTypeReadError::Io(io) => GgufLoadError::Io(io),
        })?;
        if ![ContainerType::Gguf(1), ContainerType::Gguf(2)].contains(&container) {
            return Err(GgufLoadError::InvalidFormatVersion(container));
        }

        let ctx = GgufContext {
            use_64_bit_length: container == ContainerType::Gguf(2),
        };

        let tensor_count = util::read_length(reader, ctx.use_64_bit_length)?;
        let metadata_kv_count = util::read_length(reader, ctx.use_64_bit_length)?;

        let mut metadata = HashMap::with_capacity(metadata_kv_count);
        for _ in 0..metadata_kv_count {
            let (key, value) = MetadataValue::read_key_value(&ctx, reader)?;
            metadata.insert(key, value);
        }

        let alignment = metadata
            .get("general.alignment")
            .and_then(|v| v.as_uint32())
            .unwrap_or(DEFAULT_ALIGNMENT) as u64;

        let mut tensor_infos = HashMap::with_capacity(tensor_count);
        for _ in 0..tensor_count {
            let (key, value) = TensorInfo::read_name_value(&ctx, reader)?;
            tensor_infos.insert(key, value);
        }

        let tensor_data_position = {
            let stream_position = reader.stream_position()?;
            stream_position + (alignment - (stream_position % alignment)) % alignment
        };

        Ok(Gguf {
            metadata,
            tensor_infos,
            tensor_data_position,
        })
    }
}

struct GgufContext {
    use_64_bit_length: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TensorInfo {
    pub dimensions: Vec<usize>,
    pub element_type: ElementType,
    pub offset: u64,
}
impl TensorInfo {
    fn read_name_value(
        ctx: &GgufContext,
        reader: &mut dyn BufRead,
    ) -> Result<(String, Self), GgufLoadError> {
        let name = util::read_string(reader, ctx.use_64_bit_length)?;

        let dimension_count = util::read_u32(reader)? as usize;
        let dimensions = (0..dimension_count)
            .map(|_| util::read_length(reader, ctx.use_64_bit_length))
            .collect::<Result<Vec<_>, _>>()?;

        let element_type = util::read_u32(reader)?;
        let element_type = ElementType::try_from(element_type).map_err(|_| {
            GgufLoadError::UnsupportedElementType {
                tensor_name: name.clone(),
                ftype: element_type,
            }
        })?;

        let offset = util::read_u64(reader)?;

        Ok((
            name,
            Self {
                dimensions,
                element_type,
                offset,
            },
        ))
    }
}