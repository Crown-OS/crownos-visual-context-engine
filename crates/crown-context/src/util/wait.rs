use std::os::fd::AsFd;

use anyhow::Result;
use context_protocol::context::Client;

/// Blocks until the connection has something to read.
pub(crate) fn readable(client: &Client) -> Result<()> {
    let fd = client.as_fd();
    let mut fds = [rustix::event::PollFd::new(
        &fd,
        rustix::event::PollFlags::IN,
    )];
    match rustix::event::poll(&mut fds, None) {
        Ok(_) | Err(rustix::io::Errno::INTR) => Ok(()),
        Err(error) => Err(error.into()),
    }
}
