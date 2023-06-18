use bytes::Bytes;
use tokio::sync::mpsc::UnboundedSender as Sender;

use crate::{event::RootEvent, util::AbortOnDrop, AddressT, Port};

#[derive(Debug)]
pub struct Connection<Address> {
	pub(crate) address: Address,
	/// Sender part of a deconstructed port
	pub(crate) sender: Sender<Bytes>,
	#[allow(dead_code)]
	port_abort: AbortOnDrop,
	#[allow(dead_code)]
	abort: AbortOnDrop,
}
impl<Address: AddressT> Connection<Address> {
	pub(crate) fn new(address: Address, port: Port, output: Sender<RootEvent<Address>>) -> Self {
		let Port {
			sender,
			mut receiver,
			abort_handle: port_abort,
		} = port;

		let packet_source = address.clone();
		let join_handle = tokio::task::spawn(async move {
			while let Some(input) = receiver.recv().await {
				if let Err(e) = output.send(
					ConnectionMessage {
						packet_source: packet_source.clone(),
						message: input,
					}
					.into(),
				) {
					eprintln!("port to rpc sender failed: {e}");
					break;
				}
			}
			eprintln!("port data ended");
			if let Err(e) = output.send(
				ConnectionEnding {
					from: packet_source,
				}
				.into(),
			) {
				eprintln!("port to rpc ending sender failed: {e}");
			}
		});
		let abort = AbortOnDrop(join_handle.abort_handle());

		Self {
			address,
			sender,
			port_abort,
			abort,
		}
	}
}

#[derive(Debug)]
pub struct ConnectionMessage<Address> {
	/// Direct connection, which was sent this message
	/// Not the one, from which the message is originally set from
	pub(crate) packet_source: Address,
	pub(crate) message: Bytes,
}
#[derive(Debug)]
pub struct ConnectionEnding<Address> {
	pub(crate) from: Address,
}
