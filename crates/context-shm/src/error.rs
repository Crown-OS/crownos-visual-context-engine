use std::io;

#[derive(Debug, thiserror::Error)]
pub enum ShmError {
    #[error("the memfd is not sealed against writes, shrinking and growing")]
    NotSealed,
    #[error("the memfd is empty")]
    Empty,
    #[error("memfd call failed: {0}")]
    Io(#[from] io::Error),
}

impl From<rustix::io::Errno> for ShmError {
    fn from(errno: rustix::io::Errno) -> Self {
        Self::Io(errno.into())
    }
}
