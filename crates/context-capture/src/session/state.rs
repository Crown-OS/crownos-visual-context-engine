use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use context_core::WindowIdent;
use wayland_client::globals::{BindError, GlobalList};
use wayland_client::protocol::wl_buffer::WlBuffer;
use wayland_client::protocol::wl_shm::{self, WlShm};
use wayland_client::protocol::wl_shm_pool::WlShmPool;
use wayland_client::QueueHandle;
use wayland_protocols::ext::foreign_toplevel_list::v1::client::ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1;
use wayland_protocols::ext::foreign_toplevel_list::v1::client::ext_foreign_toplevel_list_v1::ExtForeignToplevelListV1;
use wayland_protocols::ext::image_capture_source::v1::client::ext_foreign_toplevel_image_capture_source_manager_v1::ExtForeignToplevelImageCaptureSourceManagerV1;
use wayland_protocols::ext::image_capture_source::v1::client::ext_image_capture_source_v1::ExtImageCaptureSourceV1;
use wayland_protocols::ext::image_copy_capture::v1::client::ext_image_copy_capture_frame_v1::{
    ExtImageCopyCaptureFrameV1, FailureReason,
};
use wayland_protocols::ext::image_copy_capture::v1::client::ext_image_copy_capture_manager_v1::{
    ExtImageCopyCaptureManagerV1, Options,
};
use wayland_protocols::ext::image_copy_capture::v1::client::ext_image_copy_capture_session_v1::ExtImageCopyCaptureSessionV1;

use crate::frame::{CaptureTarget, CapturedFrame};
use crate::frame_inbox::FrameInbox;
use crate::frame_queue::MAX_QUEUED_BYTES;
use crate::geometry::Rect;
use crate::metrics::CaptureMetrics;
use crate::util::frame_memory::FrameMemory;

/// A frame of an unchanged window is held by the compositor until the window changes. After
/// this, the frame is given up and its memory zeroed.
const FRAME_WAIT: Duration = Duration::from_secs(3);

/// The identifier a foreign toplevel handle announced.
#[derive(Debug, Default)]
pub(super) struct ToplevelData {
    pub(super) identifier: Mutex<Option<String>>,
}

/// The buffer the compositor wants for the window, as of its last `done`.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct Constraints {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) shm_format: Option<wl_shm::Format>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FrameEnd {
    Ready,
    Failed(Option<FailureReason>),
}

#[derive(Debug)]
pub(super) struct InFlightFrame {
    target: CaptureTarget,
    pub(super) frame: ExtImageCopyCaptureFrameV1,
    pool: WlShmPool,
    buffer: WlBuffer,
    memory: FrameMemory,
    width: u32,
    height: u32,
    stride: usize,
    pub(super) damage: Vec<Rect>,
    deadline: Instant,
    pub(super) end: Option<FrameEnd>,
}

impl InFlightFrame {
    fn release(&self) {
        self.frame.destroy();
        self.buffer.destroy();
        self.pool.destroy();
    }

    pub(super) const fn bounds(&self) -> Rect {
        Rect::new(0, 0, self.width, self.height)
    }
}

#[derive(Debug)]
pub(super) struct ActiveSession {
    pub(super) window: WindowIdent,
    source: ExtImageCaptureSourceV1,
    pub(super) session: ExtImageCopyCaptureSessionV1,
    pub(super) pending_constraints: Constraints,
    pub(super) constraints: Option<Constraints>,
    pub(super) stopped: bool,
    wanted: Option<CaptureTarget>,
    pub(super) in_flight: Option<InFlightFrame>,
}

#[derive(Debug)]
pub(super) struct SessionState {
    shm: WlShm,
    _toplevel_list: ExtForeignToplevelListV1,
    source_manager: ExtForeignToplevelImageCaptureSourceManagerV1,
    copy_manager: ExtImageCopyCaptureManagerV1,
    pub(super) toplevels: HashMap<String, ExtForeignToplevelHandleV1>,
    pub(super) active: Option<ActiveSession>,
    inbox: Arc<FrameInbox>,
    metrics: Arc<CaptureMetrics>,
}

impl SessionState {
    pub(super) fn bind(
        globals: &GlobalList,
        queue: &QueueHandle<Self>,
        inbox: Arc<FrameInbox>,
        metrics: Arc<CaptureMetrics>,
    ) -> Result<Self, BindError> {
        Ok(Self {
            shm: globals.bind(queue, 1..=1, ())?,
            _toplevel_list: globals.bind(queue, 1..=1, ())?,
            source_manager: globals.bind(queue, 1..=1, ())?,
            copy_manager: globals.bind(queue, 1..=1, ())?,
            toplevels: HashMap::new(),
            active: None,
            inbox,
            metrics,
        })
    }

    /// Captures `target` when its window's buffer constraints are known. Only one window has a
    /// session: a request for another window ends the current one.
    pub(super) fn request(&mut self, target: CaptureTarget, queue: &QueueHandle<Self>) {
        if self
            .active
            .as_ref()
            .is_some_and(|active| active.window != target.window)
        {
            self.end_session();
        }
        if self.active.is_none() {
            let Some(handle) = self.toplevels.get(&target.window.0) else {
                tracing::debug!(window = %target.window.0, "no toplevel handle for the window");
                CaptureMetrics::count(&self.metrics.captures_failed);
                return;
            };
            let source = self.source_manager.create_source(handle, queue, ());
            let session = self
                .copy_manager
                .create_session(&source, Options::empty(), queue, ());
            self.active = Some(ActiveSession {
                window: target.window.clone(),
                source,
                session,
                pending_constraints: Constraints::default(),
                constraints: None,
                stopped: false,
                wanted: None,
                in_flight: None,
            });
        }
        if let Some(active) = &mut self.active {
            active.wanted = Some(target);
        }
    }

