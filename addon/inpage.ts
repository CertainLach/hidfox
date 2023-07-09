// Port polyfill for injected<->content communication

import { BasicListenerList, ListenerListLike, callListeners } from "./listener";

type InPagePacket = {
	id: string;
	recipient: string;
} & ({
	request: 'openPort',
	name: string,
	initiator: string,
} | {
	request: 'message',
	data: any,
} | {
	request: 'disconnect',
	error?: string,
});

export function generateId() {
	return Array(16).fill(0).map(() => Math.ceil(Math.random() * 256).toString(16)).join('');
}

const handleIncomingMessage = Symbol("handle incoming message");
const ports = Symbol("ports");
export class WindowMessageChannel {
	[ports] = new Map<string, WindowMessagePort>();
	onConnect = new BasicListenerList<WindowMessagePort>('onConnect');
	#messageListener: (e: MessageEvent) => void;
	#thisRecipient: string;
	constructor(
		/// Messages are identified by this field, in which all the data is contained
		public identifier: string,
		/// Channel instance: multiple channels may share the same identifier, thing that differentiates them is the recipient
		thisRecipient: string,
	) {
		this.#thisRecipient = thisRecipient;
		const STARTED_AT = new Date();
		const messageListener = (event: MessageEvent) => {
			if (event.source !== window) return;
			const p: InPagePacket = event.data?.[identifier];
			if (!p) return;
			if (p.recipient !== thisRecipient) return;
			if (!p.id || !p.request) return console.error('malformed message', p);

			if (p.request === 'openPort') {
				const port = new WindowMessagePort(this, p.id, p.initiator, false);
				this.onConnect[callListeners](port);
				return;
			}

			const handler = this[ports].get(p.id);
			if (!handler) {
				console.error('unknown port referenced:', p, STARTED_AT);
				return;
			}
			handler[handleIncomingMessage](p);
		};
		this.#messageListener = messageListener;
		window.addEventListener('message', messageListener);
	}
	connect(name: string, recipient: string): WindowMessagePort {
		this.ensureConnected();
		const newId = generateId();
		const port = new WindowMessagePort(this, newId, recipient, true);
		const msg = { [this.identifier]: { id: newId, request: 'openPort', name, initiator: this.#thisRecipient, recipient } as InPagePacket };
		window.postMessage(msg);
		return port;
	}

	#connected = true;
	ensureConnected() {
		if (!this.#connected) throw new Error('channel is closed');
	}
	close() {
		this.ensureConnected();
		this.#connected = false;
		this[ports].forEach(port => {
			if (port[isOutgoing]) {
				port.disconnect();
			} else {
				port[handleIncomingMessage]({ id: 'ignored', recipient: 'ignored', request: 'disconnect', error: 'channel is closing' })
			}
		});
		window.removeEventListener('message', this.#messageListener);
	}
}

const isOutgoing = Symbol("is outgoing");

/**
 * To unify transport between all components of this thing, everything is using Port api
 * Unfortunately, there is no port api for injected<->content communication, and this class implements such
 * communication over window.sendMessage()
 */
export class WindowMessagePort {
	#channel: WindowMessageChannel;
	#id: string;
	#recipient: string;
	#connected = true;
	[isOutgoing]: boolean;

	onMessage = new BasicListenerList<any>('onMessage');
	onDisconnect = new BasicListenerList<DisconnectEvent>('onDisconnect');
	constructor(channel: WindowMessageChannel, id: string, recipient: string, outgoing: boolean) {
		this.#channel = channel;
		this.#id = id;
		this.#recipient = recipient;
		this[isOutgoing] = outgoing
		console.log('registered port', id);
		channel[ports].set(this.#id, this as WindowMessagePort);
	}
	postMessage(data: any) {
		this.#send('message', data);
	}
	[handleIncomingMessage](p: InPagePacket) {
		switch (p.request) {
			case 'message': return this.onMessage[callListeners](p.data);
			case 'disconnect': {
				if (!this.#connected) throw new Error('already disconnected');
				this.#channel[ports].delete(this.#id);
				this.#connected = false;
				return this.onDisconnect[callListeners](new DisconnectEvent(p.error !== undefined ? new Error(p.error) : undefined))
			}
			default:
				console.error('unknown inpage message:', p);
		}
	}
	#disconnecting = false;
	disconnect() {
		if (this.#disconnecting) throw new Error('already disconnecting');
		this.#disconnecting = true;
		this.#send('disconnect', {});
	}
	#send(request: string, data?: unknown) {
		const msg = { [this.#channel.identifier]: { id: this.#id, request, data, recipient: this.#recipient } as InPagePacket };
		window.postMessage(msg);
	}
}

class DisconnectEvent {
	constructor(public error?: Error) { }
}

export interface PortLike {
	onMessage: ListenerListLike;
	onDisconnect: ListenerListLike;

	postMessage(data: unknown): void;
	disconnect(): void;
}
