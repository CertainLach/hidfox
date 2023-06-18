export class CriticalSection {
	#busy = false;
	#queue: Array<(value?: unknown) => void> = [];
	#name: string;
	constructor(name: string) {
		this.#name = `CriticalSection(${name})`;
	}

	enter() {
		console.time(this.#name);
		return new Promise(resolve => {
			this.#queue.push(resolve);

			if (!this.#busy) {
				this.#busy = true;
				this.#queue.shift()!();
			}
		});
	}

	leave() {
		if (this.#queue.length) {
			this.#queue.shift()!();
		} else {
			this.#busy = false;
		}
		console.timeEnd(this.#name);
	}
}
