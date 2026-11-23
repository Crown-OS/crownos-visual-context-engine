use std::sync::Mutex;

use context_core::{Observation, ObservationKind, Timestamp, WindowIdent, WindowRecord};
use crownos_protocols::context::v1::client::crownos_context_manager_v1::CrownosContextManagerV1;
use crownos_protocols::context::v1::client::crownos_context_window_v1::CrownosContextWindowV1;
use wayland_client::globals::GlobalList;
use wayland_client::{Proxy, QueueHandle};

use super::{BridgeError, BridgeLink, Outlet};
use crate::window_table::WindowTable;

/// What one `crownos_context_window_v1` has said so far, until its `done`.
#[derive(Debug, Default)]
pub(super) struct WindowData {
    pub(super) pending: Mutex<WindowRecord>,
}

impl WindowData {
    pub(super) fn update(&self, change: impl FnOnce(&mut WindowRecord)) {
        if let Ok(mut record) = self.pending.lock() {
            change(&mut record);
        }
    }

    pub(super) fn snapshot(&self) -> WindowRecord {
        self.pending
            .lock()
            .map(|record| record.clone())
            .unwrap_or_default()
    }
}

pub(super) struct BridgeState {
    pub(super) manager: CrownosContextManagerV1,
    pub(super) outlet: Outlet,
    pub(super) windows: WindowTable,
    link: BridgeLink,
    pub(super) initial_done: bool,
    closed: bool,
}

impl std::fmt::Debug for BridgeState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BridgeState")
            .field("windows", &self.windows.len())
            .field("initial_done", &self.initial_done)
            .finish_non_exhaustive()
    }
}

impl BridgeState {
    pub(super) fn bind(
        globals: &GlobalList,
        handle: &QueueHandle<Self>,
        outlet: Outlet,
        link: BridgeLink,
    ) -> Result<Self, BridgeError> {
        let manager: CrownosContextManagerV1 = globals.bind(handle, 1..=1, ())?;
        Ok(Self {
            manager,
            outlet,
            windows: WindowTable::default(),
            link,
            initial_done: false,
            closed: false,
        })
    }

    pub(super) fn emit(&mut self, kind: ObservationKind) {
        if self
            .outlet
            .send(Observation::new(Timestamp::now(), kind))
            .is_err()
        {
            self.closed = true;
        }
    }

    pub(super) const fn outlet_closed(&self) -> bool {
        self.closed
    }

    pub(super) fn ident_of(window: &CrownosContextWindowV1) -> Option<WindowIdent> {
        window
            .data::<WindowData>()
            .map(|data| data.snapshot().ident)
            .filter(|ident| !ident.0.is_empty())
    }

    /// A window's description is consistent: record it and tell the engine.
    pub(super) fn window_done(&mut self, window: &CrownosContextWindowV1) {
        let Some(record) = window.data::<WindowData>().map(WindowData::snapshot) else {
            return;
        };
        if record.ident.0.is_empty() {
            return;
        }
        let known = self.windows.get(&record.ident).is_some();
        if !known {
            self.link
                .window_described(record.ident.clone(), window.clone());
        }
        if !self.windows.upsert(record.clone()) {
            return;
        }
        self.emit(if known {
            ObservationKind::WindowChanged(record)
        } else {
            ObservationKind::WindowOpened(record)
        });
    }

    pub(super) fn window_closed(&mut self, window: &CrownosContextWindowV1) {
        if let Some(ident) = Self::ident_of(window) {
            self.link.window_closed(&ident);
            self.windows.remove(&ident);
            self.emit(ObservationKind::WindowClosed(ident));
        }
        window.destroy();
    }
}
