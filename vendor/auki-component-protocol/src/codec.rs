//! Explicit observation codecs. Control envelopes remain SDK-owned JSON; only
//! the bounded payload frame is application encoded.

use std::sync::Arc;

use serde::{Serialize, de::DeserializeOwned};

use crate::ComponentProtocolError;

type Encode<T> = dyn Fn(&T) -> Result<Vec<u8>, String> + Send + Sync;
type Decode<T> = dyn Fn(&[u8]) -> Result<T, String> + Send + Sync;

/// Payload encoding selected by the publisher's declared application contract.
/// Bytes are written directly, without a JSON/base64 wrapper. This does not
/// change the source Output, sequence, timestamp, frame or clock envelope.
pub struct ObservationEncoder<T> {
    encoding: Arc<str>,
    encode: Arc<Encode<T>>,
}

impl<T> Clone for ObservationEncoder<T> {
    fn clone(&self) -> Self {
        Self {
            encoding: Arc::clone(&self.encoding),
            encode: Arc::clone(&self.encode),
        }
    }
}

impl<T> ObservationEncoder<T> {
    pub fn new(
        encoding: impl Into<String>,
        encode: impl Fn(&T) -> Result<Vec<u8>, String> + Send + Sync + 'static,
    ) -> Result<Self, ComponentProtocolError> {
        Ok(Self {
            encoding: validate_encoding(encoding.into())?,
            encode: Arc::new(encode),
        })
    }

    pub fn encoding(&self) -> &str {
        &self.encoding
    }

    pub(crate) fn encode(&self, value: &T) -> Result<Vec<u8>, String> {
        (self.encode)(value)
    }
}

impl<T: Serialize> ObservationEncoder<T> {
    pub(crate) fn json() -> Self {
        Self {
            encoding: Arc::from("application/json"),
            encode: Arc::new(|value| serde_json::to_vec(value).map_err(|e| e.to_string())),
        }
    }
}

/// Decoder selected explicitly from the Product's application contract.
/// An observation advertising any other encoding is rejected, never guessed.
pub struct ObservationDecoder<T> {
    encoding: Arc<str>,
    decode: Arc<Decode<T>>,
}

impl<T> Clone for ObservationDecoder<T> {
    fn clone(&self) -> Self {
        Self {
            encoding: Arc::clone(&self.encoding),
            decode: Arc::clone(&self.decode),
        }
    }
}

impl<T> ObservationDecoder<T> {
    pub fn new(
        encoding: impl Into<String>,
        decode: impl Fn(&[u8]) -> Result<T, String> + Send + Sync + 'static,
    ) -> Result<Self, ComponentProtocolError> {
        Ok(Self {
            encoding: validate_encoding(encoding.into())?,
            decode: Arc::new(decode),
        })
    }

    pub fn encoding(&self) -> &str {
        &self.encoding
    }

    pub(crate) fn decode(&self, bytes: &[u8]) -> Result<T, ComponentProtocolError> {
        (self.decode)(bytes).map_err(ComponentProtocolError::Codec)
    }
}

impl<T: DeserializeOwned> ObservationDecoder<T> {
    pub(crate) fn json() -> Self {
        Self {
            encoding: Arc::from("application/json"),
            decode: Arc::new(|bytes| serde_json::from_slice(bytes).map_err(|e| e.to_string())),
        }
    }
}

fn validate_encoding(value: String) -> Result<Arc<str>, ComponentProtocolError> {
    if value.is_empty() || value.len() > 128 || !value.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(ComponentProtocolError::InvalidRequest(
            "payload encoding must be 1..=128 printable ASCII characters without spaces".into(),
        ));
    }
    Ok(Arc::from(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoding_names_are_explicit_and_bounded() {
        for name in ["".to_owned(), " ".to_owned(), "x\n".to_owned(), "x".repeat(129)] {
            assert!(ObservationEncoder::<()>::new(name.clone(), |_| Ok(vec![])).is_err());
            assert!(ObservationDecoder::<()>::new(name, |_| Ok(())).is_err());
        }
        assert!(ObservationEncoder::<()>::new("application/octet-stream", |_| Ok(vec![])).is_ok());
    }
}
