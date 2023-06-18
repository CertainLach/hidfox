import { CriticalSection } from "./criticalSection";
import { WindowMessageChannel, WindowMessagePort } from "./inpage";
import { BasicListenerList, callListeners } from "./listener";
import { Address, PacketHeader } from "./packet";
import { PortRpc } from "./rpc";

const AUTHOR = 'Yaroslav Bolyukin <iam@lach.pw>';

console.log('Hello from injected!');

const channel = new WindowMessageChannel('firefoxWebHid', 'injected');

// Extension reloading
{
	if ((navigator as any).firefoxWebhid) {
		console.log('extension reload was triggered');
		(navigator as any).firefoxWebhid.cleanup();
	}

	Object.defineProperty(navigator, 'firefoxWebhid', {
		get() {
			return {
				author: AUTHOR,
				cleanup() {
					channel.close();
					delete (navigator as any).hid;
					delete (navigator as any).firefoxWebhid;
					delete (window as any).USB;
				}
			};
		},
		configurable: true,
	});
}

/// If not polyfilled/on reload
if (!(navigator as any).hid) {
	let hid: Hid;
	Object.defineProperty(navigator, 'hid', {
		get() {
			if (!hid) hid = new Hid();
			return hid;
		},
		configurable: true,
	});
}
if (!(window as any).USB) {
	let usb: USB;
	Object.defineProperty(window, 'USB', {
		get() {
			if (!usb) usb = new USB();
			return usb;
		},
		configurable: true,
	});
}

type Incoming<T> = PacketHeader & T;
type ReportData = Incoming<{
	id: number,
	data: number[],
}>;
type ConnectHID = {
	id: string,
};
type SendReport = {
	report: {
		id: number,
		data: number[],
	},
};
type SendFeatureReport = {
	report: {
		id: number,
		data: number[],
	},
};
type ConnectHIDR = {};
type HidDeviceData = (ReportData | ConnectHIDR | Incoming<{
	request: 'HidDevice',
	id: string,
}>);

type ReceiveFeatureReport = {
	id: number,
}
type ReceiveFeatureReportResponse = {
	data: number[],
};

class InputReportEvent {
	constructor(public device: HidDevice, public reportId: number, public data: Uint8Array) { }
}
const onDisconnect = Symbol("on disconnect");
class HidDevice {
	#critical = new CriticalSection('HidDevice');

	#id: string;
	vendorId: number;
	productId: number;

	#_rpc?: PortRpc;
	get #rpc() {
		if (!this.#_rpc) throw new Error('not opened');
		return this.#_rpc;
	}
	get opened() {
		return !!this.#_rpc;
	}

	#onInputreport = new BasicListenerList<InputReportEvent>('onInputreport');

