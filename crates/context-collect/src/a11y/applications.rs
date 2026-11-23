use std::cell::RefCell;
use std::collections::HashMap;
use std::time::Duration;

use atspi::ObjectRefOwned;
use context_core::WindowRecord;
use tokio::time::Instant;
use zbus::names::BusName;

use super::health::ObjectHealth;
use super::reader::Reader;
use super::toolkit::Toolkit;
use crate::apps::AppResolver;

const ROOT_PATH: &str = "/org/a11y/atspi/accessible/root";
const REGISTRY: &str = "org.a11y.atspi.Registry";
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

impl ApplicationDirectory {
    pub(crate) async fn describe(
        &mut self,
        connection: &zbus::Connection,
        health: &RefCell<ObjectHealth>,
        bus_name: &str,
    ) -> Option<ApplicationInfo> {
        if let Some(info) = self.known.get(bus_name) {
            return Some(*info);
        }
        let root = root_of(bus_name)?;
        let reader = Reader::new(
            connection,
            Toolkit::Unknown,
            health,
            Instant::now() + DESCRIBE_BUDGET,
        );
        let (toolkit, pid) =
            futures_util::join!(reader.toolkit_of(&root), process_of(connection, bus_name));
        let Ok((name, version)) = toolkit else {
            return Some(ApplicationInfo {
                pid: pid.unwrap_or(0),
                toolkit: Toolkit::Unknown,
            });
        };
        let info = ApplicationInfo {
            pid: pid.unwrap_or(0),
            toolkit: Toolkit::identify(&name, &version),
        };
        if self.known.len() >= CAPACITY {
            self.known.clear();
        }
        self.known.insert(bus_name.to_owned(), info);
        Some(info)
    }

    /// The application on the bus that drew `window`: its own pid first, else the same app as
    /// the cgroup names it.
    pub(crate) async fn find_for(
        &mut self,
        connection: &zbus::Connection,
        health: &RefCell<ObjectHealth>,
        resolver: &mut AppResolver,
        window: &WindowRecord,
    ) -> Option<String> {
        let applications = registered_applications(connection, health).await;
        let mut described = Vec::with_capacity(applications.len());
        for bus_name in applications {
            if let Some(info) = self.describe(connection, health, &bus_name).await {
                if info.pid == window.pid && window.pid > 0 {
                    return Some(bus_name);
                }
                described.push((bus_name, info.pid));
            }
        }
        let app_key = resolver.resolve(&window.app_id, window.pid).app_key;
        described
            .into_iter()
            .find(|(_, pid)| *pid > 0 && resolver.resolve("", *pid).app_key == app_key)
            .map(|(bus_name, _)| bus_name)
    }

    pub(crate) fn forget(&mut self, bus_name: &str) {
        self.known.remove(bus_name);
    }
}

async fn process_of(connection: &zbus::Connection, bus_name: &str) -> Option<i32> {
    let dbus = zbus::fdo::DBusProxy::new(connection).await.ok()?;
    let name = BusName::try_from(bus_name).ok()?;
    let pid = dbus.get_connection_unix_process_id(name).await.ok()?;
    i32::try_from(pid).ok()
}

async fn registered_applications(
    connection: &zbus::Connection,
    health: &RefCell<ObjectHealth>,
) -> Vec<String> {
    let Ok(dbus) = zbus::fdo::DBusProxy::new(connection).await else {
        return Vec::new();
    };
    let Ok(registry) = BusName::try_from(REGISTRY) else {
        return Vec::new();
    };
    let Ok(owner) = dbus.get_name_owner(registry).await else {
        return Vec::new();
    };
    let Some(root) = root_of(owner.as_str()) else {
        return Vec::new();
    };
    let reader = Reader::new(
        connection,
        Toolkit::Unknown,
        health,
        Instant::now() + DESCRIBE_BUDGET,
    );
    reader
        .children(&root)
        .await
        .unwrap_or_default()
        .into_iter()
        .filter_map(|application| application.name_as_str().map(str::to_owned))
        .collect()
}
