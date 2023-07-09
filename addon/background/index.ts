import { EXTENSION_ID, EXTENSION_NAME } from "../config";
import { PortLike, generateId } from "../inpage";
import { Address } from "../packet";
import { PortRpc } from "../rpc";
import { registerStorageRpc } from "../shared/rpcStorage";

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

type OpenNative = {};
const contentListener = (contentPort: browser.runtime.Port) => {
	console.log(contentPort);
	const rpc = new PortRpc(Address.Background);
	registerStorageRpc(rpc);

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
		const _window = await browser.windows.create({
			url: browser.runtime.getURL('popup.html') + idHash,
			type: 'popup',
			height: 600,
			width: 400,
		}).catch(windowFailed);
		windowTimeout = setTimeout(() => windowFailed(new Error('opening timeout')), 5000);
		const port: PortLike = await windowPort;
		rpc.addDirect(Address.Popup, port, 50);
		return {};
	});

	rpc.addDirect(Address.Content, contentPort, 50);

	const tabId = contentPort.sender?.tab?.id;
	if (tabId) {
		browser.pageAction.show(tabId);
		browser.pageAction.setTitle({ tabId, title: `${EXTENSION_NAME} - Page uses WebHID` });
	}
}

browser.runtime.onConnect.addListener(port => {
	if (port.name === 'popup') return popupListener(port);
	if (port.name === 'content') return contentListener(port);
	throw new Error('unknown connection source')
});

