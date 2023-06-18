import { EXTENSION_ID } from "./config";
import { PortLike, generateId } from "./inpage";
import { Address } from "./packet";
import { PortRpc } from "./rpc";

console.log('Hello from background!');

let popupListeners = new Map<string, (res: PortLike) => void>();
const popupListener = (popupPort: browser.runtime.Port) => {
	if ((popupPort.sender as any).envType !== 'addon_child') throw new Error('invalid popup env');
	if ((popupPort.sender as any).id !== EXTENSION_ID) throw new Error('invalid popup extension');
	const url = new URL(popupPort.sender!.url!);
	let hash = url.hash;
	if (hash) {
		console.log(hash);
		popupListeners.get(hash)!(popupPort);
	} else {
		throw new Error('unknown popup');
	}
};

type StorageSet = { key: string, value: string };
type StorageGet = { key: string };
type StorageGetR = { value?: string };
type StorageRemove = { key: string };
type OpenNative = {};

browser.runtime.onConnect.addListener(contentPort => {
	if (contentPort.name === 'popup') return popupListener(contentPort);

	const rpc = new PortRpc(Address.Background);

	rpc.addRequestListener<StorageSet, {}>('StorageSet', async (_sender, { key, value }) => {
		localStorage.setItem(key, value);
		return {};
	});
	rpc.addRequestListener<StorageGet, StorageGetR>('StorageGet', async (_sender, { key }) => {
		return {
			value: localStorage.getItem(key) ?? undefined,
		};
	});
	rpc.addRequestListener<StorageRemove, {}>('StorageRemove', async (_sender, { key }) => {
		localStorage.removeItem(key);
		return {};
	});
	rpc.addRequestListener<OpenNative, {}>('OpenNative', async (_sender, { }) => {
		const url = contentPort.sender?.url;
		rpc.addDirect(Address.Native, browser.runtime.connectNative('hidfox'), 50);
		rpc.notify(Address.Native, 'OpenFromInject', { url });
		return {};
	});
	rpc.addRequestListener<{}, {}>('OpenPopup', async (_sender, { }) => {
		let idHash = '#' + generateId();

		let windowFailed!: (e: Error) => void;
		let windowTimeout!: ReturnType<typeof setTimeout>;
		const windowPort = new Promise<PortLike>((res, rej) => {
			windowFailed = rej;
			popupListeners.set(idHash, res);
		}).finally(() => {
			popupListeners.delete(idHash);
			clearTimeout(windowTimeout);
		})
		const window = await browser.windows.create({
			url: browser.runtime.getURL('popup.html') + idHash,
			type: 'popup',
			height: 600,
			width: 400,
		}).catch(windowFailed);
		windowTimeout = setTimeout(() => windowFailed(new Error('opening timeout')), 5000);
		console.log('wait port');
		const port: PortLike = await windowPort;
		console.log('add port');
		rpc.addDirect(Address.Popup, port, 50);
		console.log('popup opened');
		return {};
	})

	rpc.addDirect(Address.Content, contentPort, 50);
});

