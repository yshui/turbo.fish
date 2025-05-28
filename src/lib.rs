#![feature(round_char_boundary, never_type, try_blocks)]
pub mod sources;

pub enum Message {
    /// Cancel all ongoing tasks, update cwd, and recollect information.
    Refresh,
    /// A source has new information
    Update(String, Box<dyn dyn_serde::Serialize>),
}

