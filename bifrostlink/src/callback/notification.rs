use async_trait::async_trait;
use bytes::Bytes;

#[async_trait]
pub(crate) trait NotificationHandler<Address>: Sync + 'static + Send {
	fn blocking(&self) -> bool;
	async fn handle(&self, packet_source: Address, notification: Bytes);
}
