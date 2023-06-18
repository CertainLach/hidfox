#![feature(try_blocks)]

use core::pin::pin;
use std::{
	collections::{BTreeSet, HashMap},
	ffi::CString,
	io::ErrorKind,
	time::Duration,
};

use bifrostlink::{
	error::{ErrorT, ListenerForYourRequestHasBeenDeadError, ResponseError},
	native_messaging_port, notification, request, AddressT, PollingRequest, Rtt,
};
use futures::StreamExt;
use hidapi::{HidApi, HidDevice, HidResult};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_with::{serde_as, Bytes};
use tokio::{select, time};
use url::Url;

mod route;

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Address {
	Native,
	Background,
	Popup,
	Content,
	Injected,
}
impl AddressT for Address {}
#[derive(thiserror::Error, Debug)]
enum Error {
	#[error("response: {0:?}")]
	Response(ResponseError),
	#[error("json: {0}")]
	Json(#[from] serde_json::Error),
	#[error("recv: {0}")]
	Recv(#[from] tokio::sync::oneshot::error::RecvError),
	#[error("listener for your request has been dead")]
	LFYRHBDE(ListenerForYourRequestHasBeenDeadError),
}
impl Into<ResponseError> for Error {
	fn into(self) -> ResponseError {
		ResponseError(format!("{self}"))
	}
}
impl From<ResponseError> for Error {
	fn from(value: ResponseError) -> Self {
		Self::Response(value)
	}
}
impl From<ListenerForYourRequestHasBeenDeadError> for Error {
	fn from(value: ListenerForYourRequestHasBeenDeadError) -> Self {
		Self::LFYRHBDE(value)
	}
}
impl ErrorT for Error {}
type Rpc = bifrostlink::Rpc<Address, Error>;

fn from_json<'i, T: Deserialize<'i>>(s: &'i [u8]) -> Option<T> {
	let mut de = serde_json::Deserializer::from_slice(s);
	match serde_path_to_error::deserialize(&mut de) {
		Ok(v) => Some(v),
		Err(e) => {
			eprintln!("failed to deserialize {s:?}: {e}");
			None
		}
	}
}

fn cleanup_url_to_id(url: &mut Url) {
	assert_eq!(url.scheme(), "https", "only https clients supported");
	url.set_fragment(None);
	url.set_query(None);
}

#[derive(Serialize, Deserialize)]
struct OpenFromInject {
	url: Url,
}
request!(OpenFromInject => NoopResponse);

#[derive(Deserialize)]
struct ConnectHid {
	id: String,
}
request!(ConnectHid => NoopResponse);

#[derive(Serialize, Deserialize)]
struct SubscribeHid {}
request!(SubscribeHid => NoopResponse);

#[tokio::main(flavor = "current_thread")]
async fn main() {
	#[cfg(tokio_unstable)]
	console_subscriber::init();

	eprintln!("Welcome to WebHID Firefox logs!");

	let port = native_messaging_port();
	let mut rpc = Rpc::new(Address::Native);

	rpc.register_request_handler(|source, mut data: OpenFromInject| async move {
		cleanup_url_to_id(&mut data.url);
		Ok(NoopResponse {})
	});

	let mut connect_hid = rpc
		.register_polling_request_handler::<ConnectHid>()
		.unwrap();
	let mut subscribe_hid = rpc
		.register_polling_request_handler::<SubscribeHid>()
		.unwrap();

	rpc.add_direct(Address::Background, port, Rtt(50));

	eprintln!("trying storage get");
	let v = storage_get::<String>(&rpc, "helo").await;
	eprintln!("storage get result: {v:?}");

	select! {
		Some(connect) = connect_hid.next() => {
			let id = connect.data().id.clone();
			connect.respond_ok(NoopResponse{});
			device(rpc, Url::parse("https://test.com").unwrap(), id).await;
		}
		Some(subscribe_hid) = subscribe_hid.next() => {
			hid(rpc, Url::parse("https://test.com").unwrap(), subscribe_hid).await;
		}
	};

	loop {
		tokio::time::sleep(Duration::from_secs(5)).await;
		eprintln!("tick");
	}
}

#[derive(Serialize, Deserialize)]
struct DeviceInfo {
	vendor_id: u16,
	product_id: u16,
	usage: u16,
	usage_page: u16,
}
#[derive(Serialize, Deserialize, Debug)]
struct PersistentDeviceId {
	vendor_id: u16,
	product_id: u16,
	usage: u16,
	usage_page: u16,
	serial: String,
}
async fn get_allowed_persistent(reader: &Rpc, url: &Url) -> Vec<PersistentDeviceId> {
	storage_get(reader, &format!("allowed.persistent:{url}"))
		.await
		.unwrap_or_default()
}
async fn set_allowed_persistent(reader: &Rpc, url: &Url, allowed: Vec<PersistentDeviceId>) {
	storage_set(reader, &format!("allowed.persistent:{url}"), &allowed).await;
}

#[derive(PartialEq, PartialOrd, Ord, Eq, Debug, Serialize)]
struct DeviceId {
	vendor_id: u16,
	product_id: u16,
	usage: u16,
	usage_page: u16,
	path: CString,
	serial: Option<String>,
}
impl DeviceId {
	fn from_info(i: &hidapi::DeviceInfo) -> Self {
		DeviceId {
			vendor_id: i.vendor_id(),
			product_id: i.product_id(),
			path: i.path().to_owned(),
			serial: i.serial_number().map(ToOwned::to_owned),
			usage: i.usage(),
			usage_page: i.usage_page(),
		}
	}
	/// Nonce is used for one-time allowed devices
	fn id_with_nonce(&self, nonce: u32) -> String {
		use base64::engine::Engine;
		use sha2::Digest;
		let serialized =
			serde_json::to_vec(&(&self, nonce)).expect("serialization should not fail");
		let mut digest = sha2::Sha256::digest(&serialized);
		let mut out = String::new();
		base64::engine::general_purpose::STANDARD_NO_PAD.encode_string(&digest, &mut out);
		out
	}
	fn id(&self) -> String {
		self.id_with_nonce(0)
	}
	/// Devices only have persistent id, if they have assigned serial number
	fn persistent(&self) -> Option<PersistentDeviceId> {
		Some(PersistentDeviceId {
			vendor_id: self.vendor_id,
			product_id: self.product_id,
			usage: self.usage,
			usage_page: self.usage_page,
			serial: self.serial.clone()?,
		})
	}
	fn info(&self) -> DeviceInfo {
		DeviceInfo {
			vendor_id: self.vendor_id,
			product_id: self.product_id,
			usage: self.usage,
			usage_page: self.usage_page,
		}
	}
}
//
fn list_allowed_devices<'a>(
	hid: &'a mut HidApi,
	persisted: &'a [PersistentDeviceId],
) -> impl Iterator<Item = DeviceId> + 'a {
	hid.device_list()
		.filter(|info| {
			let Some(serial) = info.serial_number() else {
			        return false;
			    };
			persisted.iter().any(|a| {
				a.serial == serial
					&& a.vendor_id == info.vendor_id()
					&& a.product_id == info.product_id()
			})
		})
		.map(|i| DeviceId::from_info(i))
}
fn open_device_by_id(hid: &mut HidApi, id: &DeviceId) -> HidResult<HidDevice> {
	let dev = hid.open_path(&id.path)?;
	let info = dev.get_device_info()?;
	let new_fetch = DeviceId::from_info(&info);
	if &new_fetch != id {
		// Path now has another device connected
		return Err(hidapi::HidError::HidApiError {
			message: "outdated device list".to_owned(),
		});
	}
	Ok(dev)
}
//
#[derive(Deserialize, Serialize, Debug)]
struct Filter {
	vendor_id: Option<u16>,
	product_id: Option<u16>,
	usage: Option<u16>,
	usage_page: Option<u16>,
}

