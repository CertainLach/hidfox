import { PortLike, generateId } from "./inpage";
import { BasicListenerList, CancellationError, Listener, callListeners, waitForEvent } from "./listener";
import { Address, PacketHeader, RequestPacketHeader, ResponsePacketHeader } from "./packet";

const DEFAULT_TIMEOUT = 1000;

const handleIncoming = Symbol("handle incoming");

class Connection {
	onDisconnectListener: Listener<{ error?: Error }>;
	onMessageListener: Listener<object>;
	constructor(public rpc: PortRpc, public address: Address, public port: PortLike, public rtt: Rtt) {
		this.onDisconnectListener = (disconnect) => {
			if (disconnect.error) console.error('port disconnected with an error', disconnect.error);
			this.#cleanup();
		};
		this.onMessageListener = msg => {
			rpc[handleIncoming](address, msg as PacketHeader);
		};
		port.onDisconnect.addListener(this.onDisconnectListener as any);
		port.onMessage.addListener(this.onMessageListener as any);
	}
	#cleanup() {
		this.port.onDisconnect.removeListener(this.onDisconnectListener as any);
		this.port.onMessage.removeListener(this.onMessageListener as any);
		this.rpc.removeDirect(this.address);
	}
	disconnect() {
		this.#cleanup();
		this.port.disconnect();
	}
}

class OutgoingRequest {
	resolve!: (value: unknown) => void;
	reject!: (error: unknown) => void;
	promise: Promise<unknown>;

	constructor(cancellation: Promise<unknown>[]) {
		const promise = new Promise((res, rej) => {
			this.resolve = res;
			this.reject = rej;
		});
		this.promise = Promise.race([
			promise,
			...cancellation.map(p => p.then(() => {
				throw new CancellationError('waiting was cancelled');
			})),
		]);
	}
}

/**
 * Connection received first route/lost last route
 * TODO: split into two different packets
 */
class ConnectionListChange {
	constructor(public address: Address, public added: boolean, public onlyVia: Address | null, public rtt: Rtt) { }
}
/**
 *	Connection now has two routes
 */
class ViaListSeconded {
	constructor(public forConnection: Address, public initialVia: Address | null, public addedVia: Address | null, public rtt: Rtt) { }
}
/**
 * Connection back to one route
 */
class ViaListUnseconded {
	constructor(public forConnection: Address, public onlyVia: Address | null) { }
}

class MinRttUpdate {
	constructor(public forConnection: Address, public rtt: MinRtt, public firstChanged: boolean, public secondChanged: boolean) { }
}

/**
 *	Forwarder => Route[]
 */
class InverseRouteSet {
	#vias = new Map<Via, Set<Address>>();

