//! Platform-independent core of dragonwilds-trainer.
//!
//! The core never touches a process directly. It reads game memory through the
//! [`Memory`] trait and only *decides*; the platform crates (Windows DLL now,
//! Linux `LD_PRELOAD` for dedicated servers later) execute the decisions.
//! Everything is resolved by name at runtime, so a game patch that moves code
//! or fields does not break the trainer as long as the names stay.

pub mod features;
pub mod layout;
pub mod memory;
pub mod reflection;

pub use features::Toggles;
pub use layout::Layout;
pub use memory::{Memory, MemoryExt};
pub use reflection::{FunctionInfo, PropertyInfo, Reflection, TypeDump};
