//! A VS Code-style Source Control panel for the left tool panel.

pub mod git;
mod graph;
mod view;

pub use view::{SourceControlEvent, SourceControlView};