#[derive(Deserialize)]
struct RequestDevice {
	filters: Vec<Filter>,
}
request!(RequestDevice => NoopResponse);
#[derive(Serialize, Debug)]
struct RequestedDevice {
	id: String,
	vid: u16,
	pid: u16,
	product_name: String,
	serial: String,
}

#[derive(Serialize)]
struct RequestAccess {
	devices: Vec<RequestedDevice>,
}
#[derive(Deserialize)]
struct RequestAccessResult {
	approved: Vec<String>,
}
request!(RequestAccess => RequestAccessResult);

#[derive(Serialize, Deserialize)]
struct RemovedDevice {
	id: String,
}
notification!(RemovedDevice);
#[derive(Serialize, Deserialize)]
struct AddedDevice {
	id: String,
	info: DeviceInfo,
}
notification!(AddedDevice);

#[derive(Deserialize, Serialize)]
struct NoopResponse {}

#[derive(Serialize)]
struct OpenPopup {}
request!(OpenPopup => NoopResponse);

/// This request will be completed after device refresh
#[derive(Deserialize)]
struct PollRefresh {}
request!(PollRefresh => NoopResponse);
//
async fn hid(mut reader: Rpc, url: Url, req: PollingRequest<SubscribeHid, Address>) {
	const DEVICE_REFRESH_POLLING_INTERVAL: Duration = Duration::from_millis(400);

	let mut hid = HidApi::new().expect("hidapi init");

	let mut device_list = <BTreeSet<DeviceId>>::new();

	eprintln!("registered pollrefresh");
	let mut request_device = reader
		.register_polling_request_handler::<RequestDevice>()
		.unwrap();
	let mut poll_refresh = reader
		.register_polling_request_handler::<PollRefresh>()
		.unwrap();

	req.respond_ok(NoopResponse {});

	loop {
		eprintln!("hid watcher tick");
		// TODO: Watch for changes
		let persisted = get_allowed_persistent(&reader, &url).await;

		let new_device_list: BTreeSet<DeviceId> =
			list_allowed_devices(&mut hid, &persisted).collect();
		for removed in device_list.difference(&new_device_list) {
			reader.notify(Address::Injected, &RemovedDevice { id: removed.id() });
		}
		for added in new_device_list.difference(&device_list) {
			reader.notify(
				Address::Injected,
				&AddedDevice {
					id: added.id(),
					info: added.info(),
				},
			);
		}
		device_list = new_device_list;

		// TODO: block-in-place doesn't work with one thread
		hid = tokio::task::spawn_blocking(move || {
			if let Err(e) = hid.refresh_devices() {
				eprintln!("failed to refresh devices: {e}");
			}
			hid
		})
		.await
		.expect("should not fail");
		// TODO: Events
		let mut delay = pin!(time::sleep(DEVICE_REFRESH_POLLING_INTERVAL));
		'process_requests: loop {
			select! {
				Some(req) = request_device.next() => {
					let RequestDevice {filters} = req.data();
					let mut proposed_devices = HashMap::new();
					let devices = hid.device_list().filter(|d| {
						filters.is_empty() || filters.iter().any(|f| {
							if let Some(vendor_id) = f.vendor_id {
								if vendor_id != d.vendor_id() {
									return false;
								}
							}
							if let Some(product_id) = f.product_id {
								if product_id != d.product_id() {
									return false;
								}
							}
							if let Some(usage_page) = f.usage_page {
								if usage_page != d.usage_page() {
									return false;
								}
							}
							if let Some(usage) = f.usage {
								if usage != d.usage() {
									return false;
								}
							}
							true

						})
					}).filter_map(|d| {
						let devid = DeviceId::from_info(d);
						let id = devid.id();
						proposed_devices.insert(id.clone(), devid.persistent()?);
						Some(RequestedDevice {
							id,
							vid: d.vendor_id(),
							pid: d.product_id(),
							product_name: d.product_string().unwrap_or("<unknown>").to_owned(),
							serial: d.serial_number().unwrap_or("<unknown>").to_string(),
						})
					}).collect::<Vec<_>>();
					eprintln!("requested device");

					if let Err(_e) = reader.request(Address::Background, &OpenPopup {}).await {
						req.respond_err("failed to open popup");
						continue;
					};
					eprintln!("open popup");
					if let Err(_) = reader.wait_for_connection_to(Address::Popup).await {
						req.respond_err("failed to open popup");
						continue;
					};
					let list = match reader.request(Address::Popup, &RequestAccess {devices}).await {
						Ok(l) => l,
						Err(_) => {
							req.respond_err("popup is ignoring us");
							continue;
						}
					};

					let list = list.approved.into_iter().filter_map(|d| proposed_devices.remove(&d)).collect::<Vec<_>>();
					let mut allowed = get_allowed_persistent(&reader, &url).await;
					// TODO: Deduplicate
					allowed.extend(list);
					set_allowed_persistent(&reader, &url, allowed).await;

					//reader.request(Address::Popup);
					// notify(&PopupRequest::RequestAccess { devices }, Address::Popup);
					req.respond_ok(NoopResponse{});
				}
				Some(req) = poll_refresh.next() => {
					req.respond_ok(NoopResponse{});
				}
				() = &mut delay => {
					break 'process_requests;
				}
			}
		}
	}
}
#[serde_as]
#[derive(Serialize, Deserialize)]
struct Report {
	id: u8,
	#[serde_as(as = "Bytes")]
	data: Vec<u8>,
}
notification!(Report);