    /// Finishes ended frames, gives up frames that waited too long, and starts a wanted one.
    pub(super) fn progress(&mut self, queue: &QueueHandle<Self>, now: Instant) {
        let Some(active) = &mut self.active else {
            return;
        };
        if active.stopped {
            CaptureMetrics::count(&self.metrics.captures_denied);
            self.end_session();
            return;
        }
        if let Some(in_flight) = active
            .in_flight
            .take_if(|in_flight| in_flight.end.is_some() || now >= in_flight.deadline)
        {
            in_flight.release();
            match in_flight.end {
                Some(FrameEnd::Ready) => self.deliver(in_flight),
                Some(FrameEnd::Failed(Some(FailureReason::BufferConstraints))) => {
                    active.wanted.get_or_insert(in_flight.target);
                }
                Some(FrameEnd::Failed(Some(FailureReason::Stopped))) => {
                    CaptureMetrics::count(&self.metrics.captures_denied);
                }
                Some(FrameEnd::Failed(_)) => CaptureMetrics::count(&self.metrics.captures_failed),
                None => CaptureMetrics::count(&self.metrics.captures_unchanged),
            }
        }
        let Some(active) = &mut self.active else {
            return;
        };
        if active.in_flight.is_none()
            && let Some(constraints) = active.constraints
            && let Some(target) = active.wanted.take()
        {
            match start_frame(&self.shm, active, constraints, target, queue, now) {
                Ok(in_flight) => active.in_flight = Some(in_flight),
                Err(error) => {
                    tracing::warn!(%error, "cannot start a capture");
                    CaptureMetrics::count(&self.metrics.captures_failed);
                }
            }
        }
    }

    pub(super) fn next_deadline(&self) -> Option<Instant> {
        self.active
            .as_ref()
            .and_then(|active| active.in_flight.as_ref())
            .map(|in_flight| in_flight.deadline)
    }

    /// Gives up the frame in flight and any wanted one; its memory is zeroed.
    pub(super) fn abort(&mut self) {
        if let Some(active) = &mut self.active {
            active.wanted = None;
            if let Some(in_flight) = active.in_flight.take() {
                in_flight.release();
            }
        }
    }

    pub(super) fn end_session(&mut self) {
        self.abort();
        if let Some(active) = self.active.take() {
            active.session.destroy();
            active.source.destroy();
        }
    }

    pub(super) fn toplevel_closed(&mut self, identifier: &str) {
        if let Some(handle) = self.toplevels.remove(identifier) {
            handle.destroy();
        }
        if self
            .active
            .as_ref()
            .is_some_and(|active| active.window.0 == identifier)
        {
            self.end_session();
        }
    }

    fn deliver(&self, mut in_flight: InFlightFrame) {
        if let Err(error) = in_flight.memory.seal() {
            tracing::warn!(%error, "cannot seal a captured frame; it is dropped");
            CaptureMetrics::count(&self.metrics.captures_failed);
            return;
        }
        let bounds = in_flight.bounds();
        let damage = if in_flight.damage.is_empty() {
            vec![bounds]
        } else {
            in_flight.damage
        };
        match CapturedFrame::new(
            in_flight.target,
            in_flight.width,
            in_flight.height,
            in_flight.stride,
            damage,
            in_flight.memory,
        ) {
            Ok(frame) => {
                CaptureMetrics::count(&self.metrics.frames_captured);
                self.inbox.push(frame);
            }
            Err(error) => {
                tracing::warn!(%error, "the compositor sent a frame that does not fit");
                CaptureMetrics::count(&self.metrics.captures_failed);
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
enum StartError {
    #[error("the compositor offers no XRGB8888 or ARGB8888 shm buffer")]
    NoShmFormat,
    #[error("a {width}x{height} frame is too large")]
    TooLarge { width: u32, height: u32 },
    #[error(transparent)]
    Memory(#[from] crate::util::frame_memory::FrameMemoryError),
}

fn start_frame(
    shm: &WlShm,
    active: &ActiveSession,
    constraints: Constraints,
    target: CaptureTarget,
    queue: &QueueHandle<SessionState>,
    now: Instant,
) -> Result<InFlightFrame, StartError> {
    let format = constraints.shm_format.ok_or(StartError::NoShmFormat)?;
    let Constraints { width, height, .. } = constraints;
    let too_large = || StartError::TooLarge { width, height };
    let stride = (width as usize)
        .checked_mul(CapturedFrame::BYTES_PER_PIXEL)
        .ok_or_else(too_large)?;
    let length = stride
        .checked_mul(height as usize)
        .filter(|length| *length <= MAX_QUEUED_BYTES)
        .ok_or_else(too_large)?;
    let wire_length = i32::try_from(length).map_err(|_| too_large())?;
    let wire_width = i32::try_from(width).map_err(|_| too_large())?;
    let wire_height = i32::try_from(height).map_err(|_| too_large())?;
    let wire_stride = i32::try_from(stride).map_err(|_| too_large())?;
    let memory = FrameMemory::allocate(length)?;
    let pool = shm.create_pool(memory.memfd(), wire_length, queue, ());
    let buffer = pool.create_buffer(0, wire_width, wire_height, wire_stride, format, queue, ());
    let frame = active.session.create_frame(queue, ());
    frame.attach_buffer(&buffer);
    frame.damage_buffer(0, 0, wire_width, wire_height);
    frame.capture();
    Ok(InFlightFrame {
        target,
        frame,
        pool,
        buffer,
        memory,
        width,
        height,
        stride,
        damage: Vec::new(),
        deadline: now + FRAME_WAIT,
        end: None,
    })
}
