//! Bounded values and target references shared by public AW contracts.

use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

/// Maximum UTF-8 byte length of user-facing contract text.
pub const MAX_TEXT_BYTES: usize = 4096;
/// Maximum UTF-8 byte length of names used for authorities and operations.
pub const MAX_NAME_BYTES: usize = 128;
/// Maximum UTF-8 byte length of opaque external values.
pub const MAX_OPAQUE_BYTES: usize = 1024;
/// Maximum UTF-8 byte length of an idempotency key.
pub const MAX_IDEMPOTENCY_KEY_BYTES: usize = 256;

/// Failure returned when a bounded string violates its construction contract.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum BoundedStringError {
    /// Empty values do not carry usable contract meaning.
    #[error("value must not be empty")]
    Empty,
    /// The UTF-8 representation exceeds the type-specific byte limit.
    #[error("value exceeds the {max_bytes}-byte limit")]
    TooLong {
        /// Maximum accepted UTF-8 byte count.
        max_bytes: usize,
    },
    /// NUL bytes are forbidden at transport and operating-system boundaries.
    #[error("value must not contain a NUL character")]
    ContainsNul,
    /// Canonical names are stable protocol labels, not presentation text.
    #[error("name must use only non-space printable ASCII characters (`!` through `~`)")]
    InvalidNameCharacter,
}

fn validate_bounded(value: &str, max_bytes: usize) -> Result<(), BoundedStringError> {
    if value.is_empty() {
        return Err(BoundedStringError::Empty);
    }
    if value.len() > max_bytes {
        return Err(BoundedStringError::TooLong { max_bytes });
    }
    if value.contains('\0') {
        return Err(BoundedStringError::ContainsNul);
    }
    Ok(())
}

fn allow_any_bounded_value(_value: &str) -> Result<(), BoundedStringError> {
    Ok(())
}

fn validate_stable_name(value: &str) -> Result<(), BoundedStringError> {
    if !value.bytes().all(|byte| byte.is_ascii_graphic()) {
        return Err(BoundedStringError::InvalidNameCharacter);
    }
    Ok(())
}

macro_rules! bounded_string {
    ($name:ident, $max:ident, $doc:literal) => {
        bounded_string!($name, $max, $doc, allow_any_bounded_value);
    };
    ($name:ident, $max:ident, $doc:literal, $validate:ident) => {
        #[doc = $doc]
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(String);

        impl $name {
            /// Constructs a validated bounded value.
            pub fn new(value: impl Into<String>) -> Result<Self, BoundedStringError> {
                let value = value.into();
                validate_bounded(&value, $max)?;
                $validate(&value)?;
                Ok(Self(value))
            }

            /// Returns the validated text value.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(self.as_str())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Self::new(value).map_err(de::Error::custom)
            }
        }
    };
}

bounded_string!(
    BoundedText,
    MAX_TEXT_BYTES,
    "User-facing text whose serialized size is bounded."
);
bounded_string!(
    BoundedName,
    MAX_NAME_BYTES,
    "A canonical stable name using 1-128 non-space printable ASCII characters.",
    validate_stable_name
);
bounded_string!(
    BoundedOpaque,
    MAX_OPAQUE_BYTES,
    "An opaque external value with a strict serialized-size limit."
);
bounded_string!(
    IdempotencyKey,
    MAX_IDEMPOTENCY_KEY_BYTES,
    "A caller-scoped key used to replay command admission safely."
);

/// Error returned when a digest is not canonical lowercase SHA-256 text.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("digest must contain exactly 64 lowercase hexadecimal characters")]
pub struct DigestError;

/// Canonical lowercase hexadecimal representation of a SHA-256 digest.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Digest(String);

impl Digest {
    /// Parses a lowercase 64-character SHA-256 digest.
    pub fn parse(value: impl Into<String>) -> Result<Self, DigestError> {
        let value = value.into();
        if value.len() != 64
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(DigestError);
        }
        Ok(Self(value))
    }

    /// Returns the canonical hexadecimal representation.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Serialize for Digest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Digest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(de::Error::custom)
    }
}

/// Opaque operating-system or remote environment selected for governed work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetRef {
    /// Target provider or environment kind.
    pub kind: BoundedName,
    /// Authority that owns the target namespace.
    pub authority: BoundedName,
    /// Opaque target identifier within the authority.
    pub identifier: BoundedOpaque,
}

#[cfg(test)]
mod tests {
    use super::{
        BoundedName, BoundedOpaque, BoundedStringError, BoundedText, IdempotencyKey, MAX_NAME_BYTES,
    };

    #[test]
    fn bounded_names_use_the_canonical_stable_label_subset() {
        for accepted in ["!", "~", "text/plain", "security.code.inspect/v1"] {
            assert_eq!(
                BoundedName::new(accepted)
                    .expect("non-space printable ASCII is a stable name")
                    .as_str(),
                accepted
            );
        }
        BoundedName::new("a".repeat(MAX_NAME_BYTES))
            .expect("the exact stable-name byte limit is accepted");

        for rejected in ["has space", "line\nbreak", "tab\tname", "café", "\u{7f}"] {
            assert_eq!(
                BoundedName::new(rejected),
                Err(BoundedStringError::InvalidNameCharacter),
                "{rejected:?} must not cross the canonical name boundary"
            );
        }
        assert_eq!(BoundedName::new(""), Err(BoundedStringError::Empty));
        assert_eq!(
            BoundedName::new("a".repeat(MAX_NAME_BYTES + 1)),
            Err(BoundedStringError::TooLong {
                max_bytes: MAX_NAME_BYTES,
            })
        );
        assert_eq!(
            BoundedName::new("nul\0name"),
            Err(BoundedStringError::ContainsNul)
        );
    }

    #[test]
    fn bounded_name_deserialization_applies_the_same_stable_label_rule() {
        assert!(serde_json::from_value::<BoundedName>(serde_json::json!("tool_name")).is_ok());
        assert!(serde_json::from_value::<BoundedName>(serde_json::json!("tool name")).is_err());
        assert!(serde_json::from_value::<BoundedName>(serde_json::json!("工具")).is_err());
    }

    #[test]
    fn other_bounded_values_retain_text_and_opaque_semantics() {
        for value in ["presentation text", "工具"] {
            BoundedText::new(value).expect("bounded text permits display characters");
            BoundedOpaque::new(value).expect("bounded opaque values preserve their payload");
            IdempotencyKey::new(value).expect("bounded idempotency keys remain opaque");
        }
    }
}
