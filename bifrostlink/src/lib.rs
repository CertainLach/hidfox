#![feature(try_blocks)]

mod port;
use std::{fmt, hash::Hash};

pub use port::{native_messaging_port, Port};
mod util;
use serde::{Serialize, de::DeserializeOwned};
mod connection;
mod qos;
mod route;
pub use route::Rtt;

mod event;
mod packet;

mod notification;
pub use notification::{IncomingNotification, Notification, OutgoingNotification};
mod request;
pub use request::{IncomingRequest, OutgoingRequest, Request};

mod internal_handlers;

pub(crate) mod callback;

pub(crate) mod polling;
pub use polling::request::PollingRequest;

mod rpc;
pub use rpc::{Rpc, WeakRpc};

pub mod error;

pub use polling::notification::PollingNotification;

pub trait AddressT:
	Clone + Serialize + DeserializeOwned + Hash + Eq + fmt::Debug + Send + Sync + 'static
{
}
