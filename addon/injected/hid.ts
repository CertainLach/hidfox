import { WindowMessageChannel } from "../inpage";
import { BasicEventTarget, BasicListenerList, defineEvent } from "../listener";
import { Address } from "../packet";
import { PortRpc } from "../rpc";
import { HidDevice, hidDeviceOnDisconnect } from "./hidDevice";

type DeviceInfo = {
	vendor_id: number,
	product_id: number,
	usage: number,
	usage_page: number,
};

type AddedDevice = {
	id: string,
	info: DeviceInfo,
};
type RemovedDevice = {
	id: string,
};
type RequestDevice = {
	filters: {
		vendor_id?: number,
		product_id?: number,
		usage_page?: number,
		usage?: number,
	}[],
};

export class Hid extends BasicEventTarget {
	#rpc: PortRpc;
	#devices = new Map<string, HidDevice>();

	#onConnect = new BasicListenerList('onConnect');
	#onDisconnect = new BasicListenerList('onDisconnect');

	#initialization?: Promise<unknown>;

	constructor(channel: WindowMessageChannel) {
		super();
		this[defineEvent]('connect', this.#onConnect);
		this[defineEvent]('disconnect', this.#onDisconnect);

		const port = channel.connect('Hid', 'content');
		const rpc = new PortRpc(Address.Injected);
		this.#rpc = rpc;
		rpc.addDirect(Address.Content, port, 50);

		rpc.addNotificationListener<AddedDevice>('AddedDevice', (_sender, added) => {
			const device = new HidDevice(added.id, added.info.vendor_id, added.info.product_id, channel);
			this.#devices.set(added.id, device);
		});
		rpc.addNotificationListener<RemovedDevice>('RemovedDevice', (_sender, removed) => {
			const dev = this.#devices.get(removed.id);
			this.#devices.delete(removed.id);
			if (dev) {
				dev[hidDeviceOnDisconnect]();
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

		return devices.filter(dev => {
			return (options.filters ?? []).some(filter => {
				if (filter.vendorId && filter.vendorId !== dev.vendorId) return false;
				if (filter.productId && filter.productId !== dev.productId) return false;
				// TODO: match rest
				return true;
			});
		})
	}
}
