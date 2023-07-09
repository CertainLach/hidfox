import { Address } from "../packet";
import { PortRpc } from "../rpc";

type StorageSet = { key: string, value: string, expires_in?: number };

type StorageGet = { key: string };
type StorageGetR = { value?: string };
type StorageGetPrefix = { prefix: string };
type StorageGetPrefixR = { key: string, value: string }[];

type StorageRemove = { key: string };

function deleteKey(key: string) {
	localStorage.removeItem('native:exp:' + key);
	localStorage.removeItem('native:val:' + key);
}

function getKey(key: string): string | null {
	const expire = localStorage.getItem('native:exp:' + key);
	if (expire) {
		const expireVal = +expire;
		if (expireVal > Date.now()) {
			deleteKey(key);
			return null;
		}
	}

	const value = localStorage.getItem('native:val:' + key);
	return value;
}

export function registerStorageRpc(rpc: PortRpc) {
	const thisTime = new Map();

	rpc.addRequestListener<StorageSet, {}>('StorageSet', async (sender, { key, value, expires_in }) => {
		if (sender !== Address.Native) throw new Error('storage is only for native');

		if (expires_in === 0) {
			thisTime.set(key, value);
			// Remove for persistent storage
			deleteKey(key);
			return {};
		}

		if (expires_in !== undefined) {
			localStorage.setItem('native:exp:' + key, (expires_in + Date.now()).toString());
			thisTime.set(key, value);
		}
		localStorage.setItem('native:val:' + key, value);
		return {};
	});
	rpc.addRequestListener<StorageGet, StorageGetR>('StorageGet', async (sender, { key }) => {
		if (sender !== Address.Native) throw new Error('storage is only for native');

		const thisTimeValue = thisTime.get(key);
		if (thisTimeValue !== null) {
			return { value: thisTimeValue };
		}

		const value = getKey(key);
		if (value === null) return {};

		return { value };
	});
	rpc.addRequestListener<StorageGetPrefix, StorageGetPrefixR>('StorageGetPrefix', async (sender, { prefix }) => {
		if (sender !== Address.Native) throw new Error('storage is only for native');

		const out = [];

		for (const fullKey of Object.getOwnPropertyNames(localStorage)) {
			if (!fullKey.startsWith('native:val:')) continue;
			const key = fullKey.slice(11);
			if (!key.startsWith(prefix)) continue;
			const value = getKey(key);
			// Expired
			if (value === null) continue;
			out.push({ key, value });
		}

		for (const key of thisTime.keys()) {
			if (!key.startsWith(prefix)) continue;
			const value = thisTime.get(key)!;
			out.push({ key, value });
		}

		return out;
	});
	rpc.addRequestListener<StorageRemove, {}>('StorageRemove', async (sender, { key }) => {
		if (sender !== Address.Native) throw new Error('storage is only for native');

		thisTime.delete(key);
		deleteKey(key);
		return {};
	});
}
