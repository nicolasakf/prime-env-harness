//! Live viewer perspectives: whose eyes an environment's `frame` is drawn for.
use super::SeatId;
use serde::{Deserialize, Serialize};

/// The audience of a live viewer frame. The environment's `frame(perspective)`
/// filters by it exactly like `observe`: a public frame holds only what every
/// spectator may see, and a seat frame adds only that seat's private view.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum ViewPerspective {
    Public,
    Seat { seat: SeatId },
}

/// Largest frame a live viewer accepts from `frame(perspective)`.
pub const MAX_VIEW_FRAME_BYTES: usize = 1024 * 1024;
