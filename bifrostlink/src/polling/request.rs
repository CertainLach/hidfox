use std::{collections::hash_map::Entry, fmt::Display, future::Future, pin::Pin, task};

use bytes::Bytes;
use futures::Stream;
use serde::Serialize;
use tokio::{
	select,
	sync::{
		mpsc::{error::SendError, unbounded_channel, UnboundedReceiver as Receiver},
		oneshot,
	},
};

use crate::{
	error::ErrorT,
	packet::OutgoingMessage,
	rpc::{Rpc, WeakRpc},
	AddressT, IncomingRequest, Request,
};

#[must_use]
pub(crate) struct OpaquePollingRequest<Address: AddressT> {
	pub from: Address,
	pub id: String,
	pub request: Option<Bytes>,
	pub respond: Option<oneshot::Sender<OutgoingMessage<Address>>>,
}
impl<Address: AddressT> OpaquePollingRequest<Address> {
	fn respond_raw(&mut self, out: OutgoingMessage<Address>) {
		match self.respond.take().expect("didn't responded yet").send(out) {
			Ok(()) => {}
			Err(_) => {
				eprintln!("failed to respond")
			}
		}
	}
	fn responded(&self) -> bool {
		self.respond.is_none()
	}
}
impl<Address: AddressT> OpaquePollingRequest<Address> {
	pub(crate) fn respond_ok<R: Serialize>(mut self, response: R) {
		self.respond_raw(OutgoingMessage::new_response(
			&self.id,
			self.from.clone(),
			&response,
		))
	}
	pub(crate) fn respond_err<E: Display>(mut self, response: E) {
		self.respond_raw(OutgoingMessage::new_error_response(
			&self.id,
			self.from.clone(),
			response,
		))
	}
	pub(crate) fn respond<R: Serialize, E: Display>(self, result: Result<R, E>) {
		match result {
			Ok(r) => self.respond_ok(r),
			Err(e) => self.respond_err(e),
		}
	}
}
impl<Address: AddressT> OpaquePollingRequest<Address> {
	pub(crate) fn into_typed<R: IncomingRequest>(
		mut self,
	) -> Result<PollingRequest<R, Address>, (serde_json::Error, Self)>
	where
		R::Response: Serialize,
	{
		let raw = self.request.take().expect("not yet converted");
		let request = match serde_json::from_slice(&raw) {
			Ok(v) => v,
			Err(e) => return Err((e, self)),
		};
		Ok(PollingRequest {
			opaque: self,
			request,
		})
	}
}

impl<Address: AddressT> Drop for OpaquePollingRequest<Address> {
	fn drop(&mut self) {
		if self.responded() {
			return;
		}
		self.respond_raw(OutgoingMessage::new_error_response(
			&self.id,
			self.from.clone(),
			"no response was provided",
		));
	}
}

#[must_use]
pub struct PollingRequest<R: IncomingRequest, Address>
where
	R::Response: Serialize,
	Address: AddressT,
{
	opaque: OpaquePollingRequest<Address>,
	request: R,
}

impl<R: IncomingRequest, Address> PollingRequest<R, Address>
where
	Address: AddressT,
	R::Response: Serialize,
{
	pub fn data(&self) -> &R {
		&self.request
	}
	pub fn respond_ok(self, response: R::Response) {
		self.opaque.respond_ok(response)
	}
	pub fn respond_err(self, response: &str) {
		self.opaque.respond_err(response)
	}
	pub fn respond(self, result: Result<R::Response, &str>) {
		match result {
			Ok(r) => self.respond_ok(r),
			Err(e) => self.respond_err(e),
		}
	}
	pub async fn handle<E: Display, F: Future<Output = Result<R::Response, E>>>(
		self,
		handler: impl FnOnce(Address, R) -> F,
	) {
		let future = handler(self.opaque.from.clone(), self.request);
		let result = future.await;
		self.opaque.respond(result);
	}
}

pub struct PollingRequestStream<Address, Error, R: IncomingRequest>
where
	R::Response: Serialize,
	Address: AddressT,
	Error: ErrorT,
{
	rpc: WeakRpc<Address, Error>,
	channel: Receiver<PollingRequest<R, Address>>,
}
impl<Address, Error, R: IncomingRequest> Stream for PollingRequestStream<Address, Error, R>
where
	R::Response: Serialize,
	Address: AddressT,
	Error: ErrorT,
{
	type Item = PollingRequest<R, Address>;

	fn poll_next(
		mut self: Pin<&mut Self>,
		cx: &mut task::Context<'_>,
	) -> task::Poll<Option<Self::Item>> {
		self.channel.poll_recv(cx)
	}
}
impl<Address, Error, R: IncomingRequest> Drop for PollingRequestStream<Address, Error, R>
where
	R::Response: Serialize,
	Address: AddressT,
	Error: ErrorT,
{
	fn drop(&mut self) {
		if let Some(rpc) = self.rpc.clone().upgrade() {
			rpc.unregister_polling_request_handler::<R>();
		}
	}
}

impl<Address, Error> Rpc<Address, Error>
where
	Address: AddressT,
	Error: ErrorT,
{
	pub fn register_polling_request_handler<R: IncomingRequest + Send + 'static>(
		&mut self,
	) -> Option<PollingRequestStream<Address, Error, R>>
	where
		R::Response: Serialize,
	{
		let mut inner = self.inner.write().expect("write");

		let (otx, mut orx) = unbounded_channel();
		match inner.polling_request_handler.entry(R::name()) {
			Entry::Occupied(_) => return None,
			Entry::Vacant(v) => v.insert(otx),
		};
		let (tx, rx) = unbounded_channel();
		tokio::task::spawn(async move {
			loop {
				select! {
					req = orx.recv() => {
						let Some(req) = req else {
							break;
						};
						let request: PollingRequest<R, Address> = match req.into_typed() {
							Ok(r) => r,
							Err((e, req)) => {
								req.respond_err(format!("failed to decode request: {e}"));
								continue;
							}
						};
						if let Err(SendError(r)) = tx.send(request) {
							r.respond_err("request handler is dead inflight");
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
		Some(PollingRequestStream {
			rpc: self.clone().downgrade(),
			channel: rx,
		})
	}
	pub fn unregister_polling_request_handler<R: Request + 'static>(&self) {
		let mut inner = self.inner.write().expect("write");
		inner.polling_request_handler.remove(R::name());
	}
}
