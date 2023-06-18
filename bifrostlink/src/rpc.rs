use std::collections::{HashMap, HashSet};
use std::collections::hash_map::Entry;
use std::hash::Hash;
use std::marker::PhantomData;
use std::sync::{Arc, RwLock, Weak};

use crate::callback::notification::NotificationHandler;
use crate::callback::request::RequestHandler;
use crate::error::{ResponseError, ErrorT, ListenerForYourRequestHasBeenDeadError};
use crate::internal_handlers::{AddForwarded, RemoveForwarded};
use crate::packet::{OutgoingMessage, OpaquePacketWrapper};
use crate::polling::request::OpaquePollingRequest;
use crate::request::ResponseId;
use crate::{IncomingRequest, Notification, OutgoingRequest, Port, AddressT, IncomingNotification, OutgoingNotification};
use crate::connection::{Connection, ConnectionMessage};
use crate::event::RootEvent;
use crate::polling::notification::OpaquePollingNotification;
use crate::route::{RouteSet, Via, Rtt};
use crate::util::AbortOnDrop;
use async_trait::async_trait;
use bytes::Bytes;
use futures::Future;
use serde::Serialize;
use serde::de::DeserializeOwned;

use tokio::sync::{broadcast, oneshot};
use tokio::sync::mpsc::{unbounded_channel,error::SendError};
use tokio::sync::mpsc::UnboundedSender as Sender;

pub(crate) struct RpcInner<Address: AddressT, Error: ErrorT> {
	me: Address,
	set: RouteSet<Address>,
	#[allow(dead_code)]
	abort: AbortOnDrop,
	tx: Sender<RootEvent<Address>>,
	connections: Vec<Connection<Address>>,
	request_handler: HashMap<&'static str, Arc<dyn RequestHandler<Address>>>,

	pub(crate) polling_request_handler: HashMap<&'static str, Sender<OpaquePollingRequest<Address>>>,

	notification_handler: HashMap<&'static str, Arc<dyn NotificationHandler<Address>>>,
	pub(crate) polling_notification_handler: HashMap<&'static str, Sender<OpaquePollingNotification<Address>>>,

	connect_tx: broadcast::Sender<Address>,

