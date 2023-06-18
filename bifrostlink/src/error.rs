use std::fmt::Display;

use tokio::sync::oneshot::error::RecvError;

#[derive(Debug)]
pub struct ResponseError(pub String);
#[derive(Debug)]
pub struct ListenerForYourRequestHasBeenDeadError;

pub trait ErrorT:
	Send
	+ Sync
	+ 'static
	+ Display
	+ From<ResponseError>
	+ Into<ResponseError>
	+ From<serde_json::Error>
	+ From<RecvError>
	+ From<ListenerForYourRequestHasBeenDeadError>
{
}