	constructor(id: string, vid: number, pid: number) {
		this.#id = id;
		this.vendorId = vid;
		this.productId = pid;
	}
	async open() {
		const port = channel.connect<HidDeviceData>('HidDevice', 'content')
		const rpc = new PortRpc(Address.Injected);

		rpc.addNotificationListener<ReportData>('Report', (_sender, report) => {
			this.#onInputreport[callListeners](new InputReportEvent(this, report.id, new Uint8Array(report.data)));
		});
		// TODO: handle closing
		rpc.addDirect(Address.Content, port, 50);

		await rpc.waitForConnectionTo(Address.Background);
		await rpc.request(Address.Background, 'OpenNative', {});

		try {
			const _response = await rpc.request<ConnectHID, ConnectHIDR>(Address.Native, 'ConnectHid', { id: this.#id });
			this.#_rpc = rpc;
		} catch (e) {
			port.disconnect();
			throw e;
		}
	}
	async sendReport(id: any, data: Uint8Array) {
		this.#rpc?.notify<SendReport>(Address.Native, 'SendReport', { report: { id, data: Array.from(data) } });
	}
	async receiveFeatureReport(id: number): Promise<DataView> {
		const data = await this.#rpc?.request<ReceiveFeatureReport, ReceiveFeatureReportResponse>(Address.Native, 'ReceiveFeatureReport', {id});
		return new DataView(new Uint8Array(data.data).buffer);
	}
	async sendFeatureReport(id: number, data: Uint8Array) {
		this.#rpc?.notify<SendFeatureReport>(Address.Native, 'SendFeatureReport', { report: { id, data: Array.from(data) } });
	}
	addEventListener(name: string, handler: (evnet: unknown) => void, _opts: {}) {
		if (name === 'inputreport') return this.#onInputreport.addListener(handler);
		console.error('unknown hid device event listener:', name);
	}
	close() {
		if (this.#_rpc) {
			this.#_rpc.disconnect();
			this.#_rpc = undefined;
		}
	}
	[onDisconnect]() {
		if (this.#_rpc) {
			this.#_rpc.disconnect();
			this.#_rpc = undefined;
		}
	}
}

type DeviceInfo = {
	vendor_id: number,
	product_id: number,
	usage: number,
	usage_page: number,
};

type AddedDevice = Incoming<{
	request: 'AddedDevice',
	id: string,
	info: DeviceInfo,
}>;
type RemovedDevice = Incoming<{
	request: 'RemovedDevice',
	id: string,
}>;
type NativeInitialized = Incoming<{
	request: 'NativeInitialized',
}>;
type HidData = AddedDevice | RemovedDevice | NativeInitialized | Incoming<{ request: 'Hid' }> | Incoming<{
	request: 'RequestDevice',
}> | Incoming<{ request: 'OpenNative' }>;
type RequestDevice = {
	filters: {
		vendor_id?: number,
		product_id?: number,
		usage_page?: number,
		usage?: number,
	}[],
};

class Hid {
	#rpc: PortRpc;
	#devices = new Map<string, HidDevice>();

	#onDisconnect = new BasicListenerList('onDisconnect');

	#initialization?: Promise<unknown>;

	constructor() {
		const port = channel.connect<HidData>('Hid', 'content');
		const rpc = new PortRpc(Address.Injected);
		this.#rpc = rpc;
		rpc.addDirect(Address.Content, port, 50);

		rpc.addNotificationListener<AddedDevice>('AddedDevice', (_sender, added) => {
			const device = new HidDevice(added.id, added.info.vendor_id, added.info.product_id);
			this.#devices.set(added.id, device);
		});
		rpc.addNotificationListener<RemovedDevice>('RemovedDevice', (_sender, removed) => {
			const dev = this.#devices.get(removed.id);
			this.#devices.delete(removed.id);
			if (dev) {
				dev[onDisconnect]();
			}
		});

		port.onDisconnect.addListener((p: any) => {
			if (p.error) console.error('content script connection closed due to error:', p.error);
		});

		this.#initialization = rpc.waitForConnectionTo(Address.Background)
			.then(() => rpc.request(Address.Background, 'OpenNative', {}))
			.then(() => rpc.request(Address.Native, 'SubscribeHid', {}, 800))
			.then(() => rpc.request(Address.Native, 'PollRefresh', {}))
			.then(() => this.#initialization = undefined)

	}
	async getDevices(): Promise<HidDevice[]> {
		await this.#initialization;
		return Array.from(this.#devices.keys()).map(key => this.#devices.get(key)!);
	}
	async requestDevice(options: { filters?: { vendorId?: number, productId?: number, usagePage?: number, usage?: number }[] } = {}) {
		await this.#initialization;
		await this.#rpc.request<RequestDevice, {}>(Address.Native, 'RequestDevice', {
			filters: (options.filters ?? []).map(v => ({
				vendor_id: v.vendorId,
				product_id: v.productId,
				usage_page: v.usagePage,
				usage: v.usage,
			})),
		}, 10 * 60 * 1000);

		// Current poll interval (Which may not yet see persisted allowlist)
		await this.#rpc.request(Address.Native, 'PollRefresh', {}, 1000);
		// Next interval, which will use allowlist
		await this.#rpc.request(Address.Native, 'PollRefresh', {}, 1000);
		const devices = await this.getDevices();

		return devices.filter(dev=>{
			return (options.filters ?? []).some(filter => {
				if(filter.vendorId && filter.vendorId !== dev.vendorId) return false;
				if(filter.productId && filter.productId !== dev.productId) return false;
				// TODO: match rest
				return true;
			});
		})
	}
	addEventListener(name: string, handler: (event: unknown) => void, _opts: {}) {
		if (name === 'disconnect') return this.#onDisconnect.addListener(handler);
		console.error('unknown hid event listener:', name);
	}
}

class USB {
}
