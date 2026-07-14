#![allow(clippy::new_without_default)]

mod composer;
mod loc;
pub mod map;
mod recomposer;
mod scope;
mod state;
mod subcompose;
pub mod utils;

pub use slab;

pub use self::composer::{AnyData, Composable, ComposeNode, Composer, Node, NodeKey};
pub use self::loc::Loc;
pub use self::recomposer::Recomposer;
pub use self::scope::{Root, Scope, ScopeId};
pub use self::state::{State, StateId, StateTracker, StateTrackerRef, StateTrackerWeakRef};
pub use self::subcompose::{SlotId, SubcomposeResult, SubcomposeScope};
