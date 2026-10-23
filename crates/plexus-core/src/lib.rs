//! Plexus core: a local hub that aggregates many MCP servers behind
//! per-profile MCP endpoints. Has no desktop/UI dependencies.

pub mod clients;
pub mod http;
pub mod hub;
pub mod model;
pub mod naming;
pub mod oauth;
pub mod search;
pub mod secrets;
pub mod server;
pub mod store;

pub use hub::{ApprovalBroker, ApprovalDecision, ApprovalRequest, Hub, HubEvent, HubOptions};
