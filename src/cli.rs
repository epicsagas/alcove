// cli.rs — thin re-export layer
//
// All logic has been moved to dedicated modules:
//   • agents.rs  — Agent definitions, MCP config writers, skill installer
//   • setup/     — Interactive setup wizard (8-step state machine)
//   • commands.rs — CLI command implementations

pub use crate::setup::cmd_setup;

pub use crate::commands::{
    cmd_doctor,
    cmd_index,
    cmd_lint,
    cmd_promote,
    cmd_rebuild,
    cmd_search,
    cmd_token,
    cmd_uninstall,
    cmd_validate,
};

#[cfg(feature = "alcove-full")]
pub use crate::commands::cmd_model;

#[cfg(unix)]
pub use crate::commands::cmd_reap;
