use std::fmt::Display;

use bytes::{BufMut, Bytes, BytesMut};
use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::{OutgoingNotification, OutgoingRequest, AddressT};

#[derive(Debug)]
pub struct OutgoingMessage<Address> {
	pub(crate) to: Address,
	pub(crate) message: Bytes,
}
impl<Address> OutgoingMessage<Address>
where
	Address: AddressT,
{
	fn new<T: Serialize>(wrapper: PacketWrapper<Address, T>) -> Self {
		let to = match &wrapper {
			PacketWrapper::Response { request_origin, .. } => request_origin,
			PacketWrapper::Request { receiver, .. } => receiver,
		};
		let bytes = BytesMut::new();
		let mut writer = bytes.writer();
		serde_json::to_writer(&mut writer, &wrapper).expect("serialization should not fail");
		let bytes = writer.into_inner();
		Self {
			to: to.clone(),
			message: bytes.freeze(),
		}
	}
	pub(crate) fn new_notification<T: OutgoingNotification>(
		sender: Address,
		receiver: Address,
		data: &T,
	) -> Self {
		Self::new(PacketWrapper::Request {
			sender,
			receiver,
			request: T::name().to_owned(),
			response: None,
			data,
		})
	}
	pub fn new_request<T: OutgoingRequest>(
		sender: Address,
		receiver: Address,
		id: String,
		data: &T,
	) -> Self
	where
		T::Response: DeserializeOwned,
	{
		Self::new(PacketWrapper::Request {
			sender,
			receiver,
			request: T::name().to_owned(),
			response: Some(ResponseTo { rid: id.to_owned() }),
			data,
		})
	}
	pub fn new_error_response<E: Display>(rid: &str, receiver: Address, error: E) -> Self {
		Self::new(PacketWrapper::Response {
			rid: rid.to_owned(),
			request_origin: receiver,
			error: Some(error.to_string()),
			data: (),
		})
	}
	pub fn new_response<T: Serialize>(rid: &str, receiver: Address, data: &T) -> Self {
		Self::new(PacketWrapper::Response {
			rid: rid.to_owned(),
			request_origin: receiver,
			error: None,
			data,
		})
	}
}

#[derive(Deserialize, Debug)]
#[serde(untagged)]
pub(crate) enum OpaquePacketWrapper<Address> {
	Response {
		rid: String,
		request_origin: Address,
		error: Option<String>,
	},
	Request {
		sender: Address,
		receiver: Address,
		request: String,
		response: Option<ResponseTo>,
	},
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct ResponseTo {
	pub(crate) rid: String,
}
#[derive(Serialize, Deserialize, Debug)]
#[serde(untagged)]
enum PacketWrapper<Address, T> {
	Response {
		rid: String,
		request_origin: Address,
		error: Option<String>,
		#[serde(flatten)]
		data: T,
	},
	Request {
		sender: Address,
		receiver: Address,
		request: String,
		response: Option<ResponseTo>,
		#[serde(flatten)]
		data: T,
	},
}
