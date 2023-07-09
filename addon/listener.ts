export const callListeners = Symbol("call listeners");
export type Listener<E> = (event: E) => void;

/**
 * Mimics interface of Port.onMessage and similar objects
 */
export class BasicListenerList<E> implements ListenerListLike<E> {
	#name: string;
	constructor(name = '<unnamed>') {
		this.#name = name;
	}

	#listeners: Array<Listener<E>> = [];
	addListener<F extends E>(handler: (event: F) => void) {
		this.#listeners.push(handler as Listener<E>);
	}
	removeListener(handler: (event: E) => void) {
		const index = this.#listeners.indexOf(handler);
		if (index === -1) throw new Error('listener not found');
		this.#listeners.splice(index, 1);
	}
	[callListeners](event: E) {
		let i = 0;
		let found = false;
		while (i < this.#listeners.length) {
			const listener = this.#listeners[i];
			try {
				listener(event);
				found = true;
			} catch (e) {
				console.error('event listener failed', this.#name, e);
				debugger;
			}
			i++;
		}
		if (!found)
			console.error('no listener found for', this.#name, event)
	}
}

export interface ListenerListLike<E = unknown> {
	addListener(handler: Listener<E>): void;
	removeListener(handler: Listener<E>): void;
}

/**
 * Wait for specific listener event, waiting for either matching guard, or timeout.
 * 
 * Throws TimeoutError when timeout is reached
 */
export async function waitForEvent<R, E extends R>(
	event: ListenerListLike<R>,
	guard: (v: R) => v is E,
	timeoutMs: number,
	cancellation: Promise<unknown>[],
): Promise<E> {
	if (timeoutMs < 10 || timeoutMs > 60000) throw new Error('expected sane timeout value');

	let timeoutId: ReturnType<typeof setTimeout> | undefined;
	let listener: (event: R) => void | undefined;

	const cleanup = () => {
		clearTimeout(timeoutId);
		event.removeListener(listener);
	};

	const timeout = new Promise<E>((_, rej) => {
		timeoutId = setTimeout(() => {
			cleanup();
			rej(new TimeoutError('event'))
		}, timeoutMs);
	});
	const result = new Promise<E>((res, rej) => {
		listener = (event: R) => {
			try {
				if (guard(event)) {
					cleanup();
					res(event as E);
				}
			} catch (e) {
				cleanup();
				rej(e);
			}
		};
	});
	event.addListener(listener!);

	return Promise.race([timeout, result, ...cancellation.map(p => p.then(() => {
		throw new CancellationError('waiting was cancelled');
	}))]);
}

export class TimeoutError extends Error {
	constructor(msg: string) {
		super(msg);
		this.name = 'TimeoutError';
	}
}
export class CancellationError extends Error {
	constructor(msg: string) {
		super(msg);
		this.name = 'CancellationError';
	}
}

export const defineEvent = Symbol('defineEvent');
export class BasicEventTarget {
	#handlers = new Map<string, BasicListenerList<unknown>>();
	[defineEvent](name: string, list: BasicListenerList<unknown>) {
		let propertyListener: any = null;
		Object.defineProperties(this, {
			[`on${name}`]: {
				get: () => propertyListener,
				set(v) {
					if (typeof propertyListener === 'function') list.removeListener(propertyListener);
					if (typeof v === 'function') list.addListener(v);
					propertyListener = v;
				},
			}
		})
	}
	addEventListener(name: string, handler: (event: unknown) => void, _opts = {}) {
		const list = this.#handlers.get(name);
		if (!list) return;
		list.addListener(handler);
	}
	removeEventListener(name: string, handler: (event: unknown) => void, _opts = {}) {
		const list = this.#handlers.get(name);
		if (!list) return;
		list.removeListener(handler);
	}
}