	inc(via: Via, to: Address) {
		if (!this.#vias.has(via)) this.#vias.set(via, new Set());
		const current = this.#vias.get(via)!;
		if (!current.add(to)) throw new Error('inverse imbalance (double inc)')
	}
	dec(via: Via, to: Address) {
		if (!this.#vias.has(via)) throw new Error('inverse imbalance (unknown dec)');
		const current = this.#vias.get(via)!;

		if (!current.delete(to)) throw new Error('inverse imbalance (double dec)');
		if (current.size === 0) {
			this.#vias.delete(via);
		}
	}
	forwarded(via: Via): Address[] | undefined {
		const forwarded = this.#vias.get(via);
		if (!forwarded) return undefined;
		return Array.from(forwarded);
	}
}

type Rtt = number;
type Via = Address | null;

class MinRtt {
	constructor(
		public via: Via,
		public viaRtt: Rtt,
		/**
		 * Which Rtt to report to via. May be Infinity, if no second best exists
		 */
		public secondBest: Rtt,
	) { }
}

/**
 * Route => Forwarder[]
 */
class RouteSet {
	#routes = new Map<Address, Map<Via, Rtt>>();
	#inverse = new InverseRouteSet();
	#minRtt = new Map<Address, MinRtt>();

	connectionListChange = new BasicListenerList<ConnectionListChange>('connectionListChange');
	viaListSeconded = new BasicListenerList<ViaListSeconded>('viaListSeconded');
	viaListUnseconded = new BasicListenerList<ViaListUnseconded>('viaListUnseconded');
	changedMinRtt = new BasicListenerList<MinRttUpdate>('changedMinRtt');

	#updateMinRtt(address: Address) {
		const oldMinRtt = this.#minRtt.get(address);
		if (oldMinRtt === undefined) throw new Error('minRtt should be set explicitly on creation');

		const routes = this.#routes.get(address);
		if (routes === undefined) throw new Error('no routes to update');

		let minRttVia: Via | undefined;
		let minRttVal = Infinity;
		let secondMinRtt = Infinity;
		for (const [via, rtt] of routes) {
			if (rtt < minRttVal) {
				secondMinRtt = minRttVal;
				minRttVal = rtt;
				minRttVia = via;
			}
		}
		// List is now empty
		if (minRttVal === Infinity || minRttVia === undefined) throw new Error('should be unreachable');

		const minRtt = new MinRtt(minRttVia, minRttVal, secondMinRtt);
		if (oldMinRtt.via === minRtt.via && oldMinRtt.viaRtt === minRtt.viaRtt && oldMinRtt.secondBest === minRtt.secondBest) return;

		this.#minRtt.set(address, minRtt);
		const onlyFirstUpdated = oldMinRtt.viaRtt !== minRtt.viaRtt;
		const onlySecondUpdated = oldMinRtt.secondBest !== minRtt.secondBest;
		const minViaUpdated = oldMinRtt.via !== minRtt.via;
		this.changedMinRtt[callListeners](new MinRttUpdate(address, minRtt, onlyFirstUpdated || minViaUpdated, onlySecondUpdated || minViaUpdated))
	}

	inc(address: Address, via: Address | null, rtt: Rtt) {
		if (this.#routes.has(address)) {
			const routes = this.#routes.get(address)!;
			if (routes.has(via)) return console.error('added duplicate connection:', address, 'via', via);
			if (routes.size === 1) {
				const initialVia = routes.keys().next()!.value;
				this.viaListSeconded[callListeners](new ViaListSeconded(address, initialVia, via, Math.min(routes.get(initialVia)!, rtt)));
			}
			routes.set(via, rtt);
			this.#updateMinRtt(address);
		} else {
			const routes = new Map();
			routes.set(via, rtt);
			this.#routes.set(address, routes);
			this.#minRtt.set(address, new MinRtt(via, rtt, Infinity));
			this.connectionListChange[callListeners](new ConnectionListChange(address, true, via, rtt));
		}
		this.#inverse.inc(via, address);
	}
	dec(address: Address, via: Address | null) {
		if (!this.#routes.has(address)) return console.error('removed unknown connection:', address, 'via', via, '(There is no routes to the specified address)');
		const current = this.#routes.get(address)!;
		if (!current.has(via)) return console.error('removed unknown connection:', address, 'via', via);
		if (current.size === 1) {
			this.connectionListChange[callListeners](new ConnectionListChange(address, false, via, 0));
			this.#routes.delete(address);
			this.#minRtt.delete(address);
		} else {
			current.delete(via);
			if (current.size === 1) {
				const onlyVia = current.keys().next()!.value;
				this.viaListUnseconded[callListeners](new ViaListUnseconded(address, onlyVia))
			}
			this.#updateMinRtt(address);
		}
		this.#inverse.dec(via, address);
	}
	update(address: Address, via: Via, rtt: Rtt) {
		const current = this.#routes.get(address);
		if (!current) return console.error('updated rtt for the unknown connection');
		if (!current.has(via)) return console.error('updated rtt for the unknown via', via, address);
		current.set(via, rtt);
		this.#updateMinRtt(address);
	}
	has(address: Address): boolean {
		return this.#routes.has(address);
	}
	list(): [Address, MinRtt][] {
		return Array.from(this.#minRtt);
	}

	/**
	 * Can forwarder send messages on behalf of sender?
	 */
	mayBeForwarderFor(forwarder: Via, sender: Address): boolean {
		if (forwarder === sender) return true;
		const connections = this.#routes.get(sender);
		// No connection
		if (!connections) return false;
		return connections.has(forwarder);
	}
	forwarderFor(address: Address, blacklist: Set<Via> = new Set()): Via | undefined {
		const connections = this.#routes.get(address);
		// No connection
		if (!connections) return undefined;
		// Has direct connection
		if (connections.has(null)) return null;

		// Best possible non-blocked connection
		let bestConnection: Address | undefined;
		let minRtt = Infinity;
		for (const [connection, rtt] of connections) {
			// Connection is not null
			if (blacklist.has(connection!)) continue;
			if (rtt < minRtt) {
				bestConnection = connection!;
				minRtt = rtt;
			}
		}
		return bestConnection;
	}

	onAddDirectConnection(address: Address, rtt: number) {
		this.inc(address, null, rtt);
	}
	onRemoveDirectConnection(address: Address) {
		this.dec(address, null);

		// Remove all connections to the host:
		// for (const [via, _rtt] of Array.from(this.#routes.get(address) ?? [])) {
		// 	this.dec(address, via);
		// }
		// for (const forwarded of this.#inverse.forwarded(address) ?? []) {
		// 	this.dec(forwarded, address);
		// }
	}
}

type GetAddress = {};
type GetAddressR = {
	address: Address,
};
type AddForwarded = {
	to: Address,
	rtt: Rtt,
};
type RemovedForwarded = {
	to: Address,
};
type UpdatedForwardedRtt = {
	to: Address,
	rtt: Rtt,
};

export class PortRpc {
	#me: Address;

	#requestListeners = new Map<string, (from: Address, data: object) => Promise<object>>;
	#notificationListeners = new Map<string, (from: Address, data: object) => void>;

	#connections: Connection[] = [];
	routeSet = new RouteSet();

	#pendingOutgoingRequests = new Map<string, OutgoingRequest>();

	constructor(me: Address) {
		this.#me = me;

		this.addNotificationListener<AddForwarded>('AddForwarded', async (sender, add) => {
			this.#addForwarded(sender, add.to);
		});
		this.addNotificationListener<RemovedForwarded>('RemoveForwarded', async (sender, remove) => {
			this.#removeForwarded(sender, remove.to);
		});
		this.addNotificationListener<UpdatedForwardedRtt>('UpdatedForwardedRtt', async (sender, update) => {
			this.routeSet.update(sender, update.to, update.rtt);
		});

		this.routeSet.connectionListChange.addListener((change) => {
			for (const connection of this.#connections) {
				if (change.address === connection.address) continue;
				//throw new Error('connection should be removed from connections before this event is fired');

				// Ignore connections, which we have forwarded by this (<1)
				if (change.onlyVia === connection.address) continue;
				if (change.added) {
					this.notify<AddForwarded>(connection.address, 'AddForwarded', { to: change.address, rtt: change.rtt })
				} else {
					this.notify<RemovedForwarded>(connection.address, 'RemoveForwarded', { to: change.address })
				}
			}
		});
		// Second path to the destination was added, add connections skipped by (<1)
		this.routeSet.viaListSeconded.addListener(seconded => {
			for (const connection of this.#connections) {
				if (seconded.initialVia !== connection.address) continue;
				this.notify<AddForwarded>(connection.address, 'AddForwarded', { to: seconded.forConnection, rtt: seconded.rtt })
			}
		});
		// Remove seconded connection
		this.routeSet.viaListUnseconded.addListener(seconded => {
			for (const connection of this.#connections) {
				if (seconded.onlyVia !== connection.address) continue;
				this.notify<RemovedForwarded>(connection.address, 'RemoveForwarded', { to: seconded.forConnection })
			}
		});
		this.routeSet.changedMinRtt.addListener(changed => {
			for (const connection of this.#connections) {
				const viaIsThis = connection.address == changed.rtt.via;
				// Only announce rtt for other hosts
				const rtt = viaIsThis ? changed.rtt.secondBest : changed.rtt.viaRtt;
				const updated = viaIsThis ? changed.secondChanged : changed.firstChanged;
				if (updated && rtt !== Infinity) {
					this.notify<UpdatedForwardedRtt>(connection.address, 'UpdatedForwardedRtt', { to: changed.forConnection, rtt });
				}
			}
		})
	}

	#addForwarded(via: Address, forwarded: Address) {
		const connection = this.#directConnectionFor(via);
		if (!connection) return console.error('via should be dirrectly connected');
		this.routeSet.inc(forwarded, via, connection.rtt)
	}
	#removeForwarded(via: Address, forwarded: Address) {
		const connection = this.#directConnectionFor(via);
		if (!connection) return console.error('via should be dirrectly connected');
		this.routeSet.dec(forwarded, via)
	}

	async #handleIncomingRequest(comingFrom: null | Address, p: RequestPacketHeader) {
		if (p.sender !== this.#me && !this.routeSet.mayBeForwarderFor(comingFrom, p.sender)) return console.error('messages from', p.sender, 'should not be forwarded through', comingFrom);

		if (p.receiver === this.#me) {
			if (p.response) {
				const request = this.#requestListeners.get(p.request);
				if (!request) {
					return console.error('no request listener registered for', p.request);
				}
				let response;
				try {
					response = await request(p.sender, p);
				} catch (e) {
					console.error('request listener for', p.request, 'failed with', e);
					response = { error: e instanceof Error ? e.message : '<unknown>' };
				}
				const data: ResponsePacketHeader = Object.assign(response, {
					request_origin: p.sender,
					rid: p.response.rid,
				});
				this.#handleIncomingResponse(null, data);
			} else {
				const notification = this.#notificationListeners.get(p.request);
				if (!notification) {
					return console.error('no notification listener registered for', p.request);
				}
				try {
					notification(p.sender, p);
				} catch (e) {
					return console.error('notification listener for', p.request, 'failed with', e);
				}
			}
			return;
		}
		// FIXME: this is still possible packet may be stuck in the loop, the whole path should be blacklisted.
		const nextHop = this.#connectionFor(p.receiver, new Set([comingFrom]));
		if (!nextHop) {
			if (p.response) {
				const response = { error: 'could not forward message: no connection' };
				const packet: ResponsePacketHeader = Object.assign(response, {
					request_origin: p.sender,
					rid: p.response.rid,
				});
				this.#handleIncomingResponse(null, packet);
			}
			return console.error('could not forward packet', p);
		}
		nextHop.port.postMessage(p);
	}
	async #handleIncomingResponse(comingFrom: null | Address, p: ResponsePacketHeader) {
		if (p.request_origin == this.#me) {
			const outgoing = this.#pendingOutgoingRequests.get(p.rid);
			if (!outgoing) return console.error('received response for unknown request', p);
			if (p.error) {
				outgoing.reject(new Error(p.error));
			} else {
				outgoing.resolve(p);
			}
			return;
		}
		// FIXME: this is still possible packet may be stuck in the loop, the whole path should be blacklisted.
		const nextHop = this.#connectionFor(p.request_origin, new Set([comingFrom]));
		if (!nextHop) return console.error('could not forward packet', p);
		nextHop.port.postMessage(p);
	}
	[handleIncoming](comingFrom: Via, p: PacketHeader) {
		if ('rid' in p) this.#handleIncomingResponse(comingFrom, p);
		else this.#handleIncomingRequest(comingFrom, p);
	}

