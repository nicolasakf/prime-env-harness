//! Validated seat identities and role names; serialized as plain strings.
use serde::{Deserialize, Serialize};

macro_rules! identifier {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        impl $name {
            #[must_use]
            pub fn as_str(&self) -> &str { &self.0 }

            #[must_use]
            pub fn legacy_human() -> Self { Self("human".into()) }

            #[must_use]
            pub fn legacy_agent() -> Self { Self("agent".into()) }
        }

        impl TryFrom<String> for $name {
            type Error = String;
            fn try_from(value: String) -> Result<Self, Self::Error> {
                if value.is_empty() || value.len() > 64 || !value.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')) {
                    return Err("seat and role identifiers require 1–64 ASCII letters, digits, hyphens or underscores".into());
                }
                Ok(Self(value))
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self { value.0 }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { self.0.fmt(formatter) }
        }
    };
}

identifier!(SeatId);
identifier!(RoleName);
