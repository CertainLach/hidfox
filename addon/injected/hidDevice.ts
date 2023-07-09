import { WindowMessageChannel } from "../inpage";
import { BasicEventTarget, BasicListenerList, callListeners, defineEvent } from "../listener";
import { Address } from "../packet";
import { PortRpc } from "../rpc";

type ReportData = {
	id: number,
	data: number[],
};
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

type ForgetDevice = {
	id: string;
};

type ReceiveFeatureReport = {
	id: number,
}
type ReceiveFeatureReportResponse = {
	data: number[],
};

class InputReportEvent {
	constructor(public device: HidDevice, public reportId: number, public data: Uint8Array) { }
}
export const hidDeviceOnDisconnect = Symbol("on disconnect");
export class HidDevice extends BasicEventTarget {
	#id: string;
	vendorId: number;
	productId: number;
	#windowMessageChannel: WindowMessageChannel;

	#_rpc?: PortRpc;
	get #rpc() {
		if (!this.#_rpc) throw new Error('not opened');
		return this.#_rpc;
	}
	get opened() {
		return !!this.#_rpc;
	}

	#onInputreport = new BasicListenerList<InputReportEvent>('onInputreport');

	constructor(id: string, vid: number, pid: number, channel: WindowMessageChannel) {
		super();
		this[defineEvent]('inputreport', this.#onInputreport as any);

		this.#id = id;
		this.vendorId = vid;
		this.productId = pid;
		this.#windowMessageChannel = channel;
	}
	async open() {
		const port = this.#windowMessageChannel.connect('HidDevice', 'content')
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
		const data = await this.#rpc?.request<ReceiveFeatureReport, ReceiveFeatureReportResponse>(Address.Native, 'ReceiveFeatureReport', { id });
		return new DataView(new Uint8Array(data.data).buffer);
	}
	async sendFeatureReport(id: number, data: Uint8Array) {
		this.#rpc?.notify<SendFeatureReport>(Address.Native, 'SendFeatureReport', { report: { id, data: Array.from(data) } });
	}
	close() {
		if (this.#_rpc) {
			this.#_rpc.disconnect();
			this.#_rpc = undefined;
		}
	}
	forget() {
		this.#_rpc?.notify<ForgetDevice>(Address.Native, 'ForgetDevice', { id: this.#id });
		this.close();
	}
	[hidDeviceOnDisconnect]() {
		if (this.#_rpc) {
			this.#_rpc.disconnect();
			this.#_rpc = undefined;
		}
	}
}
