use std::{time::Duration, collections::{BTreeSet, HashMap}, pin::pin};

use bifrostlink::{request, notification, PollingRequest};
use futures::StreamExt;
use hidapi::HidApi;
use serde::{Deserialize, Serialize};
use tokio::{time, select};
use url::Url;

use crate::{DeviceInfo, Rpc, SubscribeHid, Address, DeviceId, list_allowed_devices, storage::storage_get_prefix};


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

	let allowed_prefix = format!("allowed:{url}:");

	loop {
		eprintln!("hid watcher tick");
		// TODO: Watch for changes
		let persisted = storage_get_prefix(&reader, &allowed_prefix).await;

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
