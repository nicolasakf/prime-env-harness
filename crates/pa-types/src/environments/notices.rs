//! Environment-selected attention, completion and progress metadata.
use super::{Deserialize, SeatId, Serialize};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoticeTarget {
    All,
    Seats(Vec<SeatId>),
}

impl Serialize for NoticeTarget {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::All => "*".serialize(serializer),
            Self::Seats(seats) => seats.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for NoticeTarget {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Wire {
            All(String),
            Seats(Vec<SeatId>),
        }
        match Wire::deserialize(deserializer)? {
            Wire::All(value) if value == "*" => Ok(Self::All),
            Wire::All(_) => Err(serde::de::Error::custom(
                "notice target must be '*' or a list of seat IDs",
            )),
            Wire::Seats(seats) => Ok(Self::Seats(seats)),
        }
    }
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentNotice {
    pub to: NoticeTarget,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default = "yes")]
    pub wake: bool,
    /// A task for its recipients to act on while the environment awaits them.
    /// The host drops it from a wake once the environment no longer awaits the
    /// recipient for it; it needs `wake` and is not a message.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub actionable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentFinish {
    pub summary: String,
}

/// Optional environment-owned progress and explicit wait state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentProgress {
    pub revision: u64,
    #[serde(default)]
    pub waiting_for: Vec<SeatId>,
}
