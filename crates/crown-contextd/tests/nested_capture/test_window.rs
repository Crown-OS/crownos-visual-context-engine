//! A toplevel that shows seeded noise, so any copy of its pixels can be found later.

use std::fs::File;
use std::io::Write;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

use context_eval::pixel_scan::noise_pixels;
use rustix::fs::MemfdFlags;
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{
    wl_buffer::WlBuffer, wl_compositor::WlCompositor, wl_registry::WlRegistry, wl_shm,
    wl_shm::WlShm, wl_shm_pool::WlShmPool, wl_surface::WlSurface,
};
use wayland_client::{Connection, Dispatch, QueueHandle, delegate_noop};
use wayland_protocols::xdg::shell::client::{
    xdg_surface::{self, XdgSurface},
    xdg_toplevel::XdgToplevel,
    xdg_wm_base::{self, XdgWmBase},
};

pub(crate) const WIDTH: u32 = 240;
pub(crate) const HEIGHT: u32 = 160;

#[derive(Default)]
struct WindowState {
    configured: bool,
}

impl Dispatch<WlRegistry, GlobalListContents> for WindowState {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: wayland_client::protocol::wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<XdgWmBase, ()> for WindowState {
    fn event(
        _: &mut Self,
        wm_base: &XdgWmBase,
        event: xdg_wm_base::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            wm_base.pong(serial);
        }
    }
}

impl Dispatch<XdgSurface, ()> for WindowState {
    fn event(
        state: &mut Self,
        surface: &XdgSurface,
        event: xdg_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            surface.ack_configure(serial);
            state.configured = true;
        }
    }
}

delegate_noop!(WindowState: ignore WlCompositor);
delegate_noop!(WindowState: ignore WlSurface);
delegate_noop!(WindowState: ignore WlShm);
delegate_noop!(WindowState: ignore WlShmPool);
delegate_noop!(WindowState: ignore WlBuffer);
delegate_noop!(WindowState: ignore XdgToplevel);

/// Shows the window until the process is killed. Each round draws `seed_for(round)` and
/// retitles the window, so the engine sees a title change and new damage.
pub(crate) fn show_until_killed(
    socket: &Path,
    round_length: Duration,
    seed_for: fn(u64) -> u64,
) -> Result<(), Box<dyn std::error::Error>> {
    run(UnixStream::connect(socket)?, round_length, seed_for)
}

fn run(
    stream: UnixStream,
    round_length: Duration,
    seed_for: fn(u64) -> u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<WindowState>(&connection)?;
    let handle = queue.handle();
    let compositor: WlCompositor = globals.bind(&handle, 4..=6, ())?;
    let shm: WlShm = globals.bind(&handle, 1..=1, ())?;
    let wm_base: XdgWmBase = globals.bind(&handle, 1..=6, ())?;
    let surface = compositor.create_surface(&handle, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &handle, ());
    let toplevel = xdg_surface.get_toplevel(&handle, ());
    toplevel.set_app_id("org.crownos.VcgeCaptureTest".to_owned());
    toplevel.set_title("Noise 0".to_owned());
    surface.commit();
    let mut state = WindowState::default();
    while !state.configured {
        queue.blocking_dispatch(&mut state)?;
    }
    let mut round = 0_u64;
    let mut round_started = Instant::now();
    draw(&shm, &surface, seed_for(round), &handle)?;
    loop {
        if round_started.elapsed() >= round_length {
            round += 1;
            round_started = Instant::now();
            toplevel.set_title(format!("Noise {round}"));
            draw(&shm, &surface, seed_for(round), &handle)?;
        }
        queue.roundtrip(&mut state)?;
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn draw(
    shm: &WlShm,
    surface: &WlSurface,
    seed: u64,
    handle: &QueueHandle<WindowState>,
) -> Result<(), Box<dyn std::error::Error>> {
    let pixels = noise_pixels(seed, WIDTH, HEIGHT);
    let mut memfd = File::from(rustix::fs::memfd_create(
        "vcge-test-window",
        MemfdFlags::CLOEXEC,
    )?);
    memfd.write_all(&pixels)?;
    let length = i32::try_from(pixels.len())?;
    let pool = shm.create_pool(memfd.as_fd(), length, handle, ());
    let buffer = pool.create_buffer(
        0,
        i32::try_from(WIDTH)?,
        i32::try_from(HEIGHT)?,
        i32::try_from(WIDTH * 4)?,
        wl_shm::Format::Xrgb8888,
        handle,
        (),
    );
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, i32::MAX, i32::MAX);
    surface.commit();
    pool.destroy();
    Ok(())
}