#[derive(Serialize, Deserialize)]
struct SendReport {
	report: Report,
}
notification!(SendReport);
#[derive(Serialize, Deserialize)]
struct SendFeatureReport {
	report: Report,
}
notification!(SendFeatureReport);

#[derive(Deserialize)]
struct ReceiveFeatureReport {
	id: u8,
}
#[serde_as]
#[derive(Serialize)]
struct ReceiveFeatureReportResponse {
	#[serde_as(as = "Bytes")]
	data: [u8; 64],
}
request!(ReceiveFeatureReport => ReceiveFeatureReportResponse);

async fn device(mut reader: Rpc, url: Url, id: String) {
	let mut hid = HidApi::new().expect("hidapi init");
	let persistent = get_allowed_persistent(&mut reader, &url).await;
	let Some(id) = list_allowed_devices(&mut hid, &persistent).filter(|dev| dev.id() == id).next() else {
        eprintln!("device not found");

        // notify(&DeviceConnectResponse::ConnectionError{error:"device not found".to_owned()}, Address::Injected);
        return;
    };
	let dev = match open_device_by_id(&mut hid, &id) {
		Ok(v) => v,
		Err(e) => {
			eprintln!("failed to open device: {e}");
			// notify(
			// 	&DeviceConnectResponse::ConnectionError {
			// 		error: format!("open failed: {e}"),
			// 	},
			// 	Address::Injected,
			// );
			return;
		}
	};

	let mut send_report = reader.register_polling_notification_handler::<SendReport>();
	let mut send_feature_report =
		reader.register_polling_notification_handler::<SendFeatureReport>();
	let mut receive_feat_report = reader
		.register_polling_request_handler::<ReceiveFeatureReport>()
		.unwrap();

	dev.set_blocking_mode(false).expect("unfuck blocking");
	// notify(&DeviceConnectResponse::Connected, Address::Injected);
	loop {
		loop {
			let mut out = [0; 65];
			let size = match dev.read(&mut out) {
				Ok(size) => size,
				Err(hidapi::HidError::IoError { error })
					if error.kind() == ErrorKind::WouldBlock =>
				{
					break
				}
				Err(hidapi::HidError::HidApiError { message })
					if message.contains("(device disconnected)") =>
				{
					// TODO: notify disconnect
					// notify(&DeviceData::Disconnected, Address::Injected);
					return;
				}
				Err(e) => panic!("recv error: {e}"),
			};
			match size {
				0 => break,
				// Has report id
				65 => {
					let id = out[0];
					let mut data = [0u8; 64];
					data.copy_from_slice(&out[1..size]);
					reader.notify(Address::Injected, &Report { id, data:data.to_vec() });
				}
				// No report id
				64 => {
					let mut data = [0u8; 64];
					data.copy_from_slice(&out[0..size]);
					reader.notify(Address::Injected, &Report { id: 0, data: data.to_vec() });
				}
				_ => unreachable!("report size should be either 64 or 65 bytes"),
			}
		}

		let mut delay = pin!(time::sleep(Duration::from_millis(5)));
		select! {
			msg = send_report.recv() => {
				let Some(report) = msg else {
					break;
				};
				let Report {id, ref data} = report.data().report;
				let mut data = data.to_vec();
					data.insert(0, id);

				dev.write(&data).expect("send failed");
			}
			msg = send_feature_report.recv() => {
				let Some(report) = msg else {
					break;
				};
				let Report {id, data} = &report.data().report;
				let mut data = data.to_vec();
					data.insert(0, *id);

				dev.send_feature_report(&data).expect("send failed");
			}
			Some(recv) = receive_feat_report.next() => {
				let mut data = [0u8;65];
				data[0] = recv.data().id;
				match dev.get_feature_report(&mut data) {
					Ok(_f) => {
						let mut res_data = [0u8; 64];
						res_data.copy_from_slice(&data[1..]);
						recv.respond_ok(ReceiveFeatureReportResponse{
							data: res_data
						});
					}
					Err(_e) => {
						recv.respond_err("failed to get feature report")
					}
				};
			}
			() = &mut delay => {
			}
		}
	}
}

