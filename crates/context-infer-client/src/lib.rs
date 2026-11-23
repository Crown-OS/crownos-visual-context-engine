//! [`ContextTasks`](context_infer::ContextTasks) for crown-contextd: each call goes to
//! crown-context-inferd over one shared crownos-ipc connection. Frames travel as a [`SealedFrame`]: a sealed
//! memfd that inferd maps read-only. `Busy` comes back as
//! [`InferError::Busy`](context_infer::InferError::Busy): retry after the time it gives, or
//! skip the job.

mod client;
mod connection;
mod convert;
mod frame;

pub use client::InferdClient;
pub use frame::SealedFrame;
