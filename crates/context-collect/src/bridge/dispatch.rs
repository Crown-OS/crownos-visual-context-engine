use context_core::{
    ContentType, InputCounts, Micros, ObservationKind, PauseOrigin, Placement, Sandbox,
    SelectionKind, Sensitivity, SessionState, WindowIdent, WindowState,
};
use crownos_protocols::context::v1::client::crownos_context_manager_v1::{
    self, CrownosContextManagerV1,
};
use crownos_protocols::context::v1::client::crownos_context_window_v1::{
    self, CrownosContextWindowV1,
};
use wayland_client::globals::GlobalListContents;
use wayland_client::protocol::wl_registry::{self, WlRegistry};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum, event_created_child};

use super::state::{BridgeState, WindowData};

const SELECTION_SECRET: u32 = 1;
const TYPING_PAUSED: u32 = 1;
const SCROLL_STOPPED: u32 = 2;

impl Dispatch<WlRegistry, GlobalListContents> for BridgeState {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<CrownosContextManagerV1, ()> for BridgeState {
    fn event(
        state: &mut Self,
        _: &CrownosContextManagerV1,
        event: crownos_context_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use crownos_context_manager_v1::Event;
        match event {
            Event::Window { window, identifier } => {
                if let Some(data) = window.data::<WindowData>() {
                    data.update(|record| record.ident = WindowIdent(identifier));
                }
            }
            Event::Focus { window, .. } => {
                let ident = window.as_ref().and_then(BridgeState::ident_of);
                state.windows.set_focus(ident.clone());
                state.emit(ObservationKind::Focus(ident));
            }
            Event::Session { state: session } => {
                let session = match session {
                    WEnum::Value(crownos_context_manager_v1::SessionState::Idle) => {
                        SessionState::Idle
                    }
                    WEnum::Value(crownos_context_manager_v1::SessionState::Locked) => {
                        SessionState::Locked
                    }
                    _ => SessionState::Active,
                };
                state.emit(ObservationKind::Session(session));
            }
            Event::InputActivity {
                window,
                interval_ms,
                text,
                edit,
                nav,
                chords,
                clicks,
                scroll_ticks,
                pointer_travel,
                flags,
            } => {
                let flags = match flags {
                    WEnum::Value(flags) => flags.bits(),
                    WEnum::Unknown(bits) => bits,
                };
                state.emit(ObservationKind::InputActivity {
                    window: window.as_ref().and_then(BridgeState::ident_of),
                    interval: Micros::from_millis(i64::from(interval_ms)),
                    counts: InputCounts {
                        text_keys: text,
                        edit_keys: edit,
                        nav_keys: nav,
                        chords,
                        clicks,
                        scroll_ticks,
                        pointer_travel,
                        typing_paused: flags & TYPING_PAUSED != 0,
                        scroll_stopped: flags & SCROLL_STOPPED != 0,
                    },
                });
            }
            Event::Selection {
                kind,
                window,
                mime_types,
                size_hint,
                flags,
            } => {
                let kind = match kind {
                    WEnum::Value(crownos_context_manager_v1::SelectionKind::Primary) => {
                        SelectionKind::Primary
                    }
                    _ => SelectionKind::Clipboard,
                };
                let secret = match flags {
                    WEnum::Value(flags) => flags.bits() & SELECTION_SECRET != 0,
                    WEnum::Unknown(bits) => bits & SELECTION_SECRET != 0,
                };
                state.emit(ObservationKind::Selection {
                    kind,
                    window: window.as_ref().and_then(BridgeState::ident_of),
                    mime_types: mime_types
                        .split(',')
                        .filter(|mime| !mime.is_empty())
                        .map(str::to_owned)
                        .collect(),
                    size_hint,
                    secret,
                });
            }
            Event::Paused { paused, origin } => {
                let origin = match origin {
                    WEnum::Value(crownos_context_manager_v1::PauseOrigin::Engine) => {
                        PauseOrigin::Engine
                    }
                    WEnum::Value(crownos_context_manager_v1::PauseOrigin::Lock) => {
                        PauseOrigin::Lock
                    }
                    _ => PauseOrigin::Hotkey,
                };
                state.emit(ObservationKind::Paused {
                    paused: paused != 0,
                    origin,
                });
            }
            Event::Done => state.initial_done = true,
            _ => {}
        }
    }

    event_created_child!(BridgeState, CrownosContextManagerV1, [
        crownos_context_manager_v1::EVT_WINDOW_OPCODE => (CrownosContextWindowV1, WindowData::default()),
    ]);
}

impl Dispatch<CrownosContextWindowV1, WindowData> for BridgeState {
    fn event(
        state: &mut Self,
        window: &CrownosContextWindowV1,
        event: crownos_context_window_v1::Event,
        data: &WindowData,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use crownos_context_window_v1::Event;
        match event {
            Event::AppId { app_id } => data.update(|record| record.app_id = app_id),
            Event::Title { title } => data.update(|record| record.title = title),
            Event::Process { pid } => data.update(|record| record.pid = pid),
            Event::Sandbox {
                engine,
                app_id,
                instance_id,
            } => data.update(|record| {
                record.sandbox = (!engine.is_empty()).then_some(Sandbox {
                    engine,
                    app_id,
                    instance_id,
                })
            }),
            Event::Placement {
                x,
                y,
                width,
                height,
                output,
                workspace,
            } => data.update(|record| {
                record.placement = Some(Placement {
                    x,
                    y,
                    width,
                    height,
                });
                record.output = output;
                record.workspace = workspace;
            }),
            Event::State { state: bits } => {
                let bits = match bits {
                    WEnum::Value(flags) => flags.bits(),
                    WEnum::Unknown(bits) => bits,
                };
                data.update(|record| record.state = WindowState::from_bits(bits));
            }
            Event::ContentType { content_type } => {
                let value = match content_type {
                    WEnum::Value(kind) => kind as u32,
                    WEnum::Unknown(value) => value,
                };
                data.update(|record| record.content_type = ContentType::from_wire(value));
            }
            Event::Sensitivity { sensitivity } => {
                let bits = match sensitivity {
                    WEnum::Value(flags) => flags.bits(),
                    WEnum::Unknown(bits) => bits,
                };
                data.update(|record| record.sensitivity = Sensitivity::from_bits(bits));
            }
            Event::Done => state.window_done(window),
            Event::Closed => state.window_closed(window),
            _ => {}
        }
    }
}
