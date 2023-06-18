use std::{
	future::Future,
	io::{self, Read, Write},
};

use bytes::{Bytes, BytesMut};
use tokio::{
	join,
	sync::mpsc::{unbounded_channel, UnboundedReceiver as Receiver, UnboundedSender as Sender},
	task::spawn_blocking,
};
use tracing::error;

use crate::util::AbortOnDrop;

/// Transport abstraction, duplex message-based stream
pub struct Port {
	pub(crate) sender: Sender<Bytes>,
	pub(crate) receiver: Receiver<Bytes>,
	pub(crate) abort_handle: AbortOnDrop,
}
impl Port {
	pub fn new<F: Future<Output = ()> + Send + 'static>(
		handle: impl FnOnce(Receiver<Bytes>, Sender<Bytes>) -> F,
	) -> Self {
		// Bounded should work just fine, due to OS stdio backpressure?
		let (sender, rx) = unbounded_channel();
		let (tx, receiver) = unbounded_channel();

		let join_handle = tokio::task::spawn(handle(rx, tx));
		let abort_handle = AbortOnDrop(join_handle.abort_handle());

		Self {
			sender,
			receiver,
			abort_handle,
		}
	}
}

pub fn native_messaging_port() -> Port {
	Port::new(|mut rx, tx| async move {
		let stdout_printer = spawn_blocking(move || {
			let mut stdout = std::io::stdout().lock();
			while let Some(out) = rx.blocking_recv() {
				let len = u32::try_from(out.len()).expect("can't be larger");
				let succeeded: io::Result<()> = try {
					let size = u32::to_ne_bytes(len);
					stdout.write_all(&size)?;
					stdout.write_all(&out)?;
					stdout.flush()?;
				};
				if let Err(e) = succeeded {
					error!("stdout write failed: {e}");
					break;
				}
			}
			eprintln!("output stream end")
		});
		let stdin_reader = spawn_blocking(move || {
			let mut stdin = std::io::stdin().lock();
			loop {
				let succeeded: io::Result<()> = try {
					let mut size = [0; 4];
					stdin.read_exact(&mut size)?;
					let size = u32::from_ne_bytes(size) as usize;
					let mut buf = BytesMut::zeroed(size);
					stdin.read_exact(&mut buf)?;
					if let Err(_) = tx.send(buf.freeze()) {
						break;
					}
				};
				if let Err(e) = succeeded {
					error!("stdin read failed: {e}");
					break;
				};
			}
			eprintln!("input stream end");
		});

		// TODO: select!
		let (a, b) = join!(stdout_printer, stdin_reader);
		a.unwrap();
		b.unwrap();
	})
}
