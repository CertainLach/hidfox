use std::{collections::hash_map::Entry, pin::Pin, task};

use bytes::Bytes;
use futures::Stream;
use serde::de::DeserializeOwned;
use tokio::{
	select,
	sync::mpsc::{error::SendError, unbounded_channel, UnboundedReceiver as Receiver},
};

use crate::{
	error::ErrorT,
	rpc::{Rpc, RpcInner, WeakRpc},
	AddressT, IncomingNotification, Notification,
};

pub(crate) struct OpaquePollingNotification<Address> {
	pub from: Address,
	pub request: Bytes,
}
impl<Address> OpaquePollingNotification<Address>
where
	Address: AddressT,
{
	pub(crate) fn into_typed<R: IncomingNotification>(
		self,
	) -> Result<PollingNotification<R, Address>, serde_json::Error> {
		let request = match serde_json::from_slice(&self.request) {
			Ok(v) => v,
			Err(e) => return Err(e),
		};
		Ok(PollingNotification {
			from: self.from,
			request,
		})
	}
}
pub struct PollingNotification<R: Notification, Address> {
	from: Address,
	request: R,
}
impl<N: Notification, Address> PollingNotification<N, Address> {
	pub fn from(&self) -> &Address {
		&self.from
	}
	pub fn data(&self) -> &N {
		&self.request
	}
}

struct PollingNotificationStream<Address: AddressT, Error: ErrorT, N: Notification> {
	rpc: WeakRpc<Address, Error>,
	// name: &'static str,
	channel: Receiver<PollingNotification<N, Address>>,
}
impl<Address: AddressT, Error: ErrorT, N: Notification> Stream
	for PollingNotificationStream<Address, Error, N>
{
	type Item = PollingNotification<N, Address>;

	fn poll_next(
		mut self: Pin<&mut Self>,
		cx: &mut task::Context<'_>,
	) -> task::Poll<Option<Self::Item>> {
		self.channel.poll_recv(cx)
	}
}
impl<Address: AddressT, Error: ErrorT, N: Notification> Drop
	for PollingNotificationStream<Address, Error, N>
{
	fn drop(&mut self) {
		if let Some(rpc) = self.rpc.clone().upgrade() {
			rpc.unregister_polling_notification_handler::<N>();
		}
	}
}

impl<Address: AddressT, Error: ErrorT> RpcInner<Address, Error> {
	fn register_polling_notification_handler<R: Notification + DeserializeOwned + 'static>(
		&mut self,
	) -> Receiver<PollingNotification<R, Address>> {
		let (otx, mut orx) = unbounded_channel();
		match self.polling_notification_handler.entry(R::name()) {
			Entry::Occupied(_) => panic!("request handler is already defined"),
			Entry::Vacant(v) => v.insert(otx),
		};
		// FIXME: have bounded channel, to prevent double buffering
		let (tx, rx) = unbounded_channel();
		tokio::task::spawn(async move {
			loop {
				select! {
					req = orx.recv() => {
						let Some(req) = req else {
							break;
						};
						let r = req.request.clone();
						let request: PollingNotification<R, Address> = match req.into_typed() {
							Ok(r) => r,
							Err(e) => {
								eprintln!("failed to decode notification: {e}\n{:?}", String::from_utf8_lossy(&r.to_vec()));
								continue;
							}
						};
						if let Err(SendError(_r)) = tx.send(request) {
							eprintln!("notification handler dead inflight");
							break;
						};
						continue;
					}
					() = tx.closed() => {
						break;
					}
				}
			}
		});
		rx
	}
}
impl<Address, Error> Rpc<Address, Error>
where
	Address: AddressT,
	Error: ErrorT,
{
	pub fn unregister_polling_notification_handler<N: Notification + Send + 'static>(&self) {
		let mut inner = self.inner.write().expect("write");
		inner.polling_notification_handler.remove(N::name());
	}
	pub fn register_polling_notification_handler<R: Notification + DeserializeOwned + 'static>(
		&self,
	) -> Receiver<PollingNotification<R, Address>> {
		let mut inner = self.inner.write().expect("write");
		inner.register_polling_notification_handler()
	}
}