	responses: HashMap<ResponseId, oneshot::Sender<Result<Bytes, Error>>>,
}
impl<Address:AddressT, Error:ErrorT> RpcInner<Address, Error> {
	// TODO: Implement callback handler on top of polling
	fn register_request_handler<R, F>(
		&mut self,
		handler: impl Fn(Address, R) -> F + Sync + Send + 'static,
	) where
		Error: Into<ResponseError> + ErrorT,
		R: IncomingRequest + Sync + Send + 'static,
		R::Response: Serialize,
		F: Future<Output = Result<R::Response, Error>> + Sync + Send + 'static,
	{
		struct CallbackRequestHandler<R, F, H, Address, Error> {
			handler: Box<H>,
			_marker: PhantomData<fn(R, F, Address, Error)>,
		}
		#[async_trait]
		impl<R, F, H, Address, Error: Into<ResponseError>> RequestHandler<Address> for CallbackRequestHandler<R, F, H, Address, Error>
		where
			R: IncomingRequest + Send + Sync + 'static,
			R::Response: Serialize,
			F: Future<Output = Result<R::Response, Error>> + Send + Sync + 'static,
			H: Fn(Address, R) -> F + Send + Sync + 'static,
			Address: AddressT + 'static,
			Error: Send + Sync + 'static,
		{
			async fn handle(
				&self,
				packet_source: Address,
				request: Bytes,
				rid: &str,
				respond_to: Address,
			) -> OutgoingMessage<Address> {
				let request: R = match serde_json::from_slice(&request) {
					Ok(v) => v,
					Err(e) => {
						return OutgoingMessage::new_error_response(
							rid,
							respond_to,
							&format!("failed to parse request: {e}"),
						)
					}
				};
				match (self.handler)(packet_source, request).await {
					Ok(response) => {
						return OutgoingMessage::new_response(rid, respond_to, &response)
					}
					Err(e) => {
						return OutgoingMessage::new_error_response(
							rid,
							respond_to,
							&e.into().0,
						)
					}
				}
			}
		}
		// self.request_handler
		match self.request_handler.entry(R::name()) {
			Entry::Occupied(_) => panic!("request handler is already defined"),
			Entry::Vacant(v) => v.insert(Arc::new(CallbackRequestHandler {
				handler: Box::new(handler),
				_marker: PhantomData,
			})),
		};
	}
	fn register_notification_handler<
		R: Notification + DeserializeOwned,
		F: Future<Output = Result<(), Error>> + Sync + Send + 'static,
	>(
		&mut self,
		handler: impl Fn(Address, R) -> F + Sync + Send + 'static,
		blocking: bool,
	) {
		struct CallbackNotificationHandler<R, F, H, Address, Error> {
			blocking: bool,
			handler: Box<H>,
			_marker: PhantomData<fn(R, F, Address, Error)>,
		}
		#[async_trait]
		impl<R, F, H, Address, Error> NotificationHandler<Address> for CallbackNotificationHandler<R, F, H, Address, Error>
		where
			R: Notification + DeserializeOwned,
			F: Future<Output = Result<(), Error>> + Send + Sync + 'static,
			H: Fn(Address, R) -> F + Send + Sync + 'static,
			Address: AddressT,
			Error: ErrorT,
		{
			fn blocking(&self) -> bool {
				self.blocking
			}
			async fn handle(&self, packet_source: Address, request: Bytes) {
				let request: R = match serde_json::from_slice(&request) {
					Ok(v) => v,
					Err(e) => {
						eprintln!("failed to parse notification: {e}");
						return;
					}
				};
				match (self.handler)(packet_source, request).await {
					Ok(()) => {}
					Err(err) => {
						eprintln!("failed to handle notification: {err}");
						return;
					}
				}
			}
		}
		// self.request_handler
		match self.notification_handler.entry(R::name()) {
			Entry::Occupied(_) => panic!("request handler is already defined"),
			Entry::Vacant(v) => v.insert(Arc::new(CallbackNotificationHandler {
				blocking,
				handler: Box::new(handler),
				_marker: PhantomData,
			})),
		};
	}
	fn remove_direct(&mut self, to: Address)
	where Address: Hash+Eq+Clone{
		let Some(pos) = self.connections.iter().position(|conn| conn.address == to) else {
            return;
        };
		self.connections.remove(pos);
		self.set.on_remove_direct_connection(to);
	}
	fn forwarder_for(&self, address: Address, blacklist: &HashSet<Via<Address>>) -> Option<&Connection<Address>> {
		let forwarder = self.set.forwarder_for(address.clone(), blacklist)?;
		let target = match forwarder {
			Via::Address(address) => address,
			Via::Direct => address.clone(),
		};
		self.connections
			.iter()
			.find(|connection| connection.address == target)
	}
	fn notify<T: OutgoingNotification>(&self, to: Address, notification: &T) {
		self.tx
			.send(OutgoingMessage::new_notification(self.me.clone(), to, notification).into())
			.ok()
			.expect("not closed");
	}

	pub fn complete_response(&mut self, id: ResponseId, data: Result<Bytes, Error>) {
		let Some(pending) = self.responses.remove(&id) else {
            eprintln!("completed already timed out request: {id:?}");
            return;
        };
		if let Err(_e) = pending.send(data) {
			eprintln!("failed to complete response");
			return;
		};
	}

