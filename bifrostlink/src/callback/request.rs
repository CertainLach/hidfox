use async_trait::async_trait;
use bytes::Bytes;

use crate::packet::OutgoingMessage;


#[async_trait]
pub(crate) trait RequestHandler<Address>: Sync + 'static + Send {
	async fn handle(
		&self,
		packet_source: Address,
		request: Bytes,
		rid: &str,
		respond_to: Address,
	) -> OutgoingMessage<Address>;
}
