//! Where a notification goes, as a port.
//!
//! One channel per deployment, and it comes from configuration. A rule says
//! *what is worth telling somebody about*; an operator says *who*. The split
//! is not tidiness: an endpoint that arrived in a request body would let
//! anything able to publish a rule aim this process's outbound POSTs at a link
//! local address, which is the same reasoning that keeps a rerun's endpoint in
//! a variable rather than on the log.
//!
//! The adapter lives where the other outbound clients do — beside the reactor,
//! in the wiring crate — and what is here is the two things a caller needs: a
//! way to send one payload, and a way to say what the channel is without
//! saying where it is.

use async_trait::async_trait;

use aiwatcher_core::ports::PortResult;

use crate::delivery::AlertPayload;

/// What a channel is, for an inventory and for a history row.
///
/// Never the address. A webhook URL is not a credential and is still
/// reconnaissance for somebody already inside, which is the line
/// `GET /api/v1/system` draws for every other endpoint this deployment holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, utoipa::ToSchema)]
#[schema(as = AlertChannelDescription)]
pub struct ChannelDescription {
    /// `webhook` — the only kind there is.
    pub kind: &'static str,
    /// The variable that configured it, which is what a reader needs anyway.
    pub variable: &'static str,
    /// Whether what this sends carries a signature the receiver can check.
    pub signed: bool,
}

/// Send one notification.
#[async_trait]
pub trait AlertChannel: Send + Sync + std::fmt::Debug {
    /// Deliver a payload. `Ok(())` means the receiver accepted it.
    ///
    /// # Errors
    ///
    /// [`PortError`](aiwatcher_core::ports::PortError), with its retryability
    /// set from what went wrong: `Unavailable` for a transport failure or a
    /// receiver that is having a moment, `Rejected` for a receiver that
    /// understood and said no. The queue believes it.
    async fn deliver(&self, payload: &AlertPayload) -> PortResult<()>;

    fn describe(&self) -> ChannelDescription;
}