	#directConnectionFor(address: Address): Connection | undefined {
		for (const connection of this.#connections) {
			if (connection.address == address) return connection;
		}
	}
	#forwarderFor(address: Address, blacklist: Set<Address | null> = new Set()): Connection | undefined {
		const via = this.routeSet.forwarderFor(address, blacklist);
		if (via === undefined) return undefined;
		return this.#directConnectionFor(via!);
	}
	#connectionFor(address: Address, blacklist: Set<Address | null> = new Set()): Connection | undefined {
		return this.#directConnectionFor(address) ?? this.#forwarderFor(address, blacklist);
	}

	removeDirect(to: Address) {
		const index = this.#connections.findIndex(c => c.address === to);
		if (index === -1) return console.error('connection doesn\'t exists', to);

		this.#connections.splice(index, 1);
		this.routeSet.onRemoveDirectConnection(to);
	}
	addDirect(to: Address, port: PortLike, rtt: Rtt) {
		for (const connection of this.#connections) if (connection.address === to) return console.error('connection was already added', to);

		const connection = new Connection(this, to, port, rtt);
		this.#connections.push(connection);

		for (const [route, minRtt] of this.routeSet.list()) {
			const rtt = minRtt.via === to ? minRtt.secondBest : minRtt.viaRtt;
			if (rtt !== Infinity) this.notify<AddForwarded>(to, 'AddForwarded', { to: route, rtt });
		}
		this.routeSet.onAddDirectConnection(to, rtt);
	}

	notify<T extends object>(to: Address, request: string, data: T) {
		let packet: RequestPacketHeader = Object.assign(data, {
			sender: this.#me,
			receiver: to,
			request,
		});
		this.#handleIncomingRequest(null, packet);
	}

	async request<Req extends object, Res extends object>(to: Address, request: string, data: Req, timeoutMs: number = DEFAULT_TIMEOUT): Promise<Res> {
		let rid = generateId();
		// Support for drifting
		let timedOutAt = Date.now() + timeoutMs;
		let packet: PacketHeader = Object.assign(data, {
			sender: this.#me,
			receiver: to,
			request,
			response: {
				rid,
				timedOutAt,
			},
		});


		let timeoutId: ReturnType<typeof setTimeout> | undefined;
		const timeout = new Promise((_, rej) => timeoutId = setTimeout(() => {
			console.error('timed out request:', request, data);
			rej(new Error(`timed out request: ${request}`));
		}, timeoutMs));

		const outgoing = new OutgoingRequest([timeout]);
		this.#pendingOutgoingRequests.set(rid, outgoing);

		this.#handleIncomingRequest(null, packet);

		try {
			return await outgoing.promise as Res;
		} finally {
			this.#pendingOutgoingRequests.delete(rid);
			clearTimeout(timeoutId);
		}
	}

	async waitForConnectionTo(address: Address, timeoutMs: number = DEFAULT_TIMEOUT): Promise<void> {
		if (this.routeSet.has(address)) return;
		await waitForEvent<ConnectionListChange, ConnectionListChange>(
			this.routeSet.connectionListChange, (v): v is ConnectionListChange => v.address === address && v.added === true, timeoutMs, []
		)
	}

	addRequestListener<T extends object, R extends object>(request: string, handler: (from: Address, data: T) => Promise<R>) {
		if (this.#requestListeners.has(request)) throw new Error(`listener is already registered for ${request}`);
		this.#requestListeners.set(request, handler as any)
	}
	addNotificationListener<T extends object>(request: string, handler: (from: Address, data: T) => void) {
		if (this.#notificationListeners.has(request)) throw new Error(`listener is already registered for ${request}`);
		this.#notificationListeners.set(request, handler as any)
	}

	disconnect() {
		for (const connection of this.#connections) {
			connection.port.disconnect();
		}
	}
}

