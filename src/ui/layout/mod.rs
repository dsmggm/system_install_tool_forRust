//! Layout builder modules for the main application window.
//!
//! Each module provides a `create()` function that constructs a specific
//! UI section and returns a typed struct with widget handles.

mod disk_tree;
mod footer;
mod gho_options;
mod header;
mod install_source;
mod target_partition;
mod wim_options;

pub use disk_tree::create_disk_tree;
pub use footer::create_footer;
pub use gho_options::create_gho_options;
pub use header::create_header;
pub use install_source::create_install_source;
pub use target_partition::create_target_partition;
pub use wim_options::create_wim_options;