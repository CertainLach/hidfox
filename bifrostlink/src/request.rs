use serde::{de::DeserializeOwned, Serialize};

pub trait Request: Send + Sync + 'static {
	type Response;
	fn name() -> &'static str;
}
#[macro_export]
macro_rules! request {
	($name:ident => $response:ty) => {
		impl $crate::Request for $name {
			type Response = $response;
			fn name() -> &'static str {
				stringify!($name)
			}
		}
	};
}

pub trait IncomingRequest: Request + DeserializeOwned
where
	<Self as Request>::Response: Serialize,
{
}
impl<T> IncomingRequest for T
where
	T: Request + DeserializeOwned,
	T::Response: Serialize,
{
}
pub trait OutgoingRequest: Request + Serialize
where
	<Self as Request>::Response: DeserializeOwned,
{
}
impl<T> OutgoingRequest for T
where
	T: Request + Serialize,
	T::Response: DeserializeOwned,
{
}

#[derive(PartialEq, Eq, Hash, Debug)]
pub(crate) struct ResponseId(pub String);
