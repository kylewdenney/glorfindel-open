//! # Glorfindel Transport
//!
//! Transport abstraction layer providing a dual-plane architecture:
//! - **Control Plane (DDS)**: Reliable, topic-based pub/sub for task routing and agent discovery
//! - **Data Plane (ZMQ)**: High-throughput messaging for tool call dispatch and result streaming
//!
//! This separation mirrors OMS (Open Mission Systems) architecture where control
//! and data concerns use different transport mechanisms optimized for their needs.

pub mod dds;
pub mod error;
pub mod traits;
pub mod zmq;

pub use dds::DdsControlPlane;
pub use error::TransportError;
pub use traits::{ControlPlane, DataPlane};
pub use self::zmq::ZmqDataPlane;
