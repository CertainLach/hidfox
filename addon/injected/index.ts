import { AUTHOR } from "../config";
import { WindowMessageChannel } from "../inpage";
import { Hid } from "./hid";


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
			if (!hid) hid = new Hid(channel);
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



class USB {
}

