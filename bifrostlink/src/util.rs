use tokio::task::AbortHandle;

#[derive(Debug)]
#[allow(dead_code)]
pub struct AbortOnDrop(pub AbortHandle);
impl Drop for AbortOnDrop {
	fn drop(&mut self) {
		self.0.abort()
	}
}
