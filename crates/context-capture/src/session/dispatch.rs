use wayland_client::globals::GlobalListContents;
use wayland_client::protocol::wl_buffer::{self, WlBuffer};
use wayland_client::protocol::wl_registry::{self, WlRegistry};
use wayland_client::protocol::wl_shm::{self, WlShm};
use wayland_client::protocol::wl_shm_pool::{self, WlShmPool};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum, event_created_child};
use wayland_protocols::ext::foreign_toplevel_list::v1::client::ext_foreign_toplevel_handle_v1::{
    self, ExtForeignToplevelHandleV1,
};
use wayland_protocols::ext::foreign_toplevel_list::v1::client::ext_foreign_toplevel_list_v1::{
    self, ExtForeignToplevelListV1,
};
use wayland_protocols::ext::image_capture_source::v1::client::ext_foreign_toplevel_image_capture_source_manager_v1::{
    self, ExtForeignToplevelImageCaptureSourceManagerV1,
};
use wayland_protocols::ext::image_capture_source::v1::client::ext_image_capture_source_v1::{
    self, ExtImageCaptureSourceV1,
};
use wayland_protocols::ext::image_copy_capture::v1::client::ext_image_copy_capture_frame_v1::{
    self, ExtImageCopyCaptureFrameV1,
};
use wayland_protocols::ext::image_copy_capture::v1::client::ext_image_copy_capture_manager_v1::{
    self, ExtImageCopyCaptureManagerV1,
};
use wayland_protocols::ext::image_copy_capture::v1::client::ext_image_copy_capture_session_v1::{
    self, ExtImageCopyCaptureSessionV1,
};

use super::state::{FrameEnd, SessionState, ToplevelData};
use crate::geometry::Rect;

macro_rules! ignore_events {
    ($($proxy:ty => $event:ty),* $(,)?) => {$(
        impl Dispatch<$proxy, ()> for SessionState {
            fn event(
                _: &mut Self,
                _: &$proxy,
                _: $event,
                _: &(),
                _: &Connection,
                _: &QueueHandle<Self>,
            ) {
            }
        }
    )*};
}

ignore_events!(
    WlShm => wl_shm::Event,
    WlShmPool => wl_shm_pool::Event,
    WlBuffer => wl_buffer::Event,
    ExtForeignToplevelImageCaptureSourceManagerV1 => ext_foreign_toplevel_image_capture_source_manager_v1::Event,
    ExtImageCaptureSourceV1 => ext_image_capture_source_v1::Event,
    ExtImageCopyCaptureManagerV1 => ext_image_copy_capture_manager_v1::Event,
);

impl Dispatch<WlRegistry, GlobalListContents> for SessionState {
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

impl Dispatch<ExtForeignToplevelListV1, ()> for SessionState {
    fn event(
        _: &mut Self,
        _: &ExtForeignToplevelListV1,
        _: ext_foreign_toplevel_list_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }

    event_created_child!(SessionState, ExtForeignToplevelListV1, [
        ext_foreign_toplevel_list_v1::EVT_TOPLEVEL_OPCODE => (ExtForeignToplevelHandleV1, ToplevelData::default()),
    ]);
}

impl Dispatch<ExtForeignToplevelHandleV1, ToplevelData> for SessionState {
    fn event(
        state: &mut Self,
        handle: &ExtForeignToplevelHandleV1,
        event: ext_foreign_toplevel_handle_v1::Event,
        data: &ToplevelData,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use ext_foreign_toplevel_handle_v1::Event;
        match event {
            Event::Identifier { identifier } => {
                if let Ok(mut known) = data.identifier.lock() {
                    *known = Some(identifier.clone());
                }
                state.toplevels.insert(identifier, handle.clone());
            }
            Event::Closed => {
                let identifier = data.identifier.lock().ok().and_then(|known| known.clone());
                match identifier {
                    Some(identifier) => state.toplevel_closed(&identifier),
                    None => handle.destroy(),
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<ExtImageCopyCaptureSessionV1, ()> for SessionState {
    fn event(
        state: &mut Self,
        session: &ExtImageCopyCaptureSessionV1,
        event: ext_image_copy_capture_session_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use ext_image_copy_capture_session_v1::Event;
        let Some(active) = state
            .active
            .as_mut()
            .filter(|active| active.session.id() == session.id())
        else {
            return;
        };
        let pending = &mut active.pending_constraints;
        match event {
            Event::BufferSize { width, height } => {
                pending.width = width;
                pending.height = height;
                pending.shm_format = None;
            }
            Event::ShmFormat {
                format: WEnum::Value(format),
            } => {
                let preferred = pending.shm_format == Some(wl_shm::Format::Xrgb8888);
                if !preferred
                    && matches!(format, wl_shm::Format::Xrgb8888 | wl_shm::Format::Argb8888)
                {
                    pending.shm_format = Some(format);
                }
            }
            Event::Done => active.constraints = Some(*pending),
            Event::Stopped => active.stopped = true,
            _ => {}
        }
    }
}

impl Dispatch<ExtImageCopyCaptureFrameV1, ()> for SessionState {
    fn event(
        state: &mut Self,
        frame: &ExtImageCopyCaptureFrameV1,
        event: ext_image_copy_capture_frame_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use ext_image_copy_capture_frame_v1::Event;
        let Some(in_flight) = state
            .active
            .as_mut()
            .and_then(|active| active.in_flight.as_mut())
            .filter(|in_flight| in_flight.frame.id() == frame.id())
        else {
            return;
        };
        match event {
            Event::Damage {
                x,
                y,
                width,
                height,
            } => {
                let bounds = in_flight.bounds();
                in_flight
                    .damage
                    .extend(Rect::from_signed_within(x, y, width, height, bounds));
            }
            Event::Ready => in_flight.end = Some(FrameEnd::Ready),
            Event::Failed { reason } => {
                in_flight.end = Some(FrameEnd::Failed(match reason {
                    WEnum::Value(reason) => Some(reason),
                    WEnum::Unknown(_) => None,
                }));
            }
            _ => {}
        }
    }
}