	pub fn request<T>(
		&mut self,
		to: Address,
		request: &T,
	) -> oneshot::Receiver<Result<Bytes, Error>>
	where
		T: OutgoingRequest,
		T::Response: DeserializeOwned,
	{
		let id = uuid::Uuid::new_v4().to_string();
		let (complete, pending) = oneshot::channel();
		self.responses.insert(ResponseId(id.clone()), complete);
		self.tx
			.send(OutgoingMessage::new_request(self.me.clone(), to, id, request).into())
			.ok()
			.expect("not closed");
		// TODO: timeouts
		pending
	}
	fn respond_with_error(&mut self, rid: &str, to: Address, error: &str) {
		self.tx
			.send(OutgoingMessage::new_error_response(rid, to, error).into())
			.ok()
			.expect("not closed")
	}
	fn add_direct(&mut self, to: Address, port: Port, rtt: Rtt)
where Address: Hash+Eq+Clone
	{
		if self.connections.iter().find(|c| c.address == to).is_some() {
			eprintln!("connection is already added: {to:?}");
			return;
		}

		self.set.on_add_direct_connection(to.clone(), rtt);

		let connection = Connection::new(to.clone(), port, self.tx.clone());
		self.connections.push(connection);

		for (route, min_rtt) in self.set.list().collect::<Vec<_>>() {
			let rtt = if min_rtt.via == Via::Address(to.clone()) {
				let Some(rtt) = min_rtt.second_best else {
                    continue;
                };
				rtt
			} else {
				min_rtt.rtt
			};
			self.notify(to.clone(), &AddForwarded { to: route, rtt })
		}
	}
}

async fn handle_connection_message<Address, Error>(inner: Rpc<Address, Error>, input: ConnectionMessage<Address>)
where Error: ErrorT,
	  Address: AddressT
{
	let inner = inner.inner;
	let opaque: OpaquePacketWrapper<Address> = match serde_json::from_slice(&input.message) {
		Ok(w) => w,
		Err(e) => {
			eprintln!("malformed incoming packet: {e}");
			return;
		}
	};
	let me = inner.read().expect("read").me.clone();
	let tx = inner.read().expect("read").tx.clone();
	match &opaque {
		OpaquePacketWrapper::Response {
			rid,
			request_origin,
			error,
		} => {
			if request_origin == &me {
				let mut read = inner.write().expect("read");
				read.complete_response(
					ResponseId(rid.to_owned()),
					match error {
						Some(e) => Err(ResponseError(e.to_owned()).into()),
						None => Ok(input.message.clone()),
					},
				);
				return;
			}
			todo!()
		}
		OpaquePacketWrapper::Request {
			sender,
			receiver,
			request,
			response,
		} => {
			let response = response.clone();
			if !inner
				.read()
				.expect("read")
				.set
				.may_be_forwarder_for(Via::Address(input.packet_source.clone()), sender.clone())
			{
				eprintln!(
					"messages from {:?} should not be forwarded through {:?}",
					sender, input.packet_source,
				);
				return;
			}
			if receiver == &me {
				if let Some(response) = response.clone() {
					// TODO: Handler enum?
					let (request_handler, polling_handler) = {
						let read = inner.read().expect("read");
						(
							read.request_handler
							.get(request.as_str())
							.cloned(),
							read.polling_request_handler.get(request.as_str()).cloned(),
						)
					};
					if let Some(handler) = request_handler {
						let sender = sender.clone();
						let message = input.message.clone();
						tokio::task::spawn(async move {
							let response = handler
								.handle(
									sender.clone(),
									message,
									&response.rid,
									sender.clone(),
								)
								.await;
							if let Err(_) = tx.send(response.into()) {
								eprintln!("failed to send response");
							};
						});
					// TODO: timeout/cancel
					} else if let Some(polling_handler) = polling_handler {
						let ptx = polling_handler.clone();
						let (rtx, rrx) = oneshot::channel();
						let message = input.message.clone();
						if let Err(SendError(poll)) = ptx.send(OpaquePollingRequest {
							from: sender.clone(),
							id: response.rid.clone(),
							request: Some(message),
							respond: Some(rtx),
						}) {
							poll.respond_err::<Error>(From::from(
								ListenerForYourRequestHasBeenDeadError
							));
							return;
						};

						let response = response.clone();
						let sender = sender.clone();

						tokio::task::spawn(async move {
							let response = match rrx.await {
								Ok(v) => v,
								Err(_) => {
									if let Err(_) = tx.send(
										OutgoingMessage::new_error_response(
											&response.rid,
											sender.clone(),
											&format!("no response for polling request"),
										)
										.into(),
									) {
										eprintln!("failed to send response");
									};
									return;
								}
							};
							if let Err(_) = tx.send(response.into()) {
								eprintln!("failed to send response");
							};
						});
					// TODO: timeout/cancel
					} else {
						eprintln!("no handler found for {request} request");
						if let Err(_) = tx.send(
							OutgoingMessage::new_error_response(
								&response.rid,
								sender.clone(),
								&format!("no handler defined for {request}"),
							)
							.into(),
						) {
							eprintln!("failed to send response");
						};
					}
				} else {
					let (notification_handler, polling_notification_handler) = {
						let read = inner.read().expect("read");
						(
							read.notification_handler
							.get(request.as_str())
							.cloned(),
							read.polling_notification_handler
							.get(request.as_str())
							.cloned(),
						)
					};
					if let Some(handler) = notification_handler
					{
						let sender = sender.clone();
						let is_blocking = handler.blocking();
						let message = input.message.clone();
						let task = tokio::task::spawn(async move {
							let _response = handler.handle(sender.clone(), message).await;
						});
						if is_blocking {
							if let Err(e) = task.await {
								eprintln!("blocking notification handler failed: {e:?}");
							};
						}
					// TODO: timeout/cancel
					} else if let Some(polling_handler) = polling_notification_handler {
						let ptx = polling_handler.clone();
						if let Err(_) = ptx.send(OpaquePollingNotification {
							from: sender.clone(),
							request: input.message.clone(),
						}) {
							eprintln!("polling notification listener dead");
							return;
						};

					}
					else {
						eprintln!("no handler found for {request} notification")
					}
				}
				return;
			}
				let mut inner = inner.write().expect("write");
			let Some(forwarder) = 
				inner.forwarder_for(receiver.clone(), &HashSet::new())
			 else {
				if let Some(response) = response.clone() {
					inner.respond_with_error(
						&response.rid,
						sender.clone(),
						&format!("could not forward message: no connection"),
					);
				};
				eprintln!("could not forward packet: {opaque:?}");
				return;
			};
			if let Err(_) = forwarder.sender.send(input.message.clone()) {
				eprintln!("failed to forward");
				return;
			};
		}
	}
}

