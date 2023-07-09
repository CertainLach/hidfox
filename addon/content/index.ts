import { WindowMessageChannel } from "../inpage";
import { Address } from "../packet";
import { PortRpc } from "../rpc";

const channel = new WindowMessageChannel('firefoxWebHid', 'content');

channel.onConnect.addListener(injectedPort => {
	const rpc = new PortRpc(Address.Content);

	rpc.addDirect(Address.Background, browser.runtime.connect({ name: 'content' }), 50);
	rpc.addDirect(Address.Injected, injectedPort, 50);
})

const script = document.createElement('script');
script.setAttribute('type', 'text/javascript');
script.setAttribute('src', browser.runtime.getURL('injected.js'));
document.documentElement.appendChild(script);

