//! Errors surfaced by the drive core.

/// Errors returned when driving a [`crate::DriveSession`].
#[derive(Debug, thiserror::Error)]
pub enum DriveError {
    /// Input was sent to a session that was created non-interactively (no input
    /// source was wired), so it has nowhere to deliver the message.
    #[error("drive session is not interactive; it accepts no input")]
    NotInteractive,
    /// Input was sent to a session whose run has already ended.
    #[error("drive session has ended; it accepts no further input")]
    Ended,
}