pub struct WeakRpc<Address:AddressT, Error:ErrorT> {
    inner: Weak<RwLock<RpcInner<Address, Error>>>,
}
impl<Address: AddressT, Error:ErrorT> Clone for WeakRpc<Address, Error> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}
impl<Address: AddressT, Error:ErrorT> WeakRpc<Address, Error> {
    pub fn upgrade(self) -> Option<Rpc<Address, Error>> {
        Some(Rpc {
            inner: self.inner.upgrade()?
        })
    }
}

pub struct Rpc<Address: AddressT, Error:ErrorT> {
	pub(crate) inner: Arc<RwLock<RpcInner<Address, Error>>>,
}
impl<Address:AddressT, Error:ErrorT> Clone for Rpc<Address, Error> {
	fn clone(&self) -> Self {
	    Self {
			inner: self.inner.clone()
		}
	}
}
impl<Address:AddressT, Error:ErrorT> Rpc<Address, Error> {
    pub fn downgrade(self) -> WeakRpc<Address, Error> {
        WeakRpc { inner: Arc::downgrade(&self.inner) }
    }
}

impl<Address:AddressT, Error:ErrorT> Rpc<Address, Error>
where
	Address: Hash + Eq + Clone,
	Error: From<serde_json::Error>,
{

	pub fn register_request_handler<
		R: IncomingRequest + Sync + Send + 'static,
		F: Future<Output = Result<R::Response, Error>> + Sync + Send + 'static,
	>(
		&self,
		handler: impl Fn(Address, R) -> F + Sync + Send + 'static,
	) where
		R::Response: Serialize,
	{
		let mut inner = self.inner.write().expect("write");
		inner.register_request_handler(handler)
	}
	pub fn register_notification_handler<
		R: IncomingNotification,
		F: Future<Output = Result<(), Error>> + Sync + Send + 'static,
	>(
		&self,
		handler: impl Fn(Address, R) -> F + Sync + Send + 'static,
	) {
		let mut inner = self.inner.write().expect("write");
		inner.register_notification_handler(handler, false)
	}


	/// Only use for requests, which should be done on the current state of network, can't be
	/// processed in parallel, and executed very fast
	pub fn register_blocking_notification_handler<
		R: IncomingNotification,
		F: Future<Output = Result<(), Error>> + Sync + Send + 'static,
	>(
		&self,
		handler: impl Fn(Address, R) -> F + Sync + Send + 'static,
	) {
		let mut inner = self.inner.write().expect("write");
		inner.register_notification_handler(handler, true)
	}
	pub fn new(me: Address) -> Self {
		let (etx, mut erx) = unbounded_channel();
		let (connection_tx, _) = broadcast::channel(1000);
		let connection_tx2 = connection_tx.clone();
		let set = RouteSet::new(etx.clone());

		let (set_pending, get_pending) = tokio::sync::oneshot::channel();
		let join_handle = tokio::spawn(async move {
			let inner: Arc<RwLock<RpcInner<Address, Error>>> =
				get_pending.await.map_err(|_| ()).expect("get pending");
			while let Some(erx) = erx.recv().await {
				match erx {
					RootEvent::ConnectionMessage(input) => {
						handle_connection_message(
							Rpc {
								inner: inner.clone(),
							},
							input,
						)
						.await;
					}
					RootEvent::ConnectionEnding(ending) => {
						let mut inner = inner.write().expect("write");
						inner.remove_direct(ending.from)
					}

					RootEvent::OutgoingMessage(out) => {
						let inner = inner.read().expect("write");
						let Some(forwarder) = inner.forwarder_for(out.to.clone(), &HashSet::new()) else {
							eprintln!("no path found: {:?} {:?}", out.to.clone(), inner.connections);
							continue;
						};
						if let Err(_) = forwarder.sender.send(out.message) {
							eprintln!("failed to forward");
							continue;
						};
					}

					RootEvent::MinRttUpdated(updated) => {
						let mut inner = inner.write().expect("write");
						let mut addresses = Vec::new();
						for connection in inner.connections.iter_mut() {
							let Some(update) = updated.update_for(connection.address.clone()) else {
								continue;
							};
							addresses.push((connection.address.clone(), update));
						}
						for (target, update) in addresses {
							inner.notify(target, &update);
						}
					}
					RootEvent::ViaListSeconded(seconded) => {
						let mut inner = inner.write().expect("write");
						let mut addresses = Vec::new();
						for connection in inner.connections.iter_mut() {
							if seconded.initial_via != Via::Address(connection.address.clone()) {
								continue;
							}
							addresses.push(connection.address.clone());
						}
						for addr in addresses {
							inner.notify(
								addr,
								&AddForwarded {
									to: seconded.for_connection.clone(),
									rtt: seconded.rtt,
								},
							)
						}
					}
					RootEvent::ViaListUnseconded(seconded) => {
						let mut inner = inner.write().expect("write");
						let mut addresses = Vec::new();
						for connection in inner.connections.iter_mut() {
							if seconded.only_via != Via::Address(connection.address.clone()) {
								continue;
							}
							addresses.push(connection.address.clone());
						}
						for addr in addresses {
							inner.notify(
								addr,
								&RemoveForwarded {
									to: seconded.for_connection.clone(),
								},
							)
						}
					}
					RootEvent::ConnectionAdded(added) => {
						let _ = connection_tx.send(added.to.clone());
						let mut inner = inner.write().expect("write");
						let mut addresses = Vec::new();
						for connection in inner.connections.iter_mut() {
							if added.to == connection.address {
								eprint!("racy");
								continue;
							}
							if added.via == Via::Address(connection.address.clone()) {
								continue;
							}
							addresses.push(connection.address.clone());
						}
						for addr in addresses {
							inner.notify(
								addr,
								&AddForwarded {
									to: added.to.clone(),
									rtt: added.rtt,
								},
							)
						}
					}
					RootEvent::ConnectionRemoved(removed) => {
						let mut inner = inner.write().expect("write");
						let mut addressed = Vec::new();
						for connection in inner.connections.iter_mut() {
							if removed.to == connection.address {
								eprint!("racy");
								continue;
							}
							if removed.via == Via::Address(connection.address.clone()) {
								continue;
							}
							addressed.push(connection.address.clone());
						}
						for addr in addressed {
							inner.notify(addr, &RemoveForwarded { to: removed.to.clone() })
						}
					}
				}
			}
			eprintln!("rpc worker finished")
		});
		let abort = AbortOnDrop(join_handle.abort_handle());
		let inner = Arc::new(RwLock::new(RpcInner {
			me,
			set,
			connections: Vec::new(),
			abort,
			tx: etx,
			request_handler: Default::default(),
			polling_request_handler: Default::default(),
			notification_handler: Default::default(),
			polling_notification_handler: Default::default(),
			responses: Default::default(),
			connect_tx: connection_tx2,
		}));
		set_pending
			.send(inner.clone())
			.map_err(|_| ())
			.expect("set_pending");
		let rpc = Self {
			inner: inner.clone(),
		};

		let inner = inner.clone();
		rpc.register_blocking_notification_handler(move |source: Address, add: AddForwarded<Address>| {
			eprintln!("{source:?} added forwarded {add:?}");
			let inner = inner.clone();
			async move {
				let mut inner = inner.write().expect("read");
				if inner
					.connections
					.iter()
					.find(|c| c.address == source)
					.is_none()
				{
					eprintln!("connection is not direct: {source:?} -> {add:?}");
					return Ok(());
				}
				inner.set.inc(add.to, Via::Address(source), add.rtt);
				Ok(())
			}
		});

		rpc
	}
	pub fn remove_direct(&self, to: Address) {
		let mut inner = self.inner.write().expect("read");
		inner.remove_direct(to);
	}
	pub fn add_direct(&self, to: Address, port: Port, rtt: Rtt) {
		let mut inner = self.inner.write().expect("read");
		inner.add_direct(to, port, rtt);
	}
	pub fn notify<T: OutgoingNotification>(&self, to: Address, notification: &T) {
		let inner = self.inner.read().expect("read");
		inner.notify(to, notification)
	}

	pub async fn request<T: OutgoingRequest>(
		&self,
		to: Address,
		request: &T,
	) -> Result<T::Response, Error>
	where
		T::Response: DeserializeOwned,
	{
		let ch = {
			let mut inner = self.inner.write().expect("read");
			inner.request(to, request)
		};
		let res = ch.await;
		match res {
			Ok(Ok(v)) => match serde_json::from_slice(&v) {
				Ok(v) => Ok(v),
				Err(e) => Err(From::from(e)),
			},
			Ok(Err(e)) => Err(e),
			Err(e) => Err(e.into()),
		}
	}

	pub async fn wait_for_connection_to(&self, address: Address) -> Result<(), WaitError> {
		let mut wait = {
			let inner = self.inner.write().expect("write");
			let listener = inner.connect_tx.subscribe();
			if inner.set.has(address.clone()) {
				return Ok(());
			}
			listener
		};
		loop {
			match wait.recv().await {
			    Ok(a) if a == address => {return Ok(())},
				Ok(_) => {},
				Err(_) => return Err(WaitError)
			}
		}
	}
}

pub struct WaitError;