async fn storage_get<T: DeserializeOwned>(r: &Rpc, key: &str) -> Option<T> {
	#[derive(Serialize, Deserialize)]
	struct StorageGet {
		key: String,
	}
	request!(StorageGet => StorageGetR);
	#[derive(Serialize, Deserialize)]
	struct StorageGetR {
		value: Option<String>,
	}

	let result = match r
		.request(
			Address::Background,
			&StorageGet {
				key: key.to_owned(),
			},
		)
		.await
	{
		Ok(v) => v,
		Err(e) => {
			eprintln!("storage request failed: {e:?}");
			return None;
		}
	};

	let value = result.value?;

	match serde_json::from_str(&value) {
		Ok(v) => return Some(v),
		Err(e) => {
			eprintln!("storage decode failed: {e}");
			return None;
		}
	}
}
async fn storage_remove(r: &mut Rpc, key: &str) {
	#[derive(Serialize)]
	struct StorageRemove {
		key: String,
	}
	request!(StorageRemove => NoopResponse);
	let _ = r
		.request(
			Address::Background,
			&StorageRemove {
				key: key.to_owned(),
			},
		)
		.await;
}
async fn storage_set<T: Serialize>(r: &Rpc, key: &str, value: &T) {
	#[derive(Serialize)]
	struct StorageSet {
		key: String,
		value: String,
	}
	request!(StorageSet => NoopResponse);
	let serialized = serde_json::to_string(value).expect("serialize failed");
	let _ = r
		.request(
			Address::Background,
			&StorageSet {
				key: key.to_owned(),
				value: serialized,
			},
		)
		.await;
}
