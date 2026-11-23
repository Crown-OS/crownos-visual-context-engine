use std::cell::RefCell;
use std::collections::HashMap;
use std::time::Duration;

use atspi::ObjectRefOwned;
use context_core::WindowRecord;
use tokio::time::Instant;

use super::fetch::gather_until;
use super::health::ObjectHealth;
use super::reader::AccessibilityBusConnection;
use super::toolkit::Toolkit;
use crate::apps::AppResolver;

const ROOT_PATH: &str = "/org/a11y/atspi/accessible/root";
/// The budget for learning who an application is.
const DESCRIBE_BUDGET: Duration = Duration::from_millis(200);
/// Unique names are never reused on a bus, so stale entries only cost memory.
const CAPACITY: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ApplicationInfo {
    pub(crate) pid: i32,
    pub(crate) toolkit: Toolkit,
}

/// Which process and toolkit sit behind each application on the a11y bus.
#[derive(Debug, Default)]
pub(crate) struct ApplicationDirectory {
    known: HashMap<String, ApplicationInfo>,
}

pub(crate) fn root_of(bus_name: &str) -> Option<ObjectRefOwned> {
    let name = zbus::names::UniqueName::try_from(bus_name.to_owned()).ok()?;
    let path = zbus::zvariant::ObjectPath::from_static_str_unchecked(ROOT_PATH);
    Some(atspi::ObjectRef::new_owned(name, path))
}

/// The pid and toolkit of one application; `None` for the toolkit when it did not answer, so the
/// next read asks again.
async fn identify(
    bus: &AccessibilityBusConnection,
    health: &RefCell<ObjectHealth>,
    bus_name: &str,
    deadline: Instant,
) -> (i32, Option<Toolkit>) {
    let Some(root) = root_of(bus_name) else {
        return (0, None);
    };
    let reader = bus.reader(Toolkit::Unknown, health, deadline);
    let (toolkit, pid) = futures_util::join!(reader.toolkit_of(&root), bus.process_of(bus_name));
    let toolkit = toolkit
        .ok()
        .map(|(name, version)| Toolkit::identify(&name, &version));
    (pid.unwrap_or(0), toolkit)
}

impl ApplicationDirectory {
    pub(crate) async fn describe(
        &mut self,
        bus: &AccessibilityBusConnection,
        health: &RefCell<ObjectHealth>,
        bus_name: &str,
    ) -> ApplicationInfo {
        if let Some(info) = self.known.get(bus_name) {
            return *info;
        }
        let deadline = Instant::now() + DESCRIBE_BUDGET;
        let (pid, toolkit) = identify(bus, health, bus_name, deadline).await;
        self.remember(bus_name, pid, toolkit)
    }

    fn remember(&mut self, bus_name: &str, pid: i32, toolkit: Option<Toolkit>) -> ApplicationInfo {
        let info = ApplicationInfo {
            pid,
            toolkit: toolkit.unwrap_or(Toolkit::Unknown),
        };
        if toolkit.is_some() {
            if self.known.len() >= CAPACITY {
                self.known.clear();
            }
            self.known.insert(bus_name.to_owned(), info);
        }
        info
    }

    /// The application on the bus that drew `window`: its own pid first, else the same app as
    /// the cgroup names it. Unknown applications are described in parallel.
    pub(crate) async fn find_for(
        &mut self,
        bus: &AccessibilityBusConnection,
        health: &RefCell<ObjectHealth>,
        resolver: &mut AppResolver,
        window: &WindowRecord,
    ) -> Option<String> {
        let deadline = Instant::now() + DESCRIBE_BUDGET;
        let applications = registered_applications(bus, health, deadline).await;
        let unknown: Vec<&String> = applications
            .iter()
            .filter(|bus_name| !self.known.contains_key(bus_name.as_str()))
            .collect();
        let identified = gather_until(
            unknown.into_iter().map(|bus_name| async move {
                (bus_name, identify(bus, health, bus_name, deadline).await)
            }),
            deadline,
        )
        .await;
        for (bus_name, (pid, toolkit)) in identified {
            self.remember(bus_name, pid, toolkit);
        }
        let pids: Vec<(&String, i32)> = applications
            .iter()
            .filter_map(|bus_name| Some((bus_name, self.known.get(bus_name)?.pid)))
            .collect();
        if let Some((bus_name, _)) = pids
            .iter()
            .find(|(_, pid)| window.pid > 0 && *pid == window.pid)
        {
            return Some((*bus_name).clone());
        }
        let app_key = resolver.resolve(&window.app_id, window.pid).app_key;
        pids.into_iter()
            .find(|(_, pid)| *pid > 0 && resolver.resolve("", *pid).app_key == app_key)
            .map(|(bus_name, _)| bus_name.clone())
    }

    pub(crate) fn forget(&mut self, bus_name: &str) {
        self.known.remove(bus_name);
    }
}

async fn registered_applications(
    bus: &AccessibilityBusConnection,
    health: &RefCell<ObjectHealth>,
    deadline: Instant,
) -> Vec<String> {
    let Some(root) = bus.registry_name().await.as_deref().and_then(root_of) else {
        return Vec::new();
    };
    bus.reader(Toolkit::Unknown, health, deadline)
        .children(&root)
        .await
        .unwrap_or_default()
        .into_iter()
        .filter_map(|application| application.name_as_str().map(str::to_owned))
        .collect()
}
