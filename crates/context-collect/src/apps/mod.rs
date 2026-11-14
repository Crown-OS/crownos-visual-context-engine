//! Which application is behind a pid: the systemd cgroup names it, the desktop entry describes it.

mod cgroup;
mod desktop;
mod resolver;

pub use cgroup::{CgroupIdentity, parse_cgroup, unified_path};
pub use desktop::{DesktopEntry, DesktopIndex};
pub use resolver::AppResolver;
